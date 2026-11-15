# Opening cost evidence for linked brokerage accounts

In Settings → Accounts → Bank and brokerage connections, choose **Enable
transaction history**, then **Fill opening costs from a PDF or CSV**.

This helper fills the existing opening-cost review. It does not create
activities, extend Plaid's history, establish original tax lots, or change an
already approved opening checkpoint. Final import approval remains a separate
step with the existing backend reconciliation.

## Sources and review

Use a text-based, unlocked monthly PDF statement containing position quantities
and cost basis, a position CSV, or a complete security transaction CSV. PDFs are
extracted locally in the browser, without OCR or an AI service. The extracted
CSV/TSV table uses the existing authenticated CSV parser on the Wealthfolio
server. Uploaded documents are not retained. The temporary extracted text and
starting statement are cleared when this review is closed or refreshed; only
approved aggregate costs reach the existing import endpoint.

1. Choose the brokerage file format or the explicit manual mapper, then upload a
   file (up to 10 MB and 100 PDF pages).
2. Review the extracted text against the document. For manual mapping, keep one
   account's table; remove prose, totals, cash lines and repeated page headers.
   Correct any merged PDF columns. Choose the header row and read the table.
3. Verify column mappings, dates and currency. US decimal notation is required;
   dates are ISO `YYYY-MM-DD` or US `MM/DD/YYYY`.
4. Calculate proposals. Explicitly match each security to its opening position.
   Quantities must match, except for the explicitly reviewed fractional
   remainder described below. Matching quantities alone do not establish
   security identity or account ownership.
5. Confirm the evidence and fill the proposed opening costs. Review any
   remaining manual costs and confirm the original investment import separately.

Statement market value, current price, sale proceeds, buying power and cash
balance are **not** cost basis. They are never automatically mapped as such.
Unknown basis is not silently converted to zero. The fractional-remainder
exception below preserves known total cost and discloses that the omitted
fraction adds no basis.

## Monthly statements before the cutoff

The opening position is measured at the end of the day before the first Plaid
transaction. For a first transaction on May 13, 2025, use May 12 end-of-day.

An April 30 statement can be used as a starting checkpoint: calculate its
positions, choose **Use statement, then upload intervening CSV**, and upload the
complete May 1–12 security history. Use identical security identifiers in both
tables, correcting names/symbols explicitly if necessary. Dates before/on the
statement and dates after the cutoff are excluded and counted. Changes between
the statement and cutoff must be covered; merely changing the statement date is
not reconciliation.

The starting checkpoint is in memory only. A new file does not discard it; use
**Remove starting statement** to start over. Review the document's account,
currency, identities and quantities before using it. A later statement cannot be
used to infer earlier costs by guessing historical lots.

## Uploading multiple documents

Select up to **five files at once** after choosing a supported brokerage format.
Merrill batches may mix supported statement PDFs and Settled Activity CSVs; the
selected brokerage determines the appropriate versioned adapter for each file.
Manual column mapping continues to accept one file at a time. A new selection
replaces the current batch; there is no persistent upload queue.

Limits are 10 MB per file, **25 MB total**, one million retained extracted text
characters and 10,000 parsed data rows per batch. Documents are processed
sequentially, with statements read before CSVs to establish the date window and
each PDF worker destroyed before the next document. Original selection indices
are retained for file/row references. Raw file objects are not retained in React
state. Only normalized records, source references and the reviewed opening
checkpoint remain in memory. Closing or refreshing the review clears this state.

All files in a batch must identify the same account. Account numbers are
compared transiently by the parsers and are not included in normalized rows or
batch results. Unsupported records within the required interval, malformed files
or a failed file stop the whole batch; preceding files are never partially
applied. File/row references identify the error without displaying a parser's
raw exception.

Statements are checkpoints, not additive holdings. The latest statement on or
before the opening cutoff supplies the baseline. Earlier statements are marked
as superseded. Identical same-day statements are redundant; conflicting same-day
statements stop the batch. A statement after the cutoff is rejected. If a
statement establishes the cutoff itself, CSV records are marked as covered by
that statement and are not added to its positions.

Otherwise, CSV records are combined and sorted by the existing calculator.
Records on/before the baseline or after the cutoff are excluded from the cost
calculation. Duplicate transactions in the required interval stop calculation;
no automatic deduplication guesses whether identical trades are separate events.
Use non-overlapping exports or resolve the records explicitly. Cash-only records
can accompany a verified statement without changing its security cost basis.
Complete-history confirmation is still required, including when no security
trades occurred after the baseline. An older statement without intervening CSV
history can be staged as a checkpoint, but cannot be applied as though it were a
statement at the cutoff.

The review lists each document's role and retains original file/data-row or PDF
page/line references for proposed costs. Evidence confirmation and final Plaid
import approval remain separate. This batch flow only proposes aggregate opening
costs; it does not import the older activities themselves.

