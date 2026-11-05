# Direct Plaid implementation plan

## Baseline and scope

The owner reports that the connectivity tests passed in a separate private
session. Accept that result as the feasibility gate; do not copy account data,
credentials, or screenshots into this repository. This does not establish that
the production importer is correct; that requires the validation below.

Support US accounts at Chase, Bank of America, U.S. Bank, Amex credit cards,
Merrill Edge, Charles Schwab, and Robinhood. Exclude Amex savings,
advisor-managed Merrill, trading, payments, and transfers initiated through the
API.

Target the self-hosted web application first. Use the owner's Plaid application,
with `transactions` for bank/card connections and `investments` for brokerage
connections. Preserve Wealthfolio Connect as a separate optional integration; do
not alter its entitlement checks. Daily sync and a manual Sync action are the
initial operating model. Do not promise instantaneous provider updates.

## Architecture impact

Today, `crates/connect` calls the hosted Connect API; its broker orchestrator
and the server's four-hour scheduler depend on Connect
authentication/entitlement. The new path calls Plaid directly from Rust using
`wealthfolio-http`, with an explicit opt-in. Disabling it must result in zero
Plaid requests. Bank and investment data remain in the existing profile SQLite
database. Secrets use `SecretStore`, never browser storage or plaintext database
columns.

Reuse `broker_ingest::{BrokerSyncState, ImportRun}` and the existing activity,
account, snapshot, asset, and import-review services. Reuse the server's worker
ownership/shutdown lifecycle and broker sync guard where safe. Do not introduce
a queue service, external database, independent daemon, or generic retry layer.

Required additions and the failures they prevent:

- Provider client and typed records in `crates/connect/src/plaid/`: the current
  client targets hosted Connect and cannot authenticate to Plaid or interpret
  its cursor protocol. Keep provider conversion separate from HTTP handlers.
- Item/account ownership and connection metadata in a new SQLite migration:
  current broker state has no Item ownership or connection lifecycle. Reuse
  account-level `PlaidSyncCheckpoint` with Transactions Sync
  `options.account_id` and an independent cursor for each account. Persist Item
  ownership plus unique account mappings/consent. Prevent cross-profile imports
  and duplicate mapping.
- Provider credentials in reserved secret namespaces: the generic secrets API
  currently exposes unreserved keys to frontend callers. Reserve Plaid keys
  before storing any access token. Use profile scope and environment scope.
- A bounded per-profile sync operation under the existing guard. Scheduled and
  manual operations must not overlap or race disconnect/profile deletion. Add
  finer locking only if tracing proves the existing guard insufficient.

Thin authenticated Axum endpoints own request validation. Shared Rust services
own provider logic. Frontend calls go through adapters; web-only commands must
have an explicit unsupported native implementation and parity tests. Native
Plaid linking is deferred; existing native builds must still work.

## Delivery sequence

Implement in dependency order on a feature branch. Keep the feature off by
default throughout development. Open a PR with implementation status and actual
validation results; do not enable production credentials or deploy as part of
opening the PR.

**First implementation PR in this session:** step 1 only, plus reservation of
the Plaid secret namespace and its regression tests. This is a provider
foundation, not a usable account-sync feature: no routes, Link UI, financial
writes, credential provisioning, or scheduled network calls are enabled.
Subsequent PRs deliver steps 2–5 in order. Financial storage transaction changes
need their own review because existing repository methods each commit their own
transaction.

### 1. Provider boundary and safe configuration

- Add an allowlisted Sandbox/Production environment enum; reject unknown or
  incomplete enabled configuration. No arbitrary production base URL. Use
  verified TLS, a finite timeout, redirects disabled, and no implicit retries.
- Add Link-token creation (new and update mode), public-token exchange,
  accounts, Transactions Sync, investment holdings/transactions, and Item
  removal API calls. Use explicit products, US country code, and a configured
  registered HTTPS redirect URI. Do not request Auth, Identity, or money
  movement products.
- Keep global Plaid secret and per-Item access tokens backend-only. Redact
  Debug/errors and never propagate response bodies, request bodies, credentials,
  institution account names, or financial values into logs/API error messages.
- Convert errors to bounded categories: reconnect required, rate limited,
  provider unavailable, invalid configuration, and malformed response. Preserve
  safe diagnostic request IDs where useful; avoid provider error-message text.
