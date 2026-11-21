# Monthly statement history

Monthly statement observations extend historical account-value and net-worth
charts before calculated account history begins. They do not create activities,
change current holdings, establish opening tax lots, or enter performance-return
calculations.

## Import and review

On an investment account page, open **Import historical statements**. Upload up
to five Merrill Edge CMA PDFs (10 MiB each, 25 MiB total). Files are processed
sequentially with the existing bounded local PDF reader; scanned documents that
need OCR and other institutions are not supported in this version.

Review the statement closing date, USD closing account value, source page, and
available position details. Confirm that the documents belong to the selected
account. Mixed-account documents or ambiguous summaries fail the batch before
anything is saved. Extracted drafts are transient until explicit confirmation.
Original PDFs and full institution account identifiers are not persisted.

The Merrill CMA monthly adapter selects **NET PORTFOLIO VALUE** using the dated
closing-value column. It does not substitute the previous period, acquisition
basis, or a partial sum of parsed holdings. Position market values and reported
cost basis are separate fields. An explicit unavailable basis stays unknown;
explicit zero basis remains zero. Known market value minus known basis is shown
as statement-date unrealized P&L; missing basis suppresses that position's P&L.
Incomplete details do not invalidate a verified account total and are disclosed
in review.

An identical normalized reimport is a no-op. Different content for an existing
account/date requires a visible replacement review. Saves compare the reviewed
revision inside the database transaction; concurrent changes reject the entire
batch. Deletion requires a separate confirmation and the current revision.

## Chart behavior

For each account, only the prefix before its first available calculated
valuation is eligible for statement values. The latest statement on or before a
date is carried forward through that prefix. Nothing is backfilled before the
earliest observation. Calculated history takes precedence from its coverage
boundary; a later gap does not reactivate an old statement. Overlapping
statements remain stored for inspection but do not override calculated values.

Statement values are combined once per account. Available reported cash is split
from the remaining investment value in the historical breakdown. The net-worth
chart identifies statement observations and partial account coverage. The
account chart identifies the statement observation date and uses a step line for
statement periods. Contribution-adjusted gain/velocity and momentum summaries
are withheld when the requested period contains statement observations or
incomplete account coverage. Existing transaction-based performance inputs
remain unchanged.

The current-balance read remains separate. Importing an old statement cannot
replace the current account value or the current holdings list. Reported basis
belongs to the statement date; it is not automatically assigned to today's
holdings or used to approve a Plaid opening position or incoming transfer.

Merrill CMA observations are USD in this format. Base-currency conversion uses
the existing FX service at the statement closing date, so carried observations
remain unchanged between statements. Missing FX fails the history read
explicitly rather than substituting an unconverted amount or zero.

## Persistence and architecture

`account_statement_history` is a profile-local SQLite table keyed by account and
closing date, with a normalized payload and revision. It is independent of
calculated snapshots and daily valuations, survives projection rebuilds, and is
removed with its account. Full database and portable database backups include
it; it is not a transaction CSV export or an App Sync entity in this version.

The shared core service validates confirmation, account/currency, amounts,
dates, batch limits and reconciliation. Thin web and Tauri commands delegate to
it. History-sized reads run through the existing blocking executor. Monthly-only
imports introduce no provider calls, jobs, dependency packages, or secret-store
changes.

## Validation

Synthetic tests cover summary-column selection, unknown basis, malformed and
incomplete positions, batches, review confirmation, replacement/deletion,
concurrent revisions, rollback, profile/account isolation, retained snapshots,
monthly carry-forward, calculated cutover, and mixed-account net-worth history.
Financial documents supplied by a user are never committed as test fixtures.

## Daily history extraction foundation

The feature-owned Merrill parsers now share statement identity, period and
number validation. They recognize signed leading-decimal amounts and fractional
shares, and distinguish separately closed purchase/sale blocks from continuation
pages. Existing opening-cost and transfer-cost review continue using their
original approval contracts.

`extractMerrillHistory` produces transient statement facts independently of
Plaid and acquisition costs. It combines the reported closing snapshot, opening
cash and dated activity, and checks cash movements against the closing balance.
Unknown-basis holdings remain in market value. Internal cash sweeps and in-kind
transfer market values do not enter that cash reconciliation. Missing activity
pages, unknown or undated cash movements, incomplete positions, cash mismatches,
unsupported records and unsettled trades remain explicit review issues.

The extraction function itself does not save facts, reconstruct daily
quantities, fetch quotes or publish daily chart values. The reviewed integration
below owns those operations. The implementation sequence and source-ownership
rules are in the
[daily asset history plan](statement-daily-asset-history-plan.md).

## Daily quantity and cash reconstruction

The shared core `portfolio::statements::reconstruction` module accepts reviewed,
asset-mapped statement intervals and produces bounded daily closing quantities
and cash. It derives opening quantities by reversing verified share movements
from the complete closing position list, including securities fully disposed
within the month. Cost basis is optional and is never substituted for market
value. Final posted cash already contains trade fees.

