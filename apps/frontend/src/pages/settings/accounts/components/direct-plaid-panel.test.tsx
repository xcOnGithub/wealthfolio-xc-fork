import { render, screen, waitFor, fireEvent, cleanup } from "@/test/render";
import { MemoryRouter } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { DirectPlaidPanel } from "./direct-plaid-panel";
import type { Account } from "@/lib/types";
import type { PlaidConnection, PlaidPreview } from "@/adapters/shared/plaid";
import messages from "@/i18n/locales/en/connect.json";

const state = vi.hoisted(() => ({
  connections: [] as PlaidConnection[],
  institutions: ["chase"] as string[],
  web: true,
  enabled: true,
  invoke: vi.fn(),
  invalidate: vi.fn(),
}));
vi.mock("@/adapters", () => ({
  get isWeb() {
    return state.web;
  },
  plaidAction: state.invoke,
}));
vi.mock("@tanstack/react-query", () => ({
  useQuery: () => ({
    data: {
      enabled: state.enabled,
      institutions: state.institutions,
      connections: state.connections,
    },
    isLoading: false,
  }),
  useQueryClient: () => ({ invalidateQueries: state.invalidate }),
}));
afterEach(cleanup);
const show = (path = "/settings/accounts", accounts: Account[] = []) =>
  render(
    <MemoryRouter initialEntries={[path]}>
      <DirectPlaidPanel accounts={accounts} onCreateAccount={vi.fn()} />
    </MemoryRouter>,
  );
beforeEach(() => {
  state.connections = [];
  state.institutions = ["chase"];
  state.web = true;
  state.enabled = true;
  state.invoke.mockReset();
  state.invalidate.mockReset();
});
describe("direct Plaid account setup", () => {
  it.each([
    [false, true],
    [true, false],
  ])("hides unsupported or disabled runtime (%s, %s)", (web, enabled) => {
    state.web = web;
    state.enabled = enabled;
    show();
    expect(screen.queryByTestId("direct-plaid-panel")).not.toBeInTheDocument();
    expect(state.invoke).not.toHaveBeenCalled();
  });
  it("consumes a callback once, keeps unfinished flow retryable, and displays safe error", async () => {
    state.invoke.mockRejectedValue(new Error("Plaid data is not ready; try again later"));
    show("/settings/accounts?plaidFlow=fixture-flow");
    await waitFor(() => expect(screen.getByRole("alert")).toHaveTextContent("not ready"));
    expect(state.invoke).toHaveBeenCalledExactlyOnceWith({
      operation: "finish",
      flowId: "fixture-flow",
    });
    fireEvent.click(screen.getByRole("button", { name: "Retry" }));
    await waitFor(() => expect(state.invoke).toHaveBeenCalledTimes(2));
  });
  it("clears a successful callback hint and refreshes account status", async () => {
    state.invoke.mockResolvedValue({ connectionId: "connected" });
    show("/settings/accounts?plaidFlow=fixture-flow");
    await waitFor(() => expect(state.invalidate).toHaveBeenCalled());
    await waitFor(() =>
      expect(screen.queryByRole("button", { name: "Retry" })).not.toBeInTheDocument(),
    );
    expect(state.invoke).toHaveBeenCalledTimes(1);
  });
  it("discloses hosted consent and additional-login quota before connecting", () => {
    show();
    expect(screen.getByText(messages.plaid.description)).toBeVisible();
    expect(screen.getByText(messages.plaid.anotherLogin)).toBeVisible();
    expect(screen.getByRole("combobox", { name: messages.plaid.institution })).toHaveTextContent(
      "Chase",
    );
  });
});

it.each(["ltr", "rtl"])(
  "requires classification and preserves the reviewed opening/digest in %s",
  async (direction) => {
    document.documentElement.dir = direction;
    state.connections = [
      {
        id: "connection",
        institution: "chase",
        product: "transactions",
        status: "connected",
        accounts: [{ id: "mapping", accountId: "local", status: "initial_review" }],
      },
    ];
    const preview: PlaidPreview = {
      mappingId: "mapping",
      digest: "review-1",
      initial: true,
      currency: "USD",
      suggestedOpening: "-230",
      records: [
        {
          transaction_id: "record",
          date: "2026-11-01",
          amount: "20",
          name: "Synthetic purchase",
          pending: false,
        },
      ],
      removedRecords: [
        {
          activityId: "plaid-removed-record",
          accountId: "local",
          name: "Removed purchase",
          date: "2026-11-01",
          amount: "10",
          currency: "USD",
        },
      ],
      reviewRequired: ["record"],
      decisions: {},
    };
    state.invoke.mockImplementation(async (action) => {
      if (action.operation === "preview") return preview;
      if (action.operation === "decide") {
        preview.reviewRequired = [];
        preview.decisions.record = { kind: "expense" };
        preview.digest = "review-2";
        return null;
      }
      return null;
    });
    show();
    fireEvent.click(screen.getByRole("button", { name: messages.plaid.review }));
    await screen.findByRole("combobox", {
      name: `${messages.plaid.classification} Synthetic purchase`,
    });
    expect(screen.getByRole("link", { name: /Removed purchase/ })).toHaveAttribute(
      "href",
      "/accounts/local?tab=activities",
    );
    expect(screen.getByText("plaid-removed-record")).toBeVisible();
    expect(screen.getByRole("button", { name: "Confirm" })).toBeDisabled();
    fireEvent.change(
      screen.getByRole("combobox", { name: `${messages.plaid.classification} Synthetic purchase` }),
      { target: { value: "expense" } },
    );
    await waitFor(() => expect(screen.getByRole("button", { name: "Confirm" })).toBeEnabled());
    fireEvent.click(screen.getByRole("button", { name: "Confirm" }));
    await waitFor(() =>
      expect(state.invoke).toHaveBeenCalledWith({
        operation: "approve",
        mappingId: "mapping",
        digest: "review-2",
        openingBalance: "-230",
      }),
    );
    document.documentElement.dir = "ltr";
  },
);

