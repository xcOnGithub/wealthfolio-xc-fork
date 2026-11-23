import { MERRILL_ACTIVITY_HEADERS } from "../merrill-basis-csv";
export {
  atRight,
  merrillPdfFixture,
  pdfItem,
  positionHeader,
  statementHeader,
} from "@/features/statement-history/test-fixtures/merrill-pdf";

// Entirely invented records. No customer account, security or amount is copied.
export function merrillCsvFixture() {
  return {
    headers: [...MERRILL_ACTIVITY_HEADERS],
    rows: [
      [
        "5/1/2025",
        "5/2/2025",
        "Settled",
        "Synthetic account",
        "Synthetic owner",
        "SYNTH-01",
        "DividendAndInterest",
        "DIVIDEND",
        "Synthetic issuer",
        "DEMO",
        "--",
        "--",
        "2.00",
      ],
      [
        "5/2/2025",
        "5/5/2025",
        "Settled",
        "Synthetic account",
        "Synthetic owner",
        "SYNTH-01",
        "SecurityTransactions",
        "PURCHASE",
        "Synthetic equity",
        "DEMO",
        "2",
        "10",
        "(21.00)",
      ],
    ],
  };
}
