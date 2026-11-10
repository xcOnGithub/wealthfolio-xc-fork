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

1. Upload a file (up to 10 MB and 100 PDF pages).
2. Review the extracted text against the document. Keep one account's table;
   remove prose, totals, cash lines and repeated page headers. Correct any
   merged PDF columns. Choose the header row and read the table.
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

## Supported CSVs

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