it("resumes interrupted validation even when the callback query is gone", async () => {
  state.connections = [
    {
      id: "saved-flow",
      institution: "chase",
      product: "transactions",
      status: "validating",
      accounts: [],
    },
  ];
  state.invoke.mockResolvedValue({ connectionId: "saved-flow" });
  show();
  fireEvent.click(screen.getByRole("button", { name: "Retry" }));
  await waitFor(() =>
    expect(state.invoke).toHaveBeenCalledExactlyOnceWith({
      operation: "finish",
      flowId: "saved-flow",
    }),
  );
});

it.each(["ltr", "rtl"])(
  "can undo a mistaken transfer pair and finish review after reload in %s",
  async (direction) => {
    document.documentElement.dir = direction;
    state.connections = [
      {
        id: "connection",
        institution: "chase",
        product: "transactions",
        status: "connected",
        accounts: [
          { id: "a", accountId: "local-a", status: "initial_review" },
          { id: "b", accountId: "local-b", status: "initial_review" },
        ],
      },
    ];
    const previews: Record<string, PlaidPreview> = Object.fromEntries(
      ["a", "b"].map((id) => [
        id,
        {
          mappingId: id,
          digest: "initial",
          initial: true,
          currency: "USD",
          suggestedOpening: "250",
          records: [
            {
              transaction_id: id,
              date: "2026-11-01",
              amount: id === "a" ? "10" : "-10",
              name: id === "a" ? "Synthetic outgoing" : "Synthetic incoming",
              pending: false,
            },
          ],
          reviewRequired: [id],
          decisions: {},
        },
      ]),
    );
    state.invoke.mockImplementation(async (action) => {
      if (action.operation === "preview") return structuredClone(previews[action.mappingId]);
      if (action.operation === "pair") {
        for (const id of ["a", "b"]) {
          previews[id].decisions[id] = { kind: "internal", group: "pair" };
          previews[id].reviewRequired = [];
          previews[id].digest = "paired";
        }
      }
      if (action.operation === "decide") {
        const current = previews[action.mappingId];
        if (current.decisions[action.recordId]?.group) {
          delete previews.b.decisions.b;
          previews.b.reviewRequired = ["b"];
          previews.b.digest = "unpaired";
        }
        current.decisions[action.recordId] = { kind: action.kind };
        current.reviewRequired = [];
        current.digest = "reviewed";
      }
      return null;
    });
    show();
    fireEvent.click(screen.getAllByRole("button", { name: messages.plaid.review })[0]);
    const label = `${messages.plaid.classification} Synthetic outgoing`;
    fireEvent.change(await screen.findByRole("combobox", { name: label }), {
      target: { value: "pair" },
    });
    fireEvent.change(screen.getByRole("combobox", { name: messages.plaid.localAccount }), {
      target: { value: "b" },
    });
    fireEvent.click(await screen.findByRole("button", { name: /Synthetic incoming/ }));
    await waitFor(() =>
      expect(screen.getByRole("combobox", { name: label })).toHaveValue("internal"),
    );
    fireEvent.change(screen.getByRole("combobox", { name: label }), {
      target: { value: "external" },
    });
    await waitFor(() =>
      expect(screen.getByRole("combobox", { name: label })).toHaveValue("external"),
    );
    fireEvent.click(screen.getByRole("button", { name: "Refresh" }));
    await waitFor(() =>
      expect(state.invoke).toHaveBeenCalledWith({
        operation: "preview",
        mappingId: "a",
        refresh: true,
      }),
    );
    await waitFor(() => expect(screen.getByRole("button", { name: "Confirm" })).toBeEnabled());
    fireEvent.click(screen.getByRole("button", { name: "Confirm" }));
    await waitFor(() =>
      expect(state.invoke).toHaveBeenCalledWith({
        operation: "approve",
        mappingId: "a",
        digest: "reviewed",
        openingBalance: "250",
      }),
    );
    fireEvent.click(screen.getAllByRole("button", { name: messages.plaid.review })[1]);
    const otherLabel = `${messages.plaid.classification} Synthetic incoming`;
    await screen.findByRole("combobox", { name: otherLabel });
    expect(screen.getByRole("button", { name: "Confirm" })).toBeDisabled();
    fireEvent.change(screen.getByRole("combobox", { name: otherLabel }), {
      target: { value: "external" },
    });
    await waitFor(() => expect(screen.getByRole("button", { name: "Confirm" })).toBeEnabled());
    fireEvent.click(screen.getByRole("button", { name: "Confirm" }));
    await waitFor(() =>
      expect(state.invoke).toHaveBeenCalledWith({
        operation: "approve",
        mappingId: "b",
        digest: "reviewed",
        openingBalance: "250",
      }),
    );
    document.documentElement.dir = "ltr";
  },
);

