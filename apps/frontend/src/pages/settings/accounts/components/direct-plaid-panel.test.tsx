import { render, screen, waitFor, fireEvent, cleanup } from "@/test/render";
import { MemoryRouter } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { DirectPlaidPanel } from "./direct-plaid-panel";
import type { Account } from "@/lib/types";
import type { PlaidConnection, PlaidPreview } from "@/adapters/shared/plaid";
import messages from "@/i18n/locales/en/connect.json";

const state = vi.hoisted(() => ({
  connections: [] as PlaidConnection[],
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
    data: { enabled: state.enabled, institutions: ["chase"], connections: state.connections },
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
    state.invoke.mockResolvedValue({ accounts: [], totalAccounts });
    show();
    fireEvent.click(screen.getByRole("button", { name: messages.plaid.map }));
    expect(await screen.findByText(messages.plaid.noEligibleAccounts)).toBeVisible();
    expect(screen.queryByText(messages.plaid.noProviderAccounts)).not.toBeInTheDocument();
  },
);

it("maps a returned Merrill investment account only after local selection and self-directed confirmation", async () => {
  state.connections = [merrill()];
  state.invoke.mockImplementation(async (action) =>
    action.operation === "accounts"
      ? {
          accounts: [
            { id: "external", name: "Synthetic brokerage", type: "investment", currency: "USD" },
          ],
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
    balance: 0,
    createdAt: new Date("2026-11-01"),
    updatedAt: new Date("2026-11-01"),
  };
  show("/settings/accounts", [
    local,
    { ...local, id: "cash", name: "Local bank", accountType: "CASH", trackingMode: "TRANSACTIONS" },
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
