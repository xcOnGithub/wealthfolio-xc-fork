import { beforeEach, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@/test/render";
import { FamilySettingsPanel } from "./family-settings";
import type { FamilySnapshot } from "@/features/family/types";

const defaults: FamilySnapshot = {
  settings: {
    enabled: true,
    largeChargeAmount: 1000,
    unusualMultiplier: 3,
    unusualMinimum: 100,
    budgetThreshold: 0.8,
    consentNoticeDays: 14,
    staleValuationDays: 90,
    billNoticeDays: 30,
    emailAlerts: false,
    emailDigest: false,
    webhookAlerts: false,
    webhookDigest: false,
    emailFrom: "",
    recipients: [],
    bills: [],
  },
  configRevision: 0,
  items: [],
  digests: {},
  deliveries: [],
  lastEvaluated: null,
  warnings: [],
  resendConfigured: true,
  webhookConfigured: true,
  webhookTokenConfigured: true,
};
const save = vi.fn();
beforeEach(() => {
  vi.clearAllMocks();
  save.mockResolvedValue({});
});
function open() {
  render(<FamilySettingsPanel snapshot={defaults} currency="USD" busy={false} onSave={save} />);
}
it("starts all external channels off and never displays saved credentials", () => {
  open();
  for (const name of [
    "Email new alerts",
    "Email monthly digest",
    "Webhook new alerts",
    "Webhook monthly digest",
  ])
    expect(screen.getByRole("checkbox", { name })).not.toBeChecked();
  expect(screen.getByLabelText(/Resend API key/)).toHaveValue("");
  expect(screen.getByLabelText(/Public HTTPS webhook/)).toHaveValue("");
});
it("converts percentage settings and sends new credentials once, then clears them", async () => {
  open();
  fireEvent.change(screen.getByLabelText("Budget alert at (%)"), { target: { value: "90" } });
  fireEvent.change(screen.getByLabelText(/Resend API key/), {
    target: { value: "synthetic-test-key" },
  });
  fireEvent.click(screen.getByRole("button", { name: "Save settings" }));
  await waitFor(() => expect(save).toHaveBeenCalledOnce());
  expect(save).toHaveBeenCalledWith({
    settings: { ...defaults.settings, budgetThreshold: 0.9 },
    resendApiKey: "synthetic-test-key",
  });
  await waitFor(() => expect(screen.getByLabelText(/Resend API key/)).toHaveValue(""));
  expect(screen.getByText("Settings saved.")).toBeVisible();
});
it("rejects invalid alert thresholds before saving", async () => {
  open();
  fireEvent.change(screen.getByLabelText("Unusual charge multiplier"), { target: { value: "1" } });
  fireEvent.click(screen.getByRole("button", { name: "Save settings" }));
  await screen.findByRole("alert");
  expect(save).not.toHaveBeenCalled();
});
it("keeps unsaved credentials on an error without exposing the provider error", async () => {
  save.mockRejectedValueOnce(new Error("private provider detail"));
  open();
  fireEvent.change(screen.getByLabelText(/Resend API key/), {
    target: { value: "synthetic-test-key" },
  });
  fireEvent.click(screen.getByRole("button", { name: "Save settings" }));
  await screen.findByText(/Settings could not be saved/);
  expect(screen.queryByText("private provider detail")).not.toBeInTheDocument();
  expect(screen.getByLabelText(/Resend API key/)).toHaveValue("synthetic-test-key");
});