const merrill = (status = "connected", canRemove = false): PlaidConnection => ({
  id: "merrill-connection",
  institution: "merrill_edge",
  product: "investments",
  status,
  canRemove,
  accounts: [],
});

it.each(["ltr", "rtl"])(
  "explains empty provider and local account lists and retries in %s",
  async (direction) => {
    document.documentElement.dir = direction;
    state.connections = [merrill()];
    state.invoke.mockResolvedValueOnce({ accounts: [], totalAccounts: 0 }).mockResolvedValueOnce({
      accounts: [
        { id: "external", name: "Synthetic brokerage", type: "investment", currency: "USD" },
      ],
      totalAccounts: 1,
    });
    show();
    fireEvent.click(screen.getByRole("button", { name: messages.plaid.map }));
    expect(await screen.findByText(messages.plaid.noProviderAccounts)).toBeVisible();
    expect(
      screen.getByText(messages.plaid.providerAccountCount.replace("{{total}}", "0")),
    ).toBeVisible();
    expect(screen.getByRole("combobox", { name: messages.plaid.externalAccount })).toBeDisabled();
    expect(screen.getByRole("combobox", { name: messages.plaid.localAccount })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Save" })).toBeDisabled();
    fireEvent.click(screen.getByRole("button", { name: "Retry" }));
    await waitFor(() =>
      expect(screen.getByRole("option", { name: "Synthetic brokerage" })).toBeVisible(),
    );
    expect(screen.queryByText(messages.plaid.noProviderAccounts)).not.toBeInTheDocument();
    fireEvent.change(screen.getByRole("combobox", { name: messages.plaid.externalAccount }), {
      target: { value: "external" },
    });
    expect(screen.getByRole("radio", { name: messages.plaid.createLocalAccount })).toBeChecked();
    fireEvent.click(screen.getByRole("radio", { name: messages.plaid.linkExistingAccount }));
    expect(screen.getByText(messages.plaid.noLocalAccounts)).toBeVisible();
    expect(screen.getByRole("button", { name: messages.plaid.newAccount })).toBeEnabled();
    expect(state.invoke).toHaveBeenCalledTimes(2);
    expect(state.invoke).toHaveBeenLastCalledWith({
      operation: "accounts",
      connectionId: "merrill-connection",
    });
    document.documentElement.dir = "ltr";
  },
);

it.each([2, undefined])(
  "explains unsupported accounts without claiming Plaid returned none (%s)",
  async (totalAccounts) => {
    state.connections = [merrill()];
    state.invoke.mockResolvedValue({
      accounts: [],
      totalAccounts,
      accountTypes: totalAccounts === undefined ? undefined : ["depository"],
    });
    show();
    fireEvent.click(screen.getByRole("button", { name: messages.plaid.map }));
    expect(await screen.findByText(messages.plaid.noEligibleAccounts)).toBeVisible();
    expect(screen.queryByText(messages.plaid.noProviderAccounts)).not.toBeInTheDocument();
    if (totalAccounts !== undefined) {
      expect(
        screen.getByText(messages.plaid.providerAccountCount.replace("{{total}}", "2")),
      ).toBeVisible();
      expect(
        screen.getByText(messages.plaid.providerAccountTypes.replace("{{types}}", "depository")),
      ).toBeVisible();
    }
  },
);

it.each(["investment", "depository"])(
  "maps a returned Merrill %s account only after local selection and self-directed confirmation",
  async (type) => {
    state.connections = [merrill()];
    state.invoke.mockImplementation(async (action) =>
      action.operation === "accounts"
        ? {
            accounts: [{ id: "external", name: "Synthetic brokerage", type, currency: "USD" }],
            totalAccounts: 1,
          }
        : null,
    );
    const local: Account = {
      id: "local",
      name: "Local Merrill",
      currency: "USD",
      accountType: "SECURITIES",
      trackingMode: "HOLDINGS",
      isArchived: false,
      isActive: true,
      isDefault: false,
      createdAt: new Date("2026-11-01"),
      updatedAt: new Date("2026-11-01"),
    };
    show("/settings/accounts", [
      local,
      {
        ...local,
        id: "cash",
        name: "Local bank",
        accountType: "CASH",
        trackingMode: "TRANSACTIONS",
      },
    ]);
    fireEvent.click(screen.getByRole("button", { name: messages.plaid.map }));
    await screen.findByRole("option", { name: "Synthetic brokerage" });
    fireEvent.change(screen.getByRole("combobox", { name: messages.plaid.externalAccount }), {
      target: { value: "external" },
    });
    expect(screen.getByRole("option", { name: "Local Merrill" })).toBeVisible();
    expect(screen.queryByRole("option", { name: "Local bank" })).not.toBeInTheDocument();
    fireEvent.change(screen.getByRole("combobox", { name: messages.plaid.localAccount }), {
      target: { value: "local" },
    });
    expect(screen.getByRole("button", { name: "Save" })).toBeDisabled();
    fireEvent.click(screen.getByRole("checkbox", { name: messages.plaid.merrillConfirm }));
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() =>
      expect(state.invoke).toHaveBeenCalledWith({
        operation: "map",
        connectionId: "merrill-connection",
        externalId: "external",
        accountId: "local",
        merrillConfirmed: true,
        cutoverDate: undefined,
      }),
    );
  },
);

