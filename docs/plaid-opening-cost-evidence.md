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
   Quantities must match exactly. Matching quantities alone do not establish
   security identity or account ownership.
5. Confirm the evidence and fill the proposed opening costs. Review any
   remaining manual costs and confirm the original investment import separately.

Statement market value, current price, sale proceeds, buying power and cash
balance are **not** cost basis. They are never automatically mapped as such.
Unknown basis remains unknown; it is not silently converted to zero.

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
sequentially, with each PDF worker destroyed before the next document. Raw file
objects are not retained in React state. Only normalized records, source
references and the reviewed opening checkpoint remain in memory. Closing or
refreshing the review clears this state.

All files in a batch must identify the same account. Account numbers are
compared transiently by the parsers and are not included in normalized rows or
batch results. Unsupported records, malformed files or a failed file stop the
whole batch; preceding files are never partially applied. File/row references
identify the error without displaying a parser's raw exception.

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
Use non-overlapping exports or resolve the records explicitly. Cash-only income
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
  Parenthesized debits are supported. Explicit DIVIDEND/INTEREST records with no
  quantity or price are identified separately as cash income, with their source
  rows displayed; they do not alter security cost basis. Other records, pending
  activity and sales without disposed cost basis stop normalization.
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
