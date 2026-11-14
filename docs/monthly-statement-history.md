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
it. History-sized reads run through the existing blocking executor. There are no
new provider calls, jobs, dependency packages, or secret-store changes.

## Validation

Synthetic tests cover summary-column selection, unknown basis, malformed and
incomplete positions, batches, review confirmation, replacement/deletion,
concurrent revisions, rollback, profile/account isolation, retained snapshots,
monthly carry-forward, calculated cutover, and mixed-account net-worth history.
Financial documents supplied by a user are never committed as test fixtures.
