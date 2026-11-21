//! RFC 4180 CSV and JSON Tax Packet serializers with formula-injection protection.

use rust_decimal::Decimal;

use crate::accounts::TaxTreatment;
use crate::errors::Result;

use super::model::{
    DisposalKind, HoldingTerm, TaxExportFormat, TaxPacketExport, TaxYearReport, WashSaleSeverity,
};

/// Escapes a text cell per RFC 4180 and neutralizes spreadsheet formula injection
/// (including leading whitespace before `=`, `+`, `-`, `@`, `\t`, `\r`).
pub fn csv_text_cell(raw: &str) -> String {
    let trimmed_start = raw.trim_start();
    let needs_prefix = trimmed_start
        .chars()
        .next()
        .is_some_and(|ch| matches!(ch, '=' | '+' | '-' | '@' | '\t' | '\r'))
        || raw.starts_with('\t')
        || raw.starts_with('\r');

    let safe = if needs_prefix {
        format!("'{raw}")
    } else {
        raw.to_string()
    };

    if safe.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", safe.replace('"', "\"\""))
    } else {
        safe
    }
}

/// Formats a numeric `Decimal` cell without formula-injection prefixing (so negative
/// losses like `-123.45` remain valid numeric cells in Excel/CPA software).
pub fn csv_decimal_cell(value: Decimal) -> String {
    value.normalize().to_string()
}

fn csv_opt_decimal_cell(value: Option<Decimal>) -> String {
    value.map(csv_decimal_cell).unwrap_or_default()
}

fn tax_treatment_label(treatment: Option<TaxTreatment>) -> &'static str {
    match treatment {
        Some(TaxTreatment::Taxable) => "TAXABLE",
        Some(TaxTreatment::TaxDeferred) => "TAX_DEFERRED",
        Some(TaxTreatment::TaxFree) => "TAX_FREE",
        None => "UNCLASSIFIED",
    }
}

fn term_label(term: HoldingTerm) -> &'static str {
    match term {
        HoldingTerm::ShortTerm => "SHORT_TERM",
        HoldingTerm::LongTerm => "LONG_TERM",
        HoldingTerm::Unknown => "UNKNOWN",
    }
}

fn kind_label(kind: DisposalKind) -> &'static str {
    match kind {
        DisposalKind::Sale => "SALE",
        DisposalKind::ShortCover => "SHORT_COVER",
        DisposalKind::OptionExpiry => "OPTION_EXPIRY",
        DisposalKind::ReturnOfCapitalExcess => "RETURN_OF_CAPITAL_EXCESS",
    }
}

fn wash_sale_label(risk: Option<WashSaleSeverity>) -> &'static str {
    match risk {
        Some(WashSaleSeverity::IraPermanentDisallowance) => "IRA_PERMANENT_DISALLOWANCE",
        Some(WashSaleSeverity::CrossAccount) => "CROSS_ACCOUNT",
        Some(WashSaleSeverity::SameAccount) => "SAME_ACCOUNT",
        None => "NONE",
    }
}

fn sanitize_filename_segment(value: &str) -> String {
    let cleaned: String = value
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let trimmed = cleaned.trim_matches('-');
    if trimmed.is_empty() {
        "member".to_string()
    } else {
        trimmed.chars().take(32).collect()
    }
}