The input explicitly declares settlement-date policy and complete activity and
closing positions. Unknown events, invalid signs, duplicate source occurrences,
unsupported quantities and unexplained negative positions fail validation.
Equal-valued fills with different source occurrences remain distinct. Same-day
movements are grouped at the daily close; intraday sequencing is not inferred.

Cash movements must explain closing cash within one cent. The final day uses the
reported closing cash, and adjacent intervals must agree exactly on reported
cash and quantities at the handoff. Reported holdings plus cash must explain the
closing account total. Statement intervals can cover up to 35 calendar days,
including both endpoints, to accommodate month-end business-day shifts.
Overlapping intervals are rejected, and missing months remain coverage gaps.
Reconstruction does not fetch prices, write ledger activities, change account
tracking modes or publish daily chart values.

## Historical market valuation

`portfolio::statements::pricing` prices reconstructed daily states through the
existing portfolio engine using core asset metadata and cached close prices. The
run uses isolated observed states with explicitly unknown basis. It creates no
tax lots, ledger activities or stored account-mode changes, and does not fetch
quotes or enable providers. The initial supported instruments are USD-priced
long equities and ETFs with the standard share multiplier.

Daily values use only quotes on or before the valued date. Weekends and holidays
carry the prior close; carries of seven days or more are explicitly marked
stale. A partial priced sum is never returned as a complete account value.
Reported closing market values are checked per security, so offsetting price
errors cannot make an incorrect series appear reconciled. Closing-price
verification remains unknown when a closing price is missing or there are no
closing positions to check. Daily price coverage is evaluated separately. A
verified mismatch blocks a daily value when that day holds the mismatched
security. Days with complete price coverage and no affected holdings retain
their values.

The output contains daily values and quality, with returns eligibility
explicitly false. Persisted source facts, quote coverage, user review and
dashboard source selection use the reviewed integration below.

## Reviewed daily imports

On a securities account, open **Import historical statements**, then choose
**Daily history from statements** in **History import mode**. Upload up to five
complete Merrill CMA PDFs (10 MiB each, 25 MiB combined; 100 extracted pages and
one million characters across the batch). Extraction is sequential and stays in
the browser. Review the cash check, extracted sections and every dated event.
Link each closing holding and share-moving event to its registered USD security,
including positions fully sold during the month. Asset identity is never
inferred from matching quantities. Create any missing security through Settings
first.

**Preview daily history** reconstructs cash/quantities and reads cached prices.
**Refresh historical prices** explicitly requests the covered dates, plus a
seven-day initial price lookback, from existing configured providers. Manual
assets use their stored prices. Refresh neither enables a provider nor contacts
Plaid. Individual provider request failures are counted, successful coverage is
retained, and the preview shows complete, stale, missing and closing-price
mismatch statuses. Price quality is re-evaluated from current cached quotes on
subsequent reads. Imported historical facts do not change provider settings.

Save requires an explicit full-statement confirmation after preview. The
profile-local `account_statement_reconstructions` table stores versioned,
normalized cash/quantity events, reviewed asset links and source page/row
references. Document filenames, account identifiers, counterparty descriptions
and PDFs are not persisted. The original reported monthly checkpoint and its
facts are written atomically using the existing writer and optimistic revisions.
Replacing a monthly checkpoint invalidates its daily facts; deleting a statement
or account cascades to its facts. Reuploading identical normalized data is a
no-op. Full database backups include the new table; App Sync and transaction CSV
exports do not include these facts.

Account history and combined net worth share one selection function: live
calculated history owns dates at and after its first persisted live-valuation
boundary (including unpriced live observations); reported historical statement
closes win on their own dates; validated, priced reconstruction supplies other
covered days; labelled monthly carry-forward remains the fallback. A price gap
never becomes a zero-valued security or an apparently complete partial sum.
Combined-history missing-account indicators include unavailable statement days.
Historical imports never move the live boundary backward and cannot reactivate
inside a later live-history gap.

The portal shows saved periods, daily price coverage, gaps between periods and
live handoff status. Where the two sources contain the same boundary day, cash
and exact quantities are compared without adjusting either source. Without a
common dated checkpoint the handoff remains explicitly unverified. A mismatch
does not prevent a separately identified historical period from being retained.
Chart tooltips distinguish reconstructed settlement-date values, reported
closes, monthly carries and stale prices. Contribution-adjusted metrics remain
withheld for statement-derived ranges.

The first release intentionally blocks daily publication for unresolved pending
trades, unsupported corporate actions, incomplete sections and unexplained
cash/quantity changes. Such documents can still be imported as monthly
snapshots. It does not infer trade dates, supply tax lots or approve historical
cost basis. Unknown basis, including tiny fractional holdings, does not itself
block market valuation.

For reconstructed days, FX conversion uses the shared FX graph with an explicit
on-or-before policy on every edge. A closer future FX rate cannot value an
earlier day; absent dated FX coverage fails explicitly. Existing nearest-date FX
consumers retain their compatibility policy. Idempotency compares the complete
bounded normalized payload, rather than storing an additional raw-PDF hash.
