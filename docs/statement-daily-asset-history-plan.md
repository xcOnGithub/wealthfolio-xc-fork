# Daily asset history from Merrill statements

Reconstruct historical daily holdings and cash from reviewed Merrill monthly
statements, then value them using historical market prices. Historical
reconstruction must work without Plaid transaction matching. Plaid continues to
own the existing live account history; statement reconstruction supplies
separately identified historical periods.

The first release covers daily account values and the net-worth dashboard. It
does not create tax lots, change current holdings, approve transfer costs, or
enable investment-return calculations for reconstructed periods. Monthly
statement snapshots remain available when daily reconstruction is incomplete.

## Evidence and existing components

The supplied October 2025 and October 2026 CMA statements contain holdings,
opening and closing cash, dated security trades, income and cash movements.
Local checks reconciled opening cash plus dated movements to closing cash in
both samples. Closing security market values plus cash also reconciled to the
reported account totals after diagnostic numeric normalization. These checks
establish useful monthly checkpoints; they do not independently verify every
historical daily value.

The samples expose specific requirements: leading-decimal fees and fractional
quantities, separate purchase and sale totals, cash sweeps, securities received
by transfer, holdings with unavailable basis, and unsettled trades. Historical
quotes remain necessary because monthly statements do not supply daily prices
for every position.

Reuse these existing components:

- The bounded browser PDF reader and the five-file batch limits: 10 MiB per
  file, 25 MiB total, sequential extraction and aggregate text limits.
- The Merrill v2 activity parser and monthly summary parser. Extract their
  common number, section, identity and date handling into a feature-owned
  module.
- Monthly snapshot review, explicit confirmation, revision checks and atomic
  writes described in [Monthly statement history](monthly-statement-history.md).
- The shared portfolio engine's valuation stage, historical quote and FX
  services, and explicit value-quality and unknown-basis statuses.
- Existing core blocking execution, web/Tauri adapter patterns and profile-local
  SQLite storage.

## Scope and source ownership

Start with USD Merrill Edge CMA statements containing long equities, ETFs and
cash. Support purchases, sales, dividends, interest, explicit fees and taxes,
external cash movements and security transfers when their effects are known.

Reconstruct only the covered statement intervals. A missing month is a coverage
gap, even if positions at adjacent month ends happen to agree. An explicit
reviewed confirmation of no activity may bridge an interval later; absence of an
uploaded statement must never imply no activity.

Define the live-history handoff independently of reconstructed valuations: use
the reviewed ledger coverage boundary for transaction-tracked accounts and the
accepted observed-history boundary for holdings-tracked accounts. Do not change
account tracking mode. Do not derive the boundary from the first complete
valuation after adding reconstructed history, because that would move it
backward and hide historical statement observations.

Each historical record belongs to a statement document and period. An idempotent
reupload contributes no new events. Legitimate identical fills remain separate
records identified by document and source-row occurrence. Statements are not
deduplicated against individual Plaid transactions: historical and live sources
are selected once per account and date at the handoff.

At that handoff, compare available cash and quantities with the live opening
state. A mismatch must be visible and must not cause synthetic cash movements,
position adjustments or changes to Plaid records. Standalone historical periods
can remain available with a disclosed discontinuity; a continuous-history claim
requires an explained handoff.

## Normalized statement facts

Produce one versioned review model containing:

- Statement period, currency, transient account identity, format version and
  document fingerprint.
- Reported opening cash, closing cash, closing account value and the complete
  reported closing position list where available.
- Security identifiers and user-reviewed links to existing assets, including
  securities fully sold during the month.
- Events with an occurrence identity, kind, quantity delta, final cash delta,
  reported fee, effective-date convention, available trade/settlement dates and
  page/row references.
- Separate unsettled transactions, reported sale basis, unknown basis, excluded
  cost evidence, unsupported records and extraction-completeness diagnostics.

Keep acquisition basis, execution prices, transfer market values and closing
market prices separate. Unknown cost basis does not prevent valuing a holding.
The tiny fractional positions excluded from cost evidence must still be present
in quantities and market values.

## Date and event policies

The initial PDF workflow produces explicitly labelled settlement-date history.
Use the statement's posted dates for cash and its documented effective dates for
quantity changes. Do not infer trade dates by subtracting a fixed number of
business days; settlement rules, holidays and exceptions vary.

Keep unsettled trades separate from settled events. Establish whether each
supported statement layout includes pending trades in its closing holdings and
cash before publishing the affected month. If pending treatment cannot reconcile
with those checkpoints, keep that month as a monthly snapshot until resolved. Do
not book the same trade again when the following statement settles it.

A later optional Merrill CSV supplement can provide explicit trade dates and
support trade-date positions with the corresponding unsettled cash obligations.
That requires reconciliation within Merrill documents, rather than Plaid
transaction matching, and must be validated before offering trade-date history.

