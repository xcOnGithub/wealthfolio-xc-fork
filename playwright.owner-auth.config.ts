import { defineConfig, devices } from "@playwright/test";
import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { resolve, join } from "node:path";
import { randomBytes, createHash } from "node:crypto";

// Match the isolated profiles suite: never read or write a developer installation.
const installation = mkdtempSync(join(tmpdir(), "portfolio-owner-auth-e2e-"));
const baseURL = "http://127.0.0.1:18389";
export default defineConfig({
  testDir: "./e2e/owner-auth",
  workers: 1,
  timeout: 60000,
  reporter: "list",
  outputDir: "/tmp/portfolio-owner-auth-e2e-results",
  use: {
    ...devices["Desktop Chrome"],
    ...(process.env.WF_E2E_CHROMIUM_PATH
      ? { launchOptions: { executablePath: process.env.WF_E2E_CHROMIUM_PATH } }
      : { channel: "chrome" }),
    baseURL,
    locale: "en-US",
    trace: "retain-on-failure",
  },
  webServer: {
    command: resolve("target/debug/wealthfolio-server"),
    cwd: installation,
    url: baseURL,
    reuseExistingServer: false,
    timeout: 60000,
    env: {
      WF_LISTEN_ADDR: "127.0.0.1:18389",
      WF_DB_PATH: join(installation, "app.db"),
      WF_DATA_DIR: installation,
      WF_STATIC_DIR: resolve("dist"),
      WF_SECRET_KEY: randomBytes(32).toString("base64"),
      WF_DB_REQUIRE_ENCRYPTION: "false",
      WF_AUTH_PASSWORD_HASH: "",
      WF_OIDC_ISSUER_URL: "",
      WF_OIDC_CLIENT_ID: "",
      WF_ACCOUNT_AUTH: "true",
      WF_SIGNUP_INVITE_HASH: createHash("sha256").update("e2e-only-invitation").digest("hex"),
      WF_RESEND_API_KEY: "test-only-not-a-real-key",
      WF_AUTH_EMAIL_FROM: "test@example.invalid",
      WF_CORS_ALLOW_ORIGINS: baseURL,
      WF_COOKIE_SECURE: "false",
      WF_MCP_ENABLED: "false",
      RUST_LOG: "warn",
    },
  },
});
