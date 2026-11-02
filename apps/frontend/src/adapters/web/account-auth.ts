/** Public, same-origin endpoints. Session credentials remain in HttpOnly cookies. */
export async function submitAccountAuth(
  action: "signup" | "recover" | "reset",
  body: { email: string; password?: string; invitationCode?: string; code?: string },
): Promise<void> {
  const response = await fetch(`/api/v1/auth/${action}`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    credentials: "same-origin",
    body: JSON.stringify(body),
  });
  if (!response.ok) throw new Error(response.status === 429 ? "rateLimited" : "failed");
}