Event rules:

| Event                                      | Quantity and cash treatment                                                                                                                     |
| ------------------------------------------ | ----------------------------------------------------------------------------------------------------------------------------------------------- |
| Purchase or sale                           | Apply the verified quantity delta and final settlement cash. Statement trade cash already includes its reported fees; do not charge them again. |
| Income or external cash movement           | Apply the posted cash delta once. Preserve its external/internal classification for future use, without enabling returns now.                   |
| Cash sweep                                 | Treat movement between brokerage cash and its deposit program as internal. The combined cash balance must not change twice.                     |
| Security transfer                          | Apply the quantity change. No acquisition-cost estimate is required for market valuation.                                                       |
| Return of capital or basis-only adjustment | A verified cash-only effect can be represented for net worth while basis remains unknown. Keep stricter cost-evidence approval rules separate.  |
| Split or reinvestment                      | Require verified effective dates and quantity/cash effects. Avoid applying a split twice through both event processing and quote adjustment.    |
| Unsupported event                          | Block the dependent reconstruction interval if its cash or quantity effect is unresolved. Preserve the verified monthly account total.          |

## Reconstruction and reconciliation

1. Extract every relevant section. Normalize valid leading-decimal forms such as
   `(.xx)` and `.0001`, signed values and placeholders. Recognize separate
   purchase/sale blocks and totals across continuation pages. Unknown layouts
   and unreadable financial rows must produce actionable errors.
2. Review account identity, currency, document period, section completeness,
   security mappings, dates and unsupported events. Do not guess a security from
   its quantity or market value.
3. Derive an opening position state by reversing the month's verified quantity
   changes from its complete closing positions. A security absent from a closing
   list can mean zero only when that list is verified complete.
4. Use reported opening cash and replay dated cash deltas. Independently reverse
   from closing cash to check the same opening balance. Group each day's effects
   consistently; ambiguous ordering or unexplained short positions blocks the
   affected interval.
5. Replay forward and verify the closing cash and each security quantity.
   Compare adjacent statement boundaries when both are available. Reconcile
   separate debit/credit and purchase/sale totals where printed; matching a net
   total alone is insufficient.
6. Verify closing securities plus cash against the reported portfolio total
   using the statement's own closing valuations. Include unknown-basis holdings.
   Use decimal arithmetic, exact share quantities and explicit reporting-scale
   monetary tolerances; do not add a general share epsilon.
7. Produce daily position and cash keyframes only inside the validated interval.
   Do not extrapolate holdings through missing months or before the first
   established opening state. Endpoint reconciliation is necessary but does not
   replace complete extraction of the transaction sections.

## Market prices and valuation

Resolve every security appearing in opening positions or events, including
positions fully disposed before month end. Fetch historical prices only through
already configured and enabled market-data providers; document import must not
silently enable providers or introduce a new external service.

Reuse the portfolio engine's valuation stage with historical position/cash
facts. Keep them isolated from current account-state snapshots and ordinary
ledger replay. Extend the fact boundary only as needed to accept bounded
historical keyframes with unknown basis; do not create a separate pricing
formula or fabricate zero-cost tax lots.

Use the existing quote and FX quality rules. A previous close can cover a
weekend or exchange holiday; an unexpectedly stale quote on trading days must
remain flagged. Never use a later quote to backfill an earlier day. Missing
quotes, unresolved assets and missing FX must produce partial/unavailable values
rather than an apparently complete account total.

Verify split-adjusted versus unadjusted quote conventions. Statement closing
prices may differ from provider closes because of timing or rounding. Report
those differences separately from cash/quantity reconciliation; never alter a
position or cash balance to force a market-price match. Verified statement-date
account totals remain visible as reported observations.

## Persistence and application boundaries

**Architecture impact:** extend browser extraction to send explicitly reviewed
normalized transaction and position facts to the existing backend. Persist those
facts in profile-local SQLite so history can be reconstructed after restart. The
current monthly snapshot contract contains no events or coverage policy; it
cannot supply this behavior on its own.

Add a versioned reconstruction record related to the existing account statement
history, using a new migration and the existing database writer. Store
normalized facts, asset mappings, period, date policy, coverage status, source
references, fingerprint and revision. Original PDFs, full account identifiers
and unrelated statement descriptions remain unpersisted and unlogged. Raw
documents stay in the browser; no OCR or AI service is introduced.

Reuse optimistic revisions and atomic batch writes. Replacements, deletions and
mapping changes must update source facts and invalidate the affected historical
results together. Derived history must be reproducible from the reviewed facts
and current quote data; avoid a new cache or background worker in the initial
implementation. Bound history-sized reads and run them through the existing core
blocking executor.