it("shows a rejected account's reported type and subtype", async () => {
  state.connections = [merrill()];
  state.invoke.mockResolvedValue({
    accounts: [],
    totalAccounts: 1,
    accountTypes: ["depository"],
    accountClassifications: [{ type: "depository", subtype: "checking" }],
  });
  show();
  fireEvent.click(screen.getByRole("button", { name: messages.plaid.map }));
  expect(
    await screen.findByText(
      messages.plaid.providerAccountTypes.replace("{{types}}", "depository (checking)"),
    ),
  ).toBeVisible();
  expect(screen.getByRole("button", { name: "Save" })).toBeDisabled();
});

it.each(["ltr", "rtl"])(
  "confirms removal of an unused disconnected connection in %s",
  async (direction) => {
    document.documentElement.dir = direction;
    state.connections = [merrill("disconnected", true)];
    state.invoke.mockImplementation(async () => {
      state.connections = [];
      return null;
    });
    show();
    fireEvent.click(screen.getByRole("button", { name: messages.plaid.removeConnection }));
    expect(screen.getByText(messages.plaid.removeConnectionConfirm)).toBeVisible();
    expect(state.invoke).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(screen.queryByText(messages.plaid.removeConnectionConfirm)).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: messages.plaid.removeConnection }));
    fireEvent.click(screen.getByRole("button", { name: "Confirm" }));
    await waitFor(() =>
      expect(state.invoke).toHaveBeenCalledExactlyOnceWith({
        operation: "remove_connection",
        connectionId: "merrill-connection",
      }),
    );
    await waitFor(() => expect(screen.queryByText("Merrill Edge")).not.toBeInTheDocument());
    expect(state.invalidate).toHaveBeenCalledWith({ queryKey: ["direct-plaid"] });
    document.documentElement.dir = "ltr";
  },
);

it.each([
  ["connected", true],
  ["pending_revocation", false],
  ["disconnected", false],
])("does not offer removal for unsafe connections (%s)", (status, canRemove) => {
  state.connections = [merrill(status as string, canRemove as boolean)];
  show();
  expect(
    screen.queryByRole("button", { name: messages.plaid.removeConnection }),
  ).not.toBeInTheDocument();
  expect(state.invoke).not.toHaveBeenCalled();
});

it("keeps a disconnected connection visible if removal fails", async () => {
  state.connections = [merrill("disconnected", true)];
  state.invoke.mockRejectedValue(new Error("Plaid state changed; reload before continuing"));
  show();
  fireEvent.click(screen.getByRole("button", { name: messages.plaid.removeConnection }));
  fireEvent.click(screen.getByRole("button", { name: "Confirm" }));
  expect(await screen.findByRole("alert")).toHaveTextContent("state changed");
  expect(screen.getByText("Merrill Edge")).toBeVisible();
});

