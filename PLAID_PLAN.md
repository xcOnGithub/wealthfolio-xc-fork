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
- Two disposable Sandbox personas accepted account-scoped card sync, holdings,
  and the same-Item Hosted Link update request. Their history was NOT_READY;
  completed import behavior is established by synthetic service/SQLite tests.
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
- Verified: the local Wealthfolio callback
  `https://localhost:3000/settings/accounts` is registered and the full browser
  Hosted Link flow returns successfully. Legacy Quickstart root callbacks are
  separate from this deployment.
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

## Local deployment finding: completed empty history

The full browser Link flow on an isolated encrypted Sandbox installation exposed
an additional case: the Business Credit Card account returns
HISTORICAL_UPDATE_COMPLETE, no changes, has_more=false, and next_cursor="". The
previous service rejected it indefinitely as NotReady. The new regression fails
against that behavior before the fix.

Architecture impact: preserve None as the unreviewed initial mapping and use the
existing Some("") checkpoint to retain a reviewed empty account. This prevents
repeated opening-balance imports without adding a persisted field or migration.
The Plaid client permits the documented empty initial cursor. Accept an empty
response cursor only for a complete, terminal, empty first page without a real
saved checkpoint; reject partial readiness, changes, pagination, and regression
from an opaque cursor. Later transactions can advance the same stream normally.
The existing writer still commits the opening and checkpoint atomically.

Reference: <https://plaid.com/docs/api/products/transactions/>
(TXN-002/008/019).

The local callback https://localhost:3000/settings/accounts was added through
Dashboard and freshly verified through MCP. The isolated app completed Hosted
Link, selected both shared cards plus investments, imported the first card's 145
posted transactions plus an opening, and imported brokerage holdings. The
focused regressions also cover the completed empty-card case, repeat sync, and
later cursor advancement. Local deployment acceptance is recorded in the PR
validation. No production Item, VM configuration, or financial data was changed.

## Reviewed brokerage transaction history

### Architecture impact

Brokerage mappings remain in HOLDINGS mode unless the owner explicitly reviews
and activates transaction tracking. The same Plaid Item and encrypted secret
store are reused. An optional JSON mapping checkpoint stores the immutable
reviewed opening date, cash, quantities, and verified aggregate unit costs.
Recomputing openings on every sync would hide missing trades; the existing bank
cursor cannot represent this investment baseline. No schema migration, worker,
queue, retry policy, provider, or lock is added.

Conversion belongs to connect/plaid; validation of actual portfolio accounting
belongs to core. The existing SQLite writer applies source upserts, explicit
cancellations, import runs, broker quotes, account-mode change and checkpoint
inside one transaction. Before commit it runs the same portfolio engine used by
portfolio reads over the actual database ledger. Any mismatch rolls everything
back. User-modified rows retain precedence and inconsistent edits require
review. The existing activities-changed event rebuilds derived views. Broker
snapshots remain retained as evidence; transaction-mode reads exclude observed
holdings snapshots so the same positions are not counted twice.

### Provider and accounting contract

- Investments API joins transactions to their returned securities (INV-001).
  Initial/manual review retrieves the available 24-month window. Scheduled sync
  fetches a 30-day overlap, bounded by the checkpoint start, and retains prior
  source rows by stable transaction ID (INV-005/012). Missing rows alone never
  mean deletion. Explicit cancel_transaction_id removes its original and
  cancellation together; unavailable originals block import.
- Current cash is total account value minus noncash holding values. Initial
  positions/cash are current balances reversed through available history. Later
  sync always uses the fixed approved baseline. Never use available/buying
  power.
- Buy/sell quantities, net cash and fees are checked together. Supported
  ordinary long equities, ETFs and mutual funds must have compatible currency
  and local asset identity. Dividends, interest, explicit cash
  deposits/contributions, withdrawals/distributions, supported account fees and
  taxes use explicit activity meanings.
  Unknown/corporate/derivative/reinvestment events, shorts, missing securities
  and inconsistent economics block the entire activation.
