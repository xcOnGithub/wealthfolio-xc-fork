# Invitation-only owner account

This private fork adds an optional email/password account for **one installation
owner**. It replaces the server-wide password when enabled. It does not add
multi-user financial-data isolation. The one invitation grants access to all
existing profiles and financial data in this installation. Keep it private.

## Configure before deploying

1. Back up the installation data and its existing master key. Keep the same
   `WF_SECRET_KEY` / `WF_SECRET_KEY_FILE`: authentication changes do not require
   changing the encryption key or moving financial data.
2. Generate one invitation on a trusted machine, outside the repository:

   ```sh
   node scripts/create-owner-invitation.mjs /absolute/private-directory
   ```

   Share `owner-invitation.txt` only with the owner. Load `owner-auth.env` into
   the container environment. The plaintext invitation must not be committed,
   baked into the Docker image, or sent to an email provider. The generator
   refuses to overwrite an existing invitation. Generating another code after
   signup does not allow another account: the server permanently closes signup
   once an owner exists.

3. Configure Resend with a verified sender/domain and set these variables
   through your deployment's secret configuration (never put real values in
   Git):

   ```dotenv
   WF_ACCOUNT_AUTH=true
   WF_SIGNUP_INVITE_HASH=<SHA-256 hex digest from owner-auth.env>
   WF_RESEND_API_KEY=<Resend API key>
   WF_AUTH_EMAIL_FROM=Portfolio <access@your-verified-domain.example>
   WF_CORS_ALLOW_ORIGINS=https://your-app.example
   WF_COOKIE_SECURE=true
   ```

   Outbound HTTPS to `api.resend.com` is needed for recovery. The sender must be
   authorized by Resend to deliver to the registered email. Without these
   settings account mode refuses to start. No emails are sent at signup.

4. Remove `WF_AUTH_PASSWORD_HASH` and OIDC settings. Account mode rejects those
   alternate login methods to prevent bypassing password resets. Keep existing
   authentication in place until the new container is fully configured.
5. Run **one server process/container per installation**. Persist the data root:
   `web-account.json` is an encrypted, atomically replaced account record stored
   next to the legacy database path selected by `WF_DATA_DIR` / `WF_DB_PATH`. It
   is separate from profile databases, so profile restores cannot reopen signup
   or revert the owner password. Include this file in installation backups along
   with the matching master key. Do not delete it to upgrade the app.

A bare or unconfigured build preserves upstream authentication behavior. Account
mode is enabled only with `WF_ACCOUNT_AUTH=true`. Use HTTPS in production.

## Owner flow

- Open `/auth/signup`, enter the invitation, email address, and a password of at
  least 12 characters (at most 1024 UTF-8 bytes). The invitation can be redeemed
  exactly once, including across server restarts. The email is trimmed and
  compared case-insensitively. Use an address you control and can receive mail
  at.
- After signup, sign in at `/auth/login` using that email and password.
- At `/auth/recover`, request a recovery code. The page always gives the same
  confirmation for registered and unregistered email addresses. Delivery
  failures are logged without provider responses, email addresses, or recovery
  codes.
- At `/auth/reset`, enter the email, emailed code, and a new password. A
  recovery code lasts 10 minutes, is single-use, and permits at most five failed
  attempts. Requests have a per-account 60-second cooldown; a new code replaces
  the previous code. Failed attempt counts and cooldowns survive restarts. Reset
  revokes all prior owner session cookies and bearer JWTs, including sliding
  refreshes.

Passwords use salted Argon2id hashes. Invitation and recovery codes use SHA-256
hashes and cryptographic randomness. Session cookies retain HttpOnly and
SameSite=Lax. Signup, sign-in, recovery, and reset share a per-peer-IP throttle;
password work is bounded across peers. Behind a reverse proxy, the peer IP may
be the proxy, so the throttle can be shared across clients. Session cookies from
legacy login are rejected when account mode is enabled. Downgrading to legacy
mode requires explicit server configuration and is not an account-recovery flow.

Recovery transmits only the destination email and recovery email content to
Resend. Portfolio information is not included. No email delivery or deployment
occurs merely by building this source.

## Architecture impact and validation

Web authentication gains persistent installation-owned credentials and an
outbound Resend call on recovery. The existing encrypted secret-store format,
JWT signer, cookie policy, profile access checks, and rate limiter are reused.
No portfolio schema migration or desktop authentication behavior is changed. New
state is necessary because an environment-variable password cannot store a
registered email, atomically consume an invitation, or persist a password reset.
There is no background delivery worker or retry queue.

Focused server tests cover persistence, invalid/reused invitations, concurrent
signup, case-insensitive sign-in, code expiry/attempt limits/reuse, password
replacement, rejection of legacy sessions, and invalidation of existing and
refreshed JWTs. Frontend tests cover the page flows, request payloads, and
hiding financial content until successful sign-in. Live Resend delivery requires
the operator's configured sender and API key and must be checked on staging.

Modified files are part of a private fork, modified in October 2026, under
AGPL-3.0. Preserve the original license and copyright notices. The new account
screens omit upstream branding; the rest of the app must also follow
`TRADEMARKS.md` before distributing a rebranded build.
