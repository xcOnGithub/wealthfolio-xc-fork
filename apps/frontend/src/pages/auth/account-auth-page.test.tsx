import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { AccountAuthRoutes } from "./account-auth-page";

const login = vi.fn();
const fetchMock = vi.fn<typeof fetch>();
vi.mock("@/context/auth-context", () => ({ useAuth: () => ({ login, clearError: vi.fn() }) }));
beforeEach(() => {
  login.mockReset();
  fetchMock.mockReset();
  vi.stubGlobal("fetch", fetchMock);
});
afterEach(() => vi.unstubAllGlobals());
function mount(path: string) {
  return render(
    <MemoryRouter initialEntries={[path]}>
      <AccountAuthRoutes />
    </MemoryRouter>,
  );
}
function fill(label: string, value: string) {
  fireEvent.change(screen.getByLabelText(label), { target: { value } });
}

it("signs in with the registered email and password", async () => {
  login.mockResolvedValue(undefined);
  mount("/auth/login");
  fill("Email address", "owner@example.com");
  fill("Password", "a long test password");
  fireEvent.click(screen.getByRole("button", { name: "Sign in" }));
  await waitFor(() =>
    expect(login).toHaveBeenCalledWith("a long test password", "owner@example.com"),
  );
});

it("submits the invitation and shows signup confirmation on the sign-in page", async () => {
  fetchMock.mockResolvedValue(new Response(null, { status: 201 }));
  mount("/auth/signup");
  fill("Email address", "owner@example.com");
  fill("Password", "a long test password");
  fill("Invitation code", "private-invite");
  fireEvent.click(screen.getByRole("button", { name: "Create account" }));
  expect(await screen.findByRole("status")).toHaveTextContent("Account created");
  const [url, options] = fetchMock.mock.calls[0];
  expect(url).toBe("/api/v1/auth/signup");
  expect(JSON.parse(options!.body as string)).toMatchObject({
    email: "owner@example.com",
    password: "a long test password",
    invitationCode: "private-invite",
  });
  expect(login).not.toHaveBeenCalled();
  expect(screen.queryByLabelText("Invitation code")).not.toBeInTheDocument();
});

it("requests recovery and lets the user open the reset page", async () => {
  fetchMock.mockResolvedValue(new Response(null, { status: 202 }));
  mount("/auth/recover");
  fill("Email address", "owner@example.com");
  fireEvent.click(screen.getByRole("button", { name: "Send recovery code" }));
  expect(await screen.findByRole("status")).toHaveTextContent("If this email is registered");
  expect(fetchMock.mock.calls[0][0]).toBe("/api/v1/auth/recover");
  fireEvent.click(screen.getByRole("link", { name: "I have a recovery code" }));
  expect(await screen.findByLabelText("Recovery code")).toBeInTheDocument();
});

it("resets the password using the emailed code and returns to sign-in", async () => {
  fetchMock.mockResolvedValue(new Response(null, { status: 204 }));
  mount("/auth/reset");
  fill("Email address", "owner@example.com");
  fill("Password", "replacement test password");
  fill("Recovery code", "test-recovery-code");
  fireEvent.click(screen.getByRole("button", { name: "Reset password" }));
  expect(await screen.findByRole("status")).toHaveTextContent("Password updated");
  expect(fetchMock.mock.calls[0][0]).toBe("/api/v1/auth/reset");
  expect(JSON.parse(fetchMock.mock.calls[0][1]!.body as string)).toMatchObject({
    email: "owner@example.com",
    code: "test-recovery-code",
    password: "replacement test password",
  });
  expect(login).not.toHaveBeenCalled();
});

it("blocks weak signup passwords without sending a request", async () => {
  mount("/auth/signup");
  fill("Email address", "owner@example.com");
  fill("Password", "short");
  fill("Invitation code", "private-invite");
  fireEvent.click(screen.getByRole("button", { name: "Create account" }));
  expect(await screen.findByRole("alert")).toHaveTextContent("at least 12 characters");
  expect(fetchMock).not.toHaveBeenCalled();
});

it("shows rate limiting without exposing server response details", async () => {
  fetchMock.mockResolvedValue(new Response("sensitive provider response", { status: 429 }));
  mount("/auth/recover");
  fill("Email address", "owner@example.com");
  fireEvent.click(screen.getByRole("button", { name: "Send recovery code" }));
  expect(await screen.findByRole("alert")).toHaveTextContent("Too many attempts");
  expect(screen.queryByText("sensitive provider response")).not.toBeInTheDocument();
});