- Nonzero opening positions require an owner-supplied verified unit cost.
  Opening lots are aggregate, without original acquisition dates or tax-lot
  provenance. Current quantities, cash, and reported holding cost basis are
  reconciled through the portfolio engine. This is limited-history accounting,
  not a claim of full lifetime performance or original tax lots.
- Review uses a digest and 30-minute expiry. No financial import happens in
  preview. Activation requires explicit confirmation. Existing edited rows,
  account ownership/type/currency, and connection admission are checked again at
  persistence. Broker price quotes continue after conversion.
- Existing manual/daily freshness is unchanged; readiness webhooks remain a
  separate known gap (CHECK-014). Existing banking/card sync is unchanged.

### Verification

Use pure conversion tests plus SQLite/real-engine integration tests for opening
cost requirements, rollback, idempotency, fee-inclusive trades, cash events,
corrections/cancellations and owner-edit conflicts. UI tests require an explicit
acknowledgment, enforce blockers and verified costs, cover pagination and RTL.
Local end-to-end validation uses only encrypted isolated Plaid Sandbox data:
standard equity history must become ledger activities, derived holdings and cash
must reconcile without snapshot double counting, repeat sync must retain IDs,
and an unsupported option must leave its original holdings mapping unchanged. No
production Item, credentials, GCP configuration or financial data is modified.

References: <https://plaid.com/docs/investments/>
<https://plaid.com/docs/api/products/investments/>
<https://plaid.com/docs/sandbox/user-custom/>

### Local validation evidence

The isolated encrypted app was exercised with a custom equity-only Sandbox Item:
review and explicit activation produced two source trades plus one opening cash
activity. The real portfolio view reports the expected quantity, cash and cost
basis; repeat sync retained the same IDs. The original standard Sandbox persona
contains a derivative and unsupported source events: review displays blockers,
leaves confirmation disabled and preserves HOLDINGS mode with no ledger rows.
The browser exposed a timezone shift in date-only review labels; use the shared
calendar-date formatter and its regression test. Blocker lists scroll within the
review so a long history does not bury the action controls.

A separate custom Sandbox persona covers a nonzero opening position and
conversion of an already imported broker snapshot. Local evidence is kept
outside the repository; no tokens, keys or financial rows are committed.
Production institution-specific history, corporate actions and original tax-lot
accounting remain outside this validated first version.

## Investment share-movement reconciliation follow-up

- Decisions: reuse the existing reviewed import and real portfolio engine for
  security transfers; require explicit incoming book cost per source record.
  Preserve previously reviewed costs in defaulted ledger metadata rather than
  inferring them from transfer market values. Add an optional transfer-cost map
  to the existing approval action; no new route or provider call is needed.
- Quantity preview uses signed trades and in-kind transfers, preserving provider
  security IDs and the existing duplicate-asset guard (INV-001, INV-004,
  INV-008). Economic inconsistencies and unsupported instruments continue to
  block writes.
- Rounded unit prices use a bounded check and cash-consistent effective
  execution prices; there is no arbitrary price/basis override.
- Architecture impact: synchronous shared conversion; the existing ledger JSON
  gains defaulted transfer costs for idempotent sync. Old records need no table
  migration. Existing profile ownership, credential storage, approval digest,
  admission checks and atomic SQLite reconciliation remain authoritative
  (GUIDE-001, GUIDE-002, GUIDE-006, DATA-007).
- Verification: synthetic pure conversion, actual SQLite/engine atomic approval,
  rejected missing/negative/unrelated/changed transfer costs, repeat sync,
  immutable baseline, cancellation handling, and LTR/RTL review tests. A local
  read-only replay exercises the updated review without permitting approval.
- Acceptance limits: no new live Sandbox Link/exchange was exercised for this
  follow-up (CHECK-008); unresolved source economics/unsupported instruments
  have no in-app override and remain blocked (CHECK-012, CHECK-015). The
  pre-existing webhook/freshness gap remains separate (CHECK-014). New Dashboard
  tasks are not applicable because Link, products, tokens, callbacks and
  provider calls do not change. This does not claim a production account
  cutover.