**Architecture impact:** Extend the existing in-memory review and format
adapters. Reuse the authenticated CSV parser, generation-based admission checks
and existing approval endpoint. No schema, stored documents, jobs, new worker
mechanism, provider setting or external service is added.

## Brokerage-specific formats

Choose the exact **Brokerage file format** before uploading. The registry in
`brokerage-basis-formats.ts` currently includes:

- **Merrill Edge — Settled Activity CSV v1.** Requires the complete, ordered
  Merrill header schema. Uses Trade Date and Settled status, checks a single
  account, reads the operation from Description 1 and treats Description 2 as
  the security description. PURCHASE quantities and prices must be positive; the
  acquisition basis is the net debit in Amount ($), including charges.
  Parenthesized debits are supported. The purchase check allows half the
  exported unit-price quantum (at least cent precision) times the exact share
  quantity, plus half a cent of cash rounding, with the previous one-cent
  minimum. The reported net debit remains the proposed acquisition basis; share
  quantities are never rounded. Explicit DIVIDEND/INTEREST/BANK INTEREST records
  with no quantity or price are identified separately as cash income, with their
  source rows displayed; they do not alter security cost basis. Settled
  FundTransfers/Withdrawal records are also recognized when the security
  identifier, quantity and price are blank or `--` and the cash amount is
  negative. These records are excluded from security cost calculations; this
  helper does not import or classify them as ledger payments. Other records,
  pending activity and sales without disposed cost basis stop normalization.
- **Merrill Edge — CMA monthly statement PDF v1.** Recognizes the CMA Assets
  section, printed statement period and account header. Uses the labeled
  Quantity and Cost Basis columns rather than Market Value, reads equities'
  symbol column and funds' SYMBOL continuation lines, and checks each section's
  printed total cost against the extracted positions. Repeated table headers and
  section totals on subsequent pages are supported. Normalized rows retain
  source page and text-line references. Cash holdings are not opening security
  positions. Other layouts, incomplete symbols, unknown basis and mismatching
  totals stop extraction.

These adapters were checked privately against the provided examples. Committed
regression records are independently invented; original customer files and
extracted financial values are not repository fixtures. Supporting this export
version does not establish that all Merrill exports use the same layout.

Brokerage modes lock the mappings and source text, and the PDF mode uses its
printed closing date. Unknown formats do not silently fall back to generic
mapping. **Manually mapped table (advanced)** is a separate explicit choice.
When bridging a starting statement with CSV activity, changing the format keeps
that reviewed starting checkpoint in memory and clears the current file and
proposals.

To add another firm or export version, supply representative documents, add a
separately identified registry entry and a parser with fixed recognition rules,
and add synthetic positive and negative fixtures. Cover header/layout changes,
unsupported records, account/currency scope, missing costs, continuation pages,
source provenance and reconciliation. Financial calculation and final import
approval remain shared; no provider API or database change is required.

A later statement cannot establish an earlier opening checkpoint. Format
recognition does not prove that the CSV covers every transaction between a
statement and the required cutoff; the existing history attestation and exact
position-quantity checks still apply.

## Required-interval CSV review

The brokerage normalizer receives the opening cutoff and, when supplied, the
latest verified statement date. It checks headers, row shape, account, settled
status and valid trade/settlement dates for every record before excluding rows
on/before that statement or after the cutoff. Only in-range rows undergo
operation and acquisition-basis interpretation. Outside records are disclosed by
count per file; they are not imported or added to a checkpoint. A standalone
file with no eligible records explains that an earlier statement and intervening
history are needed.

Positive `FundTransfers / FUNDS RECEIVED` records with blank security, quantity
and price are cash-only. `BANK INTEREST` follows the existing positive
cash-income checks. Invalid signs and security-bearing transfers remain
rejected. In-range sales, security transfers and transfer/adjustment records
give specific guidance for verified disposed/transferred basis or an eligible
later position statement; proceeds and current market values are never
substituted for cost basis.

## Manually mapped CSVs