- Test real HTTP request serialization against a local mock server, including no
  credentials in URLs, wrong environments, timeouts, redirects, malformed JSON,
  and response/error redaction. Tests use fictitious fixtures only.

### 2. Connection lifecycle, storage, and account selection

- Bind Link sessions to the authenticated account/profile, product, environment,
  and expiration; exchange only an outstanding session belonging to that caller.
  Use a random single-use session ID, validate OAuth continuation state, and
  disallow caller-supplied Plaid URLs. Handle exchange failure without blindly
  repeating a single-use public-token exchange.
- A random session ID alone does not prove public-token/session association.
  Resolve completed results through the server-owned Link token using
  `/link/token/get` and verify association before exchange. Confirm this works
  with the chosen Link flow before onboarding implementation; otherwise choose a
  documented server-verifiable flow. Preserve CSRF and same-origin checks.
- Persist only secret references in SQLite. Protect dynamic tokens using the
  existing encrypted profile secret store. Production enablement requires
  encryption; fail closed if storage is plaintext or unavailable.
- Build secret references from a generated canonical lowercase connection UUID,
  scoped by environment/profile. `format_service_id` lowercases logical keys,
  while Plaid IDs are case-sensitive; never embed raw Item/account IDs in keys
  or normalize provider identities. Store original provider IDs only in owned
  connection metadata.
- Make token save/metadata failures recoverable without creating orphaned live
  connections or exposing tokens. Profile delete, backup, restore, and secret
  cleanup must recognize Plaid keys; never repopulate a deleted profile.
- List sanitized connection/account metadata. Require explicit selection and
  mapping to a new or existing Wealthfolio account before importing. Match IDs,
  not account names; reject an account already mapped to another source unless
  the owner explicitly replaces that source through a reviewed workflow.
- Enforce institution, product, and fetched account type/subtype server-side,
  never from frontend labels. Reject Amex savings. Ambiguous Merrill variants
  require owner review before activation rather than assuming Merrill Edge.
- Item metadata, mappings, cursors, secret references, and Link sessions stay
  profile-local and outside device-sync/outbox replication. Existing broker
  exclusion rules apply to imported financial records. Tokens never enter
  data-only exports. Restore without matching credentials means disconnected;
  database backups must not be assumed to include the encrypted secret store.
- Reconnect using Link update mode for the existing Item. Disconnect stops work,
  calls `/item/remove`, removes its credential, and retains imported history. If
  revocation fails, retain a disabled pending-revocation reference and show
  retry status; do not claim remote access was revoked.
- Existing successful private-test Items should be reused through an owner-run
  secure migration, if still available. Never paste their tokens into chat or
  commit them. Creating replacement Items requires checking the current Plaid
  plan's connection limits; deleting an Item may not restore a Trial allowance.

### 3. Bank and credit card ingestion

- Fetch `/transactions/sync` to completion, including added/modified/removed
  records, before committing the next account cursor. On pagination mutation,
  restart from the original cursor with bounded attempts, discarding the partial
  batch. Each request uses `options.account_id`; never mix Item-wide and
  account-scoped cursors. Do not advance on fetch, validation, review, or
  persistence failure.
- Apply the full batch and cursor atomically in SQLite, extending the existing
  repository transaction boundary if necessary. Do not hold a database
  transaction open during HTTP requests. Avoid unselected accounts' cursor
  effects by never requesting them; selecting another account starts its own
  initial cursor/history without rewinding already mapped accounts.
- Extract a transaction-local apply helper from the activity repository,
  preserving user-overlay/outbox rules. Apply one account batch, its removals,
  import-run state, and account cursor in one `WriteHandle::exec_tx`. Sequential
  calls to existing `bulk_upsert` and import-run APIs cannot provide atomicity.
  Emit portfolio/spending events after commit; fault injection proves full
  rollback.
- First-release review policy: block the affected account, leave records/cursor
  unchanged, persist a sanitized review-required status, and stop automatic
  retries for that account until owner resolution. Refetch from the unchanged
  cursor afterward. Never skip unsupported changes. Durable staged review is
  deferred unless this all-or-nothing policy proves insufficient.
- Stable identities include environment, Item, external account, and provider
  record ID. Handle pending-to-posted replacement and removed records without
  deleting manual entries or overwriting user edits. Preserve source provenance
  through import runs, including corrections and tombstones.
