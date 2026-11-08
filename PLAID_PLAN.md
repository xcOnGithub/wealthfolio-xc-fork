# Shared Merrill Item: credit-card support

## Goal and decisions

Make credit cards already shared on a Merrill Item selectable and importable
alongside brokerage accounts. Reuse the existing Item, token, connection, and
account-scoped transaction streams. Do not create another production Item. The
user chose direct tools rather than the guided workflow.

Keep the existing `plaid_action` operations and web-only runtime boundary; add
optional product metadata to account DTOs, with the connection product as the
frontend fallback. No new endpoint, database column, or credential store.

## Architecture impact

The institution product previously controlled discovery, local-account
validation, review, scheduled dispatch, and writer validation. For Merrill,
derive the account product from the existing persisted `credit_card` flag. Make
that classification immutable with mapping ownership, so metadata updates cannot
switch a mapping's sync stream or reuse its cursor for another product. Existing
brokerage mappings and their identifiers remain unchanged; no data migration is
required.

The Rust service owns eligibility and dispatch; SQLite still validates
compatible local accounts inside the writer transaction. Cards use
CREDIT_CARD/TRANSACTIONS; brokerage and cash-management accounts retain
SECURITIES/HOLDINGS. Brokerage-only self-directed attestation remains required
for investment mappings.

Reuse existing review/staging, owner-edit precedence, atomic ledger/checkpoint
commits, and manual/daily scheduling. No new worker, lock, retry loop, webhook
receiver, refresh add-on, or financial-data destination is introduced.

Merrill Hosted Link adds Transactions consent without initializing that product
until a card is actually previewed. Reconnect passes the saved access token and
requests additional consent on the same Item. ADDITIONAL_CONSENT_REQUIRED uses
the existing reconnect/attention error. Product initialization requests 730 days
on the account-scoped sync stream. No token exchange occurs during reconnect.

## Guidance applied

- GUIDE-001/002/003/004/006: existing server-only encrypted token and
  configuration boundaries remain; no real credentials or financial rows are
  committed/logged.
- GUIDE-005: checked installed Plaid SDK request types for consent and sync
  options.
- ITEM-001/003, TXN-020: reuse the same Item and use update mode for consent.
- TXN-001/002/004/005/007/009/019/022: retain account-scoped cursors,
  terminal-page persistence, removals, pagination restart, currency/sign
  handling, and explicit review. Route credit accounts to Transactions, never
  Investments endpoints.
- INV-001/010: preserve existing brokerage holdings/history behavior.
- OAUTH-001/002/005: retain the production redirect for new and update flows.
- GUIDE-014: final acceptance evidence must match the checked revision.
  Synthetic tests do not establish live Merrill data accuracy or a successful
  owner consent.

## Verification

- New regression fails when the original eligibility filter is restored: the
  shared credit card is absent. The runner restores the exact modified source.
- Synthetic purchase/refund scenarios exercise discovery, compatible mapping,
  negative liability balance, explicit approval, ledger commit, per-account
  cursor resumption, and preservation of the brokerage mapping on one
  connection.
- Existing CMA import, authentication/admission, pagination, removal,
  owner-edit, atomic rollback, disabled-provider and profile/environment tests
  remain relevant.
- Hosted Link request tests check additional consent, update-mode token reuse,
  no new required product on reconnect, and safe consent-required errors.
- Frontend tests exercise product-specific review actions and local-account
  choices in both text directions; adapter parity, types, lint, and both bundles
  are checked.
- Required Rust checks: focused connect/storage suites, formatting,
  affected-crate Clippy with all targets/features, and compilation of both
  runtime consumers.

## Operator and owner tasks

- Verified: Dashboard grants US Production Transactions and Investments;
  production OAuth callback is registered. No production credentials need to be
  replaced.
- Needs human: local Quickstart uses `https://localhost:3000/`, but the newly
  restored Dashboard entry is `https://localhost:3000`. Direct Link-token
  validation accepts the latter and rejects the former. Add the exact
  trailing-slash URI at <https://dashboard.plaid.com/developers/api>. Production
  callback is unaffected.
- After reviewed deployment: open the existing Merrill connection, reconnect if
  additional product consent is required, map each card to a compatible local
  credit-card account, and review its first import. Do not choose another login.
- Pending live validation: owner verifies selected accounts and imported
  balances/ transactions privately after deployment. No production imports occur
  in tests.
- Deferred existing limitation: freshness uses the installation's existing
  manual/ daily scheduler, not Plaid readiness webhooks. This patch preserves
  that boundary; webhook-driven freshness and its acceptance checks are not
  claimed as implemented.
- Cannot verify via Dashboard tools: institution-specific consent completion and
  the current Item's readiness; only the owner's same-Item update and product
  fetch establish these. No new institution registration is introduced.