it.each(["ltr", "rtl"])(
  "maps a shared Merrill card to credit-card accounts and routes reviews by account product in %s",
  async (direction) => {
    document.documentElement.dir = direction;
    state.connections = [
      {
        id: "merrill",
        institution: "merrill_edge",
        product: "investments",
        status: "connected",
        accounts: [
          {
            id: "brokerage",
            accountId: "brokerage-local",
            product: "investments",
            status: "initial_review",
          },
          {
            id: "card-mapping",
            accountId: "mapped-card",
            product: "transactions",
            status: "initial_review",
          },
        ],
      },
    ];
    state.invoke.mockImplementation(async (action) => {
      if (action.operation === "accounts")
        return {
          accounts: [
            {
              id: "investment",
              name: "Synthetic brokerage",
              type: "investment",
              currency: "USD",
              product: "investments",
            },
            {
              id: "card",
              name: "Synthetic card",
              type: "credit",
              currency: "USD",
              product: "transactions",
            },
          ],
        };
      if (action.operation === "preview")
        throw new Error("Plaid connection needs reconnection or operator attention");
      return null;
    });
    const accounts = [
      {
        id: "local-card",
        name: "Local card",
        accountType: "CREDIT_CARD",
        trackingMode: "TRANSACTIONS",
        currency: "USD",
      },
      {
        id: "local-brokerage",
        name: "Local brokerage",
        accountType: "SECURITIES",
        trackingMode: "HOLDINGS",
        currency: "USD",
      },
    ] as Account[];
    show("/settings/accounts", accounts);
    expect(screen.getAllByRole("button", { name: messages.plaid.importHoldings })).toHaveLength(1);
    expect(screen.getAllByRole("button", { name: messages.plaid.history })).toHaveLength(1);
    fireEvent.click(screen.getByRole("button", { name: messages.plaid.review }));
    await screen.findByRole("alert");
    expect(state.invoke).toHaveBeenCalledWith({ operation: "preview", mappingId: "card-mapping" });
    fireEvent.click(screen.getByRole("button", { name: messages.plaid.map }));
    const external = await screen.findByRole("combobox", { name: messages.plaid.externalAccount });
    fireEvent.change(external, { target: { value: "card" } });
    const local = screen.getByRole("combobox", { name: messages.plaid.localAccount });
    expect(local).toHaveTextContent("Local card");
    expect(local).not.toHaveTextContent("Local brokerage");
    expect(screen.getByLabelText((text) => text.startsWith(messages.plaid.cutover))).toBeVisible();
    fireEvent.change(local, { target: { value: "local-card" } });
    expect(
      screen.queryByRole("checkbox", { name: messages.plaid.merrillConfirm }),
    ).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() =>
      expect(state.invoke).toHaveBeenCalledWith({
        operation: "map",
        connectionId: "merrill",
        externalId: "card",
        accountId: "local-card",
        merrillConfirmed: false,
        cutoverDate: undefined,
      }),
    );
    document.documentElement.dir = "ltr";
  },
);

it.each(["ltr", "rtl"])(
  "creates and links a new account without a cutover date in %s",
  async (direction) => {
    document.documentElement.dir = direction;
    state.connections = [
      {
        id: "bank",
        institution: "chase",
        product: "transactions",
        status: "connected",
        accounts: [],
      },
    ];
    state.invoke.mockImplementation(async (action) =>
      action.operation === "accounts"
        ? {
            accounts: [
              {
                id: "external",
                name: "Synthetic checking",
                mask: "1234",
                type: "depository",
                product: "transactions",
                currency: "USD",
              },
            ],
          }
        : { accountId: "created", mappingId: "mapping", created: true },
    );
    show();
    fireEvent.click(screen.getByRole("button", { name: messages.plaid.map }));
    const external = await screen.findByRole("combobox", { name: messages.plaid.externalAccount });
    fireEvent.change(external, { target: { value: "external" } });
    expect(screen.getByRole("radio", { name: messages.plaid.createLocalAccount })).toBeChecked();
    const name = screen.getByRole("textbox");
    expect(name).toHaveValue("Synthetic checking •1234");
    expect(
      screen.queryByLabelText((text) => text.startsWith(messages.plaid.cutover)),
    ).not.toBeInTheDocument();
    fireEvent.change(name, { target: { value: " " } });
    expect(screen.getByRole("button", { name: messages.plaid.createAndLink })).toBeDisabled();
    fireEvent.change(name, { target: { value: "My checking" } });
    fireEvent.click(screen.getByRole("button", { name: messages.plaid.createAndLink }));
    await waitFor(() =>
      expect(state.invoke).toHaveBeenCalledWith({
        operation: "create_and_map",
        connectionId: "bank",
        externalId: "external",
        name: "My checking",
        merrillConfirmed: false,
      }),
    );
    await waitFor(() => expect(screen.queryByRole("textbox")).not.toBeInTheDocument());
    expect(state.invalidate).toHaveBeenCalledWith(
      expect.objectContaining({ predicate: expect.any(Function) }),
    );
    document.documentElement.dir = "ltr";
  },
);

