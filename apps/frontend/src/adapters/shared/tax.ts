import type { TaxExportFormat, TaxPacketExport, TaxYearReport } from "@/lib/types";

import { invoke } from "./platform";

export const getTaxYearReport = async (
  taxYear: number,
  memberName?: string | null,
): Promise<TaxYearReport> => {
  return invoke<TaxYearReport>("get_tax_year_report", {
    taxYear,
    memberName: memberName ?? null,
  });
};

export const exportTaxPacket = async (
  taxYear: number,
  format: TaxExportFormat,
  memberName?: string | null,
): Promise<TaxPacketExport> => {
  return invoke<TaxPacketExport>("export_tax_packet", {
    taxYear,
    format,
    memberName: memberName ?? null,
  });
};
