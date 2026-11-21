import { exportTaxPacket, getTaxYearReport } from "@/adapters";
import { QueryKeys } from "@/lib/query-keys";
import type { TaxExportFormat, TaxYearReport } from "@/lib/types";
import { useQuery } from "@tanstack/react-query";

export function useTaxYearReport(taxYear: number, memberName?: string | null) {
  return useQuery<TaxYearReport, Error>({
    queryKey: [QueryKeys.TAX_REPORT, taxYear, memberName ?? "__all__"],
    queryFn: () => getTaxYearReport(taxYear, memberName),
  });
}

export async function downloadTaxPacket(
  taxYear: number,
  format: TaxExportFormat,
  memberName?: string | null,
): Promise<string> {
  const packet = await exportTaxPacket(taxYear, format, memberName);
  const blob = new Blob([packet.content], { type: packet.contentType });
  const url = URL.createObjectURL(blob);
  try {
    const link = document.createElement("a");
    link.href = url;
    link.download = packet.filename;
    document.body.appendChild(link);
    link.click();
    link.remove();
  } finally {
    URL.revokeObjectURL(url);
  }
  return packet.filename;
}