it("keeps a failed creation retryable and reports an already-linked retry without another local create", async () => {
  state.connections = [merrill()];
  state.invoke
    .mockResolvedValueOnce({
      accounts: [
        {
          id: "external",
          name: "Synthetic card",
          type: "credit",
          product: "transactions",
          currency: "USD",
        },
      ],
    })
    .mockRejectedValueOnce(new Error("Connection lost"))
    .mockResolvedValueOnce({ accountId: "existing", mappingId: "same", created: false });
  show();
  fireEvent.click(screen.getByRole("button", { name: messages.plaid.map }));
  fireEvent.change(await screen.findByRole("combobox", { name: messages.plaid.externalAccount }), {
    target: { value: "external" },
  });
  expect(
    screen.queryByRole("checkbox", { name: messages.plaid.merrillConfirm }),
  ).not.toBeInTheDocument();
  fireEvent.click(screen.getByRole("button", { name: messages.plaid.createAndLink }));
  expect(await screen.findByRole("alert")).toHaveTextContent("Connection lost");
  expect(screen.getByRole("textbox")).toHaveValue("Synthetic card");
  fireEvent.click(screen.getByRole("button", { name: messages.plaid.createAndLink }));
  expect(await screen.findByText(messages.plaid.alreadyLinked)).toBeVisible();
  expect(
    state.invoke.mock.calls.filter(([action]) => action.operation === "create_and_map"),
  ).toHaveLength(2);
});

it("blocks creation of an already-linked external account", async () => {
  state.connections = [merrill()];
  state.invoke.mockResolvedValue({
    accounts: [
      {
        id: "external",
        name: "Synthetic",
        type: "investment",
        currency: "USD",
        mappingId: "mapped",
      },
    ],
  });
  show();
  fireEvent.click(screen.getByRole("button", { name: messages.plaid.map }));
  fireEvent.change(await screen.findByRole("combobox", { name: messages.plaid.externalAccount }), {
    target: { value: "external" },
  });
  expect(screen.getByText(messages.plaid.alreadyLinked)).toBeVisible();
  expect(screen.queryByRole("textbox")).not.toBeInTheDocument();
  expect(screen.getByRole("button", { name: messages.plaid.createAndLink })).toBeDisabled();
});

it("preserves brokerage confirmation and clears an existing-account cutover when switching to new", async () => {
  state.connections = [merrill()];
  state.invoke.mockResolvedValue({
    accounts: [
      {
        id: "card",
        name: "Synthetic card",
        type: "credit",
        currency: "USD",
        product: "transactions",
      },
      {
        id: "brokerage",
        name: "Synthetic brokerage",
        type: "depository",
        currency: "USD",
        product: "investments",
      },
    ],
  });
  show("/settings/accounts", [
    {
      id: "local",
      name: "Local card",
      accountType: "CREDIT_CARD",
      currency: "USD",
      trackingMode: "TRANSACTIONS",
    },
  ] as Account[]);
  fireEvent.click(screen.getByRole("button", { name: messages.plaid.map }));
  const external = await screen.findByRole("combobox", { name: messages.plaid.externalAccount });
  fireEvent.change(external, { target: { value: "card" } });
  expect(screen.getByRole("radio", { name: messages.plaid.linkExistingAccount })).toBeChecked();
  fireEvent.change(
    screen.getByLabelText((text) => text.startsWith(messages.plaid.cutover)),
    { target: { value: "2026-11-01" } },
  );
  fireEvent.click(screen.getByRole("radio", { name: messages.plaid.createLocalAccount }));
  expect(
    screen.queryByLabelText((text) => text.startsWith(messages.plaid.cutover)),
  ).not.toBeInTheDocument();
  fireEvent.change(external, { target: { value: "brokerage" } });
  expect(screen.getByRole("button", { name: messages.plaid.createAndLink })).toBeDisabled();
  fireEvent.click(screen.getByRole("checkbox", { name: messages.plaid.merrillConfirm }));
  fireEvent.click(screen.getByRole("button", { name: messages.plaid.createAndLink }));
  await waitFor(() =>
    expect(state.invoke).toHaveBeenCalledWith({
      operation: "create_and_map",
      connectionId: "merrill-connection",
      externalId: "brokerage",
      name: "Synthetic brokerage",
      merrillConfirmed: true,
    }),
  );
});