Keep business validation in core. Wire review/preview/save/delete operations
through shared frontend adapters, both web and Tauri runtimes, and thin server
handlers. Confirmed changes may refresh historical quote coverage through the
existing provider/coordinator mechanisms; they must not change Plaid settings,
run Plaid sync, edit current holdings or update existing ledger activities.

## Dashboard and review experience

Extend **Import historical statements** with monthly snapshot and daily history
options. Daily review shows parsed sections, asset links, cash and quantity
reconciliation, pending events, price availability, date convention and covered
intervals. Keep the existing five-file memory limits and sequential processing.

For each account and date, select exactly one source:

1. Existing live calculated history at or after its ownership boundary.
2. A verified statement closing observation on its historical statement date.
3. Validated daily reconstruction within its approved historical interval.
4. The existing labelled monthly carry-forward fallback where eligible.

Never silently treat the fallback as reconstructed daily history. Preserve gaps,
partial pricing and handoff discrepancies in chart quality indicators. Do not
reactivate old statement values inside later live-history gaps. Combined
net-worth reads must apply the same source selection as the account chart, with
cash separated once from investment value.

Keep current balances and investment-return calculations unchanged. Disable
contribution-adjusted metrics for ranges that include reconstructed value-only
history until the necessary flow and basis semantics are separately supported.
All new controls, errors and source labels must support every locale and RTL.

## Delivery phases

| Phase                   | Deliverable and completion gate                                                                                                                                                                                                                                             |
| ----------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 1 Parser foundation     | Shared Merrill extraction model, leading-decimal support, complete fractional holdings, correct block totals and pending classification. Both sample layouts pass local reconciliation; committed tests use invented data. Existing v1/v2 cost workflows remain compatible. |
| 2 Reconstruction core   | Pure deterministic cash/quantity reconstruction, date policy, interval coverage and diagnostics. Synthetic multi-day and multi-month cases reproduce known states, including fully disposed securities and unknown basis.                                                   |
| 3 Valuation integration | Bounded historical keyframes priced by the shared engine. Unknown basis remains distinct from missing prices; weekends, splits and quote gaps behave explicitly.                                                                                                            |
| 4 Reviewed persistence  | Minimal migration, shared core validation, atomic revisions and idempotent imports across web and Tauri. Source replacement/deletion and profile isolation are tested.                                                                                                      |
| 5 Portal and charts     | Review and save daily history; display source quality and handoff status. Account and combined net-worth charts select sources once without changing current balances or performance inputs.                                                                                |
| 6 Release validation    | End-to-end synthetic imports, private local sample checks, backup/restore and regression verification. Publish only supported, reconciled intervals; retain monthly snapshots for the rest.                                                                                 |

Implement parser foundations as a focused follow-up to PR #26. Keep storage,
valuation and chart integration in subsequent reviewable changes. The feature
remains unavailable for ordinary imports until the corresponding end-to-end
review, persistence and valuation path is complete.

## Required tests and acceptance criteria

- Parser regressions for both layouts: headings, continuation pages, signed
  leading-decimal fees, fractional quantities, unavailable basis, identical
  fills and lots, duplicate pages, malformed fields, separate totals and pending
  trades.
- Deterministic daily cash/quantity checks before, on and after trades, income,
  transfers, fees and sweeps. Include partial sales, fully disposed securities,
  split/reinvestment cases, negative-position errors and same-day ordering.
- Reconcile opening and closing checkpoints and consecutive months. Reject
  offsetting debit/credit errors, incomplete sections and unexplained changes;
  never hide an interval gap through interpolation.
- Reimport, overlapping documents, conflicting replacements, stale reviews,
  transaction rollback, deletion, account/profile isolation and database
  backup/restore. Identical real fills must survive duplicate protection.
- Price fixtures covering non-trading days, prolonged missing quotes, delisted
  assets, splits, quote-currency normalization, unknown basis and partial
  values.
- Prove statement-only reconstruction works with no Plaid connection or Plaid
  transaction data. No Plaid request may occur during preview or confirmation.
- Prove historical imports cannot alter live ledger rows, current positions,
  reviewed costs or existing performance results. Verify one source per
  account/day, consistent account/combined charts and visible handoff
  mismatches.
- Test actual normalized statement import through persisted history and
  net-worth reads across multiple historical dates; parser mocks alone are
  insufficient.
- Validate the changed UI in LTR and RTL, all locale keys, web/Tauri adapter
  parity, affected Rust services, TypeScript checks and relevant completed
  builds. Keep customer documents and derived financial rows out of committed
  fixtures.

The first-release acceptance target is a labelled, reproducible daily market
value series for each validated statement period, with independently reconciled
cash and quantities. Exact reproduction of Merrill's trade-date account values
requires explicit trade dates, pending-settlement handling and compatible price
conventions; it is not implied by the initial settlement-date series.