- Map deposits, withdrawals, card liabilities/payments, and transfer pairs to
  the existing financial model. Confirm sign conventions with fixtures. Prevent
  internal transfers/card payments being counted twice as income/expense. Use
  decimal values, explicit currency, and nullable available balances.
- Banks/cards use TRANSACTIONS mode; cards reject HOLDINGS. Reuse existing
  spending categorization and card/liability behavior. Do not pair internal
  transfers/card payments by amount/date alone: ambiguous matches require review
  and must not silently suppress actual expense or create apparent income.
- Initial import displays a review summary; establish an opening-balance
  boundary consistent with the existing tracking mode. Never infer complete
  historical net worth from limited provider history or double count balances
  and activities. Unknown transaction categories become review warnings.

### 4. Brokerage ingestion

- Fetch holdings/securities and date-paginated investment transactions. Reuse
  the existing investment checkpoint/lookback model with stable provider IDs;
  reject inconsistent pagination and deduplicate overlapping windows.
- Map buys, sells, dividends, interest, fees, transfers, splits, and supported
  options with explicit type/subtype rules; Plaid investment amount signs differ
  from a generic income/expense convention. Unsupported records require review,
  never a guessed trade. Missing symbols/cost basis/tax lots are valid cases.
- Resolve assets using existing asset services and preserve manual edits. Keep
  positions, cash, and activity-derived valuation consistent with the chosen
  account tracking mode; do not apply holdings plus trades as duplicate changes.
  Disclose limited history and exclude unsupported historical performance.

- Initial brokerage release uses HOLDINGS mode for net-worth snapshots and cash,
  with investment transactions shown only in a separate source-history review,
  not applied to the same ledger. Transaction-derived performance is a later
  opt-in after complete-history reconciliation and opening positions are
  established; do not combine it with authoritative holdings snapshots.

### 5. UI, scheduling, and operational wiring

- Add a separate direct Plaid section under account connections: Connect bank,
  Connect brokerage, selected accounts, review, Sync, reconnect, disconnect,
  last successful sync, source freshness, and actionable sanitized failures.
- Load the official Link client only when the user starts linking. Narrowly
  extend CSP for required Plaid script/frame/connect origins; do not allow all
  HTTPS scripts/frames. Test OAuth return after full browser navigation, expired
  sessions, cancellation, duplicate clicks, and right-to-left layout.
- Extend server scheduling with a daily due check for direct Plaid independent
  of Connect entitlement. Manual/scheduled sync share services and guards.
  Persist last success; after restart run overdue work once. Isolate failures
  per Item, respect rate limiting, and avoid retrying reconnect-required Items.
  Reuse existing worker cancellation and profile lifecycle admission.
- Defer public webhooks for the first release: daily/manual polling meets the
  requirement without adding an unauthenticated ingress/verification surface. A
  future webhook needs verified Plaid signatures and body hashes before use.
- Extend GCP config validation, Secret Manager fetch/export, Compose wiring,
  operator documentation, and placeholder examples. Global secret comes from
  Secret Manager at runtime; dynamic access tokens stay in encrypted application
  storage. GitHub Actions builds images without Plaid credentials.

## Validation and release gates

Use synthetic unit and local HTTP fixtures; no live financial credentials in CI.
Target at least 95% line and branch coverage for newly added Plaid code,
measured separately so unrelated coverage cannot conceal missing critical paths.
Report the coverage tool, scope, exact revision, and any instrumentation limits;
do not call missing branch instrumentation a passing measurement.

Required regression cases:

- Disabled/configuration failures make no requests; secrets cannot be read,
  overwritten, or deleted through generic APIs; errors/Debug never reveal them.
- Authentication, profile isolation, session expiration/replay, forged callback,
  native unsupported behavior, CSP, and encrypted-store enforcement.
- Multi-account Item pagination, mutation restart, empty/repeated pages, initial
  readiness, pending replacement, modifications/removals, currency/signs, manual
  edits, selecting accounts later, and source-replacement conflicts.
- Crash/failure before batch commit leaves data/cursor unchanged; replay is
  idempotent. Mid-run disconnect/profile delete cannot write or recreate
  secrets.
- Investment overlap, corporate actions, missing security metadata/cost basis,
  unsupported types, history gaps, and holdings/activity double counting.
- Concurrent/manual/scheduled sync, restart, provider outage, rate limits,
  reconnect, failed revocation, backup/restore, and source freshness.