it.each(["ltr", "rtl"])(
  "marks eligible sweep holdings as cash during holdings review and invalidates portfolio queries in %s",
  async (direction) => {
    document.documentElement.dir = direction;
    state.connections = [
      {
        id: "merrill-connection",
        institution: "merrill_edge",
        product: "investments",
        status: "connected",
        accounts: [
          {
            id: "brokerage",
            accountId: "local-merrill",
            product: "investments",
            status: "review",
          },
        ],
      },
    ];
    state.invoke.mockImplementation(async (action) => {
      if (action.operation === "preview_holdings") {
        return {
          mappingId: "brokerage",
          digest: "holdings-1",
          currency: "USD",
          holdings: [
            {
              security_id: "sec-voo",
              quantity: 10,
              institution_value: 5200,
              cost_basis: 4500,
            },
            {
              security_id: "sec-deposit",
              quantity: 6226.48,
              institution_value: 6226.48,
              cost_basis: null,
            },
          ],
          securities: [
            {
              security_id: "sec-voo",
              name: "Vanguard S&P 500 ETF",
              ticker_symbol: "VOO",
              type: "etf",
            },
            {
              security_id: "sec-deposit",
              name: "Ml Direct Deposit Progrm",
              ticker_symbol: "DEPOSIT",
              type: "other",
            },
            {
              security_id: "sec-prior-sweep",
              name: "Prior Zeroed Sweep",
              ticker_symbol: "SWEEP",
              type: "other",
            },
          ],
          cashSecurityIds: ["sec-prior-sweep"],
          eligibleCashSecurityIds: ["sec-deposit", "sec-prior-sweep"],
          transactionCount: 0,
          fetchedAt: "2026-11-25T08:00:00Z",
        };
      }
      return null;
    });

    show();
    fireEvent.click(screen.getByRole("button", { name: messages.plaid.importHoldings }));
    const depositCheckbox = await screen.findByRole("checkbox", {
      name: `${messages.plaid.countAsCash} Ml Direct Deposit Progrm`,
    });
    const priorSweepCheckbox = screen.getByRole("checkbox", {
      name: `${messages.plaid.countAsCash} Prior Zeroed Sweep`,
    });
    expect(depositCheckbox).not.toBeChecked();
    expect(priorSweepCheckbox).toBeChecked();
    expect(
      screen.queryByRole("checkbox", {
        name: `${messages.plaid.countAsCash} Vanguard S&P 500 ETF`,
      }),
    ).not.toBeInTheDocument();

    expect(screen.getByText(messages.plaid.countAsCashHelp)).toBeVisible();
    fireEvent.click(depositCheckbox);
    expect(depositCheckbox).toBeChecked();
    fireEvent.click(screen.getByRole("button", { name: "Confirm" }));

    await waitFor(() =>
      expect(state.invoke).toHaveBeenCalledWith({
        operation: "approve_holdings",
        mappingId: "brokerage",
        digest: "holdings-1",
        allowOpaque: false,
        cashSecurityIds: ["sec-prior-sweep", "sec-deposit"],
      }),
    );
    expect(state.invalidate).toHaveBeenCalledWith(
      expect.objectContaining({ predicate: expect.any(Function) }),
    );
    document.documentElement.dir = "ltr";
  },
);

it.each(["ltr", "rtl"])(
  "lets owners clear saved cash approvals that are no longer eligible in %s",
  async (direction) => {
    document.documentElement.dir = direction;
    state.connections = [
      {
        id: "merrill-connection",
        institution: "merrill_edge",
        product: "investments",
        status: "connected",
        accounts: [
          {
            id: "brokerage",
            accountId: "local-merrill",
            product: "investments",
            status: "review",
          },
        ],
      },
    ];
    state.invoke.mockImplementation(async (action) => {
      if (action.operation === "preview_holdings") {
        return {
          mappingId: "brokerage",
          digest: "holdings-2",
          currency: "USD",
          holdings: [
            {
              security_id: "sec-deposit",
              quantity: 100,
              institution_value: 105,
              cost_basis: null,
            },
            {
              security_id: "sec-still-par",
              quantity: 50,
              institution_value: 50,
              cost_basis: null,
            },
          ],
          securities: [
            {
              security_id: "sec-deposit",
              name: "Drifted Sweep",
              ticker_symbol: "DRIFT",
              type: "other",
            },
            {
              security_id: "sec-still-par",
              name: "Valid Sweep",
              ticker_symbol: "PAR",
              type: "other",
            },
          ],
          cashSecurityIds: ["sec-deposit", "sec-still-par", "sec-gone-ineligible"],
          eligibleCashSecurityIds: ["sec-still-par"],
          transactionCount: 0,
          fetchedAt: "2026-11-26T08:00:00Z",
        };
      }
      return null;
    });

    show();
    fireEvent.click(screen.getByRole("button", { name: messages.plaid.importHoldings }));

    const removeDrifted = await screen.findByRole("button", {
      name: `${messages.plaid.removeCashClassification} Drifted Sweep`,
    });
    const removeGone = screen.getByRole("button", {
      name: `${messages.plaid.removeCashClassification} sec-gone-ineligible`,
    });
    const validCheckbox = screen.getByRole("checkbox", {
      name: `${messages.plaid.countAsCash} Valid Sweep`,
    });
    expect(validCheckbox).toBeChecked();

    fireEvent.click(removeDrifted);
    fireEvent.click(removeGone);
    expect(
      screen.queryByRole("button", {
        name: `${messages.plaid.removeCashClassification} Drifted Sweep`,
      }),
    ).not.toBeInTheDocument();
    expect(screen.queryByText(/sec-gone-ineligible/)).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Confirm" }));

    await waitFor(() =>
      expect(state.invoke).toHaveBeenCalledWith({
        operation: "approve_holdings",
        mappingId: "brokerage",
        digest: "holdings-2",
        allowOpaque: false,
        cashSecurityIds: ["sec-still-par"],
      }),
    );
    document.documentElement.dir = "ltr";
  },
);

