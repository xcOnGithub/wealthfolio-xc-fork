import type { PdfTextItem, PdfBasisReview } from "./basis-pdf";
import { extractMerrillStatement } from "./merrill-basis-pdf";
import { extractMerrillActivity } from "@/features/statement-history/merrill-activity";
export type {
  StatementSection,
  StatementSource,
  StatementActivity,
  StatementDisposition,
  StatementEvidence,
} from "@/features/statement-history/merrill-activity";

/** Preserve the cost-review contract while sharing activity extraction with history. */
export function extractMerrillStatementV2(
  pages: PdfTextItem[][],
  onAccount?: (id: string) => void,
  review?: PdfBasisReview,
) {
  let account = "";
  let positions: { security: string; description: string }[] = [];
  const positionsCsv = extractMerrillStatement(
    pages,
    (id) => {
      account = id;
    },
    review,
    (rows) => {
      positions = rows;
    },
  );
  const evidence = extractMerrillActivity(pages, positions);
  onAccount?.(account);
  return { positionsCsv, evidence };
}