Position example (enter the statement's verified end-of-day date):

```csv
Symbol,Quantity,Cost basis,Currency
EXAMPLE,10,501.25,USD
```

Alternatively map an explicitly labeled average unit cost. If both total and
unit costs are mapped, their difference must not exceed one cent. Duplicate
position identifiers require consolidation using verified records before upload.

Transaction example:

```csv
Symbol,Date,Action,Quantity,Cost basis,Price,Fee,Currency
EXAMPLE,2025-05-01,BUY,2,,20,1,USD
EXAMPLE,2025-05-02,SELL,1,20.5,,,USD
```

Only positive-quantity BUY and SELL actions are supported. BUY uses total
acquisition basis (including fees), or purchase price × quantity + explicit fees
(enter `0` only when fees are known to be zero). SELL requires the broker's
reported **disposed cost basis for the actual shares sold**, not the sale
proceeds. This avoids assuming FIFO when the broker used specific lots.

A transaction-only file must contain complete history from zero holdings.
Otherwise start with a verified position statement. Sales subtract reported
basis; buys add acquisition basis. The helper calculates aggregate remaining
basis divided by remaining quantity. Same-day rows use the source order.
Duplicate rows, negative positions/basis and inconsistent zero-share balances
block calculation. Unrecognized actions also block it: do not omit splits,
transfers, reinvestments or basis adjustments to force a result. Establish a
verified position checkpoint after such events instead.

## Explicitly unavailable statement basis

Merrill CMA statements default to rejecting a holding whose cost basis is
explicitly marked unavailable. The optional exclusion checkbox allows those
entries to be omitted from the uploaded evidence only. It never substitutes zero
basis or removes a tracked holding. Missing columns, malformed amounts, invalid
identities for included positions, duplicate included positions and inconsistent
known-basis section totals still block the document.

A fund continuation entry without a repeated description can use the preceding
verified symbol in the same section as its omission label. That label is used
only for disclosure, never to create a normalized position.

The review lists each omitted entry with its source file, page and line, and
retains that disclosure when a statement becomes a starting checkpoint for a
subsequent CSV. Changing the option clears the evidence and checkpoint and
requires another upload. Exclusions live only in the current review. A required
opening position still needs verified basis before final import approval.

## Architecture impact and validation

The helper reuses the shared CSV adapter and existing Plaid approval payload. It
adds no database tables, server endpoints, network providers, jobs or secret
handling. Decimal arithmetic is used for evidence calculations; the backend
remains authoritative for ledger reconciliation and persistence. PDF.js and its
worker are bundled locally and loaded only for PDF files. Extracted document
text is untrusted input and is displayed as text, never executed as HTML.

Tests cover exact quantity matching, source provenance, monthly statement/CSV
bridging, overlap exclusion, fee inclusion, actual disposed basis, unknown and
conflicting costs, duplicates, unsupported events, parser warnings/failures,
file/PDF limits, worker cleanup, and keeping evidence confirmation separate from
final import approval. Synthetic browser checks exercise actual PDF extraction;
financial documents from a real institution are not test fixtures.

## Share movements and incoming-transfer costs

Opening quantities are reconstructed from the signed buy, sell and in-kind
transfer quantities across the returned history. This quantity preview is
independent of execution-price validation: inconsistent trade values still block
approval, but do not silently remove a known share change from the preview.
Ordinary equity/ETF dispositions whose provider subtype is `exercise` are
validated as sales; unsupported instruments remain blocked.

The price check permits bounded rounding based on reported unit-price precision
and exact share quantity. For an accepted trade, effective unit price is derived
from reported net cash and explicit fees so book cost and cash stay consistent.
Amounts beyond that bound require source verification; they are not accepted as
an arbitrary price correction.

In-kind transfers create security `TRANSFER_IN`/`TRANSFER_OUT` activities. Their
reported market value is not cash and is never used as book cost. Explicit fees
become separate cash-fee activities so they neither use market value nor change
the verified transferred book cost. Incoming transfers require a verified unit
book cost for each source record before approval. Reviewed costs are retained in
the existing investment-ledger metadata for repeat syncs; new transfers need new
review, and previously reviewed costs cannot silently change. A provider
correction to a reviewed transfer’s quantity, security, date or fees blocks
reuse of its saved cost and requires source reconciliation. Outgoing transfers
use the portfolio engine's existing lot-relief logic. These records are not
automatically paired with another account's transfer leg.

Excluding an unavailable-basis statement entry removes evidence only. If it is a
sub-position of a holding, its shares still belong in the full opening quantity.
A partial known-basis row normally cannot be assigned to a larger holding. For
an explicitly excluded same-security remainder of at most 0.0001 share and 0.01%
of the required position, the review can reconcile the full quantity while
preserving the reported total cost. The excluded fraction adds zero cost; unit
cost is recalculated as reported total divided by the full opening quantity,
using the existing supported precision. The review displays this adjustment
before applying it. Only exclusions from the active statement or its retained
checkpoint can explain the difference; superseded statements, unrelated
securities, unexplained differences and larger omissions remain blocked. The
holding quantity is never reduced, and final import approval remains separate.

**Architecture impact:** Shared Rust conversion and the existing review/approval
path own these changes. The approval DTO gains an optional `transferCosts` map;
legacy requests deserialize with an empty map. `InvestmentLedger` gains a
backward-compatible defaulted map of reviewed incoming-transfer unit costs. No
table migration, new endpoint, provider call, background job or secret storage
is added. SQLite approval and real-engine reconciliation remain atomic. Native
Plaid linking remains explicitly unsupported.