Run focused Rust crate/storage/server tests, fmt and Clippy, affected runtime
compilation, frontend tests/types, adapter parity, both frontend runtime builds,
and deployment Python tests. Exercise web onboarding/sync/reconnect/disconnect
with Sandbox and synthetic data. The owner's private connectivity tests are
accepted evidence for institution access, not a substitute for importer tests.

Before production enablement: back up the profile database and encrypted secret
store together, register the actual HTTPS OAuth redirect, configure the minimum
Secret Manager access on the VM identity, and verify configuration without
printing values. Pilot one existing Item with an import review and compare its
results privately to the institution. Expand to the remaining institutions only
after reconciliation. Rollback disables scheduling/network access and restores
compatible application code; avoid rewinding only the database cursor against
newer financial records or secrets. No trades or monetary transactions are part
of validation or rollout.

## Reference contracts

- Existing services: `crates/connect/src/broker_ingest/`,
  `crates/connect/src/broker/service.rs`, `crates/core/src/secrets/mod.rs`.
- Existing server boundaries: `apps/server/src/api/connect.rs`,
  `apps/server/src/scheduler.rs`, `apps/server/src/api/secrets.rs`,
  `apps/server/src/secrets/mod.rs`, `apps/server/src/profiles.rs`.
- Runtime adapters: `apps/frontend/src/adapters/` and its
  `adapter-command-parity.test.ts`; deployment: `deploy/gcp/`.
- [Plaid Link API](https://plaid.com/docs/api/link/),
  [Transactions](https://plaid.com/docs/api/products/transactions/),
  [Investments](https://plaid.com/docs/investments/),
  [OAuth](https://plaid.com/docs/link/oauth/).

## First PR implementation status

The provider client and reserved secret namespace are implemented. No frontend,
routes, scheduler, database migration, GCP configuration, or live credentials
are enabled. Provider DTOs deserialize payloads; account-scope consistency,
duplicate/cursor validation, Link expiration and verified session association
remain mandatory lifecycle/ingestion gates, not guarantees of deserialization.
The exact-decimal adapter enables `serde_json/arbitrary_precision` through Cargo
feature unification; run existing Connect/core regressions as well as new tests.

## Foundation verification

The scoped provider suite has 12 tests, using only loopback HTTP fixtures and
synthetic financial records. A fresh nightly LLVM branch run reports **251/252
lines (99.60%) and 44/44 instrumented branches (100%)** for `plaid/client.rs`.
DTO/module declarations have no executable coverage entries. Core secret-key
reservation is tested separately and is outside these percentages.

`.github/workflows/plaid-coverage.yml` runs the provider and secret-reservation
tests and independently requires at least 95% lines and branches. It pins the
same nightly/tool versions as existing owner-auth instrumentation. LLVM 23's
all-source export crashes on unrelated mappings, so `plaid_coverage.py` uses
explicit source paths, like the existing owner-auth exporter. Its four unit
tests cover threshold boundaries, missing instrumentation, invalid counts, and
unexpected/duplicate sources. Generated reports live in `coverage/plaid/` and
are CI artifacts, not committed financial data.

Broader first-PR checks are Connect/core regression tests together (to exercise
Cargo feature unification), fmt, affected-crate Clippy, runtime compilation,
workflow lint and documentation formatting. Steps 2–5 have additional release
gates above; foundation coverage does not establish their correctness.

## Review record

Independent reviewer identified transaction composition, review/cursor policy,
tracking/spending semantics, local ownership/restore, server-side eligibility,
Link-session association, and ambiguous transfer handling. The sections above
now specify each contract. Reviewer recommended a foundation-only first PR; its
explicit scope is recorded in the delivery sequence. The reviewer found no
unresolved blockers for that foundation scope. A subsequent official API check
confirmed per-account Transactions Sync streams (`options.account_id`); reuse
existing account checkpoints instead of introducing an Item-wide cursor. This
keeps atomic ingestion scoped to one account. The reviewer re-reviewed that
change against the official API and found no unresolved foundation blockers.
Reassigning an account to another Item/environment clears its checkpoint; never
reuse a cursor across streams.

Implementation review caught reqwest 0.13's default protocol retry policy. The
constructor now explicitly uses `retry::never()`; redirects are also disabled.
The reviewer found no other implementation blockers for the foundation scope.