it.each(["ltr", "rtl"])(
  "connects Charles Schwab and maps brokerage and shared Schwab Bank accounts without Merrill confirmation in %s",
  async (direction) => {
    document.documentElement.dir = direction;
    state.institutions = ["merrill_edge", "schwab"];
    state.connections = [
      {
        id: "schwab-connection",
        institution: "schwab",
        product: "investments",
        status: "connected",
        accounts: [
          {
            id: "schwab-brokerage-map",
            accountId: "local-brokerage",
            product: "investments",
            status: "initial_review",
          },
          {
            id: "schwab-checking-map",
            accountId: "local-checking",
            product: "transactions",
            status: "initial_review",
          },
        ],
      },
    ];
    state.invoke.mockImplementation(async (action) =>
      action.operation === "accounts"
        ? {
            accounts: [
              {
                id: "schwab-brokerage",
                name: "Schwab One Brokerage",
                mask: "6083",
                type: "investment",
                product: "investments",
                currency: "USD",
              },
              {
                id: "schwab-checking",
                name: "Schwab Bank Investor Checking",
                mask: "4321",
                type: "depository",
                product: "transactions",
                currency: "USD",
              },
            ],
            totalAccounts: 2,
          }
        : { accountId: "created", mappingId: "created-map", created: true },
    );
    const localBrokerage: Account = {
      id: "local-brokerage",
      name: "Existing Schwab Brokerage",
      currency: "USD",
      accountType: "SECURITIES",
      trackingMode: "HOLDINGS",
      isArchived: false,
      isActive: true,
      isDefault: false,
      createdAt: new Date("2026-11-01"),
      updatedAt: new Date("2026-11-01"),
    };
    const localChecking: Account = {
      ...localBrokerage,
      id: "local-checking",
      name: "Existing Schwab Checking",
      accountType: "CASH",
      trackingMode: "TRANSACTIONS",
    };
    show("/settings/accounts", [localBrokerage, localChecking]);

    // Institution dropdown renders Charles Schwab and maps product-specific actions.
    expect(screen.getByRole("option", { name: "Charles Schwab" })).toBeVisible();
    expect(screen.getByText("Charles Schwab", { selector: "strong" })).toBeVisible();
    expect(screen.getByRole("button", { name: messages.plaid.importHoldings })).toBeVisible();
    expect(screen.getByRole("button", { name: messages.plaid.investments.enable })).toBeVisible();
    expect(screen.getByRole("button", { name: messages.plaid.review })).toBeVisible();

    fireEvent.click(screen.getByRole("button", { name: messages.plaid.map }));
    const external = await screen.findByRole("combobox", { name: messages.plaid.externalAccount });

    // Brokerage links to SECURITIES/HOLDINGS without Merrill's self-directed checkbox or cutover.
    fireEvent.change(external, { target: { value: "schwab-brokerage" } });
    expect(
      screen.queryByRole("checkbox", { name: messages.plaid.merrillConfirm }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByLabelText((text) => text.startsWith(messages.plaid.cutover)),
    ).not.toBeInTheDocument();
    expect(screen.getByRole("option", { name: "Existing Schwab Brokerage" })).toBeVisible();
    expect(
      screen.queryByRole("option", { name: "Existing Schwab Checking" }),
    ).not.toBeInTheDocument();
    fireEvent.change(screen.getByRole("combobox", { name: messages.plaid.localAccount }), {
      target: { value: "local-brokerage" },
    });
    expect(screen.getByRole("button", { name: "Save" })).toBeEnabled();

    // Shared Schwab Bank checking filters to CASH/TRANSACTIONS, offers cutover, and creates CASH account.
    fireEvent.change(external, { target: { value: "schwab-checking" } });
    expect(screen.getByRole("option", { name: "Existing Schwab Checking" })).toBeVisible();
    expect(
      screen.queryByRole("option", { name: "Existing Schwab Brokerage" }),
    ).not.toBeInTheDocument();
    expect(screen.getByLabelText((text) => text.startsWith(messages.plaid.cutover))).toBeVisible();
    fireEvent.click(screen.getByRole("radio", { name: messages.plaid.createLocalAccount }));
    fireEvent.click(screen.getByRole("button", { name: messages.plaid.createAndLink }));
    await waitFor(() =>
      expect(state.invoke).toHaveBeenCalledWith({
        operation: "create_and_map",
        connectionId: "schwab-connection",
        externalId: "schwab-checking",
        name: "Schwab Bank Investor Checking •4321",
        merrillConfirmed: false,
      }),
    );
    document.documentElement.dir = "ltr";
  },
);