pub fn build_tax_packet_export(
    report: &TaxYearReport,
    format: TaxExportFormat,
) -> Result<TaxPacketExport> {
    let scope = report
        .member_filter
        .as_deref()
        .map(sanitize_filename_segment)
        .unwrap_or_else(|| "family".to_string());
    let year = report.tax_year;

    match format {
        TaxExportFormat::RealizedGainsCsv => {
            let mut lines = Vec::with_capacity(report.realized_gains.disposals.len() + 1);
            lines.push(
                [
                    "Symbol",
                    "Description",
                    "Account",
                    "Owner",
                    "Tax Treatment",
                    "Disposal Kind",
                    "Date Acquired",
                    "Date Sold",
                    "Holding Days",
                    "Term",
                    "Quantity",
                    "Proceeds (Base)",
                    "Cost Basis (Base)",
                    "Realized Gain/Loss (Base)",
                    "Base Currency",
                    "Cost Basis Method",
                    "Wash Sale Risk (Advisory)",
                    "Est. Disallowed Loss (Base)",
                ]
                .join(","),
            );
            for row in &report.realized_gains.disposals {
                let open_date = row.open_date.map(|d| d.to_string()).unwrap_or_default();
                let holding_days = row.holding_days.map(|d| d.to_string()).unwrap_or_default();
                lines.push(
                    [
                        csv_text_cell(&row.symbol),
                        csv_text_cell(row.asset_name.as_deref().unwrap_or(&row.symbol)),
                        csv_text_cell(&row.account_name),
                        csv_text_cell(row.owner_name.as_deref().unwrap_or("")),
                        csv_text_cell(tax_treatment_label(row.tax_treatment)),
                        csv_text_cell(kind_label(row.kind)),
                        csv_text_cell(&open_date),
                        csv_text_cell(&row.disposal_date.to_string()),
                        holding_days,
                        csv_text_cell(term_label(row.term)),
                        csv_decimal_cell(row.quantity),
                        csv_opt_decimal_cell(row.proceeds_base),
                        csv_opt_decimal_cell(row.cost_basis_base),
                        csv_opt_decimal_cell(row.realized_pnl_base),
                        csv_text_cell(&report.base_currency),
                        csv_text_cell(&row.cost_basis_method),
                        csv_text_cell(wash_sale_label(row.wash_sale_risk)),
                        csv_opt_decimal_cell(row.estimated_disallowed_loss_base),
                    ]
                    .join(","),
                );
            }
            Ok(TaxPacketExport {
                filename: format!("wealthfolio-tax-{year}-{scope}-realized-gains.csv"),
                content_type: "text/csv;charset=utf-8".to_string(),
                content: lines.join("\r\n"),
            })
        }
        TaxExportFormat::IncomeCsv => {
            let mut lines = Vec::with_capacity(report.income.by_account.len() + 1);
            lines.push(
                [
                    "Account",
                    "Owner",
                    "Tax Treatment",
                    "Gross Dividends (Base)",
                    "Gross Interest (Base)",
                    "Other Taxable Income (Base)",
                    "Return of Capital (Base)",
                    "Income Tax Withheld (Base)",
                    "Income Fees (Base)",
                    "Net Investment Income (Base)",
                    "Standalone Taxes Paid (Base)",
                    "Base Currency",
                ]
                .join(","),
            );
            for row in &report.income.by_account {
                lines.push(
                    [
                        csv_text_cell(&row.account_name),
                        csv_text_cell(row.owner_name.as_deref().unwrap_or("")),
                        csv_text_cell(tax_treatment_label(row.tax_treatment)),
                        csv_decimal_cell(row.totals.gross_dividends_base),
                        csv_decimal_cell(row.totals.gross_interest_base),
                        csv_decimal_cell(row.totals.gross_other_income_base),
                        csv_decimal_cell(row.totals.return_of_capital_base),
                        csv_decimal_cell(row.totals.income_withholding_tax_base),
                        csv_decimal_cell(row.totals.income_fees_base),
                        csv_decimal_cell(row.totals.net_investment_income_base),
                        csv_decimal_cell(row.totals.standalone_tax_paid_base),
                        csv_text_cell(&report.base_currency),
                    ]
                    .join(","),
                );
            }
            Ok(TaxPacketExport {
                filename: format!("wealthfolio-tax-{year}-{scope}-income.csv"),
                content_type: "text/csv;charset=utf-8".to_string(),
                content: lines.join("\r\n"),
            })
        }
        TaxExportFormat::ContributionLimitsCsv => {
            let mut lines = Vec::new();
            lines.push(
                [
                    "Member",
                    "Category",
                    "Group Name",
                    "Year",
                    "Contributed (Base)",
                    "Limit Amount",
                    "Remaining",
                    "Over Contribution",
                    "Deadline",
                    "Recurring",
                    "Projected",
                    "Base Currency",
                ]
                .join(","),
            );
            for group in &report.contribution_limits.by_member {
                for row in &group.limits {
                    lines.push(
                        [
                            csv_text_cell(row.member_name.as_deref().unwrap_or("Household")),
                            csv_text_cell(row.limit_category.as_deref().unwrap_or("")),
                            csv_text_cell(&row.group_name),
                            row.contribution_year.to_string(),
                            csv_decimal_cell(row.contributed_base),
                            csv_decimal_cell(row.limit_amount),
                            csv_decimal_cell(row.remaining_amount),
                            csv_decimal_cell(row.over_contribution_amount),
                            csv_text_cell(&row.deadline_date.to_string()),
                            if row.is_recurring { "true" } else { "false" }.to_string(),
                            if row.is_projected { "true" } else { "false" }.to_string(),
                            csv_text_cell(&report.base_currency),
                        ]
                        .join(","),
                    );
                }
            }
            Ok(TaxPacketExport {
                filename: format!("wealthfolio-tax-{year}-{scope}-contribution-limits.csv"),
                content_type: "text/csv;charset=utf-8".to_string(),
                content: lines.join("\r\n"),
            })
        }
        TaxExportFormat::FullJson => {
            let content = serde_json::to_string_pretty(report)?;
            Ok(TaxPacketExport {
                filename: format!("wealthfolio-tax-{year}-{scope}-packet.json"),
                content_type: "application/json;charset=utf-8".to_string(),
                content,
            })
        }
    }
}
