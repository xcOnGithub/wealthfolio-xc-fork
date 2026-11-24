//! ISO-backed exchange registry with Wealthfolio-specific enrichments.
//!
//! `iso10383.json` is the complete identity reference. `exchanges.json` only
//! adds application metadata and verified provider symbol rules. An exchange
//! therefore remains selectable even when Wealthfolio has no pricing rule for it.

use std::collections::{HashMap, HashSet};

use lazy_static::lazy_static;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub(crate) struct ExchangeCatalog {
    pub exchanges: Vec<ExchangeEntry>,
    #[serde(default)]
    pub legacy_exchanges: Vec<ExchangeEntry>,
    pub currency_priority: HashMap<String, Vec<String>>,
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
pub struct ExchangeEntry {
    pub mic: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub long_name: Option<String>,
    #[serde(default)]
    pub currency: Option<String>,
    #[serde(default)]
    pub timezone: Option<String>,
    #[serde(default)]
    pub close: Option<[u8; 2]>,
    #[serde(default)]
    pub providers: HashMap<String, ProviderRule>,
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
pub struct ProviderRule {
    /// The single deterministic suffix for this MIC on the provider. Venues
    /// with instrument-specific alternatives use an asset provider override;
    /// listing multiple suffixes here would not say which one to choose.
    #[serde(default)]
    pub suffix: Option<String>,
    /// The provider addresses this MIC with an unsuffixed symbol.
    #[serde(default)]
    pub bare: bool,
    #[serde(default)]
    pub currency: Option<String>,
    #[serde(default)]
    pub codes: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IsoCatalog {
    source_url: String,
    source_sha256: String,
    publication_date: String,
    implementation_date: String,
    records: Vec<IsoMicEntry>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IsoMicEntry {
    mic: String,
    operating_mic: String,
    kind: String,
    name: String,
    #[serde(default)]
    acronym: Option<String>,
    country_code: String,
    status: String,
}

/// Exchange data exposed to the desktop and web clients.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExchangeInfo {
    pub mic: String,
    pub operating_mic: String,
    pub mic_type: String,
    pub name: String,
    pub long_name: String,
    pub country_code: String,
    pub status: String,
    pub currency: Option<String>,
    pub configured: bool,
}

/// Old values written by Wealthfolio before the registry used canonical MICs.
/// These aliases are accepted on read and normalized on every new write.
const LEGACY_MIC_ALIASES: &[(&str, &str)] = &[
    ("CXE", "BCXE"),
    ("DXE", "CCXE"),
    ("XAQE", "AQSE"),
    ("XNEO", "NEOE"),
    ("XTAI_OTC", "ROCO"),
];

/// Explicit provider-reported venue names and aliases mapped to canonical ISO 10383 MICs.
/// Keeps US Nasdaq tiers (`XNAS`) cleanly separated from foreign Nasdaq venues (`XSTO`, `XCSE`, etc.).
const EXPLICIT_EXCHANGE_LABEL_ALIASES: &[(&str, &str)] = &[
    // US Nasdaq tiers & Finnhub labels
    ("NASDAQ NMS - GLOBAL MARKET", "XNAS"),
    ("NASDAQ - GLOBAL MARKET", "XNAS"),
    ("NASDAQ - GLOBAL SELECT MARKET", "XNAS"),
    ("NASDAQ - CAPITAL MARKET", "XNAS"),
    ("NASDAQ NMS", "XNAS"),
    ("NASDAQ GLOBAL SELECT", "XNAS"),
    // US NYSE / Arca / American / BATS / IEX / OTC
    ("NEW YORK STOCK EXCHANGE INC.", "XNYS"),
    ("NYSE ARCA, INC.", "ARCX"),
    ("NYSE ARCA INC.", "ARCX"),
    ("NYSE MKT LLC", "XASE"),
    ("NYSE MKT", "XASE"),
    ("NYSE AMERICAN, LLC", "XASE"),
    ("AMEX", "XASE"),
    ("BATS GLOBAL MARKETS", "BATS"),
    ("CBOE BZX", "BATS"),
    ("IEX", "IEXG"),
    ("OTC LINK", "OTCM"),
    ("PINK SHEETS", "OTCM"),
    // Foreign Nasdaq venues (explicitly distinguished from US XNAS)
    ("NASDAQ STOCKHOLM", "XSTO"),
    ("NASDAQ OMX STOCKHOLM", "XSTO"),
    ("NASDAQ OMX NORDIC STOCKHOLM", "XSTO"),
    ("NASDAQ COPENHAGEN", "XCSE"),
    ("NASDAQ OMX COPENHAGEN", "XCSE"),
    ("NASDAQ HELSINKI", "XHEL"),
    ("NASDAQ OMX HELSINKI", "XHEL"),
    ("NASDAQ ICELAND", "XICE"),
    ("NASDAQ OMX ICELAND", "XICE"),
    ("NASDAQ TALLINN", "XTAL"),
    ("NASDAQ OMX TALLINN", "XTAL"),
    ("NASDAQ RIGA", "XRIS"),
    ("NASDAQ OMX RIGA", "XRIS"),
    ("NASDAQ VILNIUS", "XLIT"),
    ("NASDAQ OMX VILNIUS", "XLIT"),
    ("NASDAQ OSLO", "NORX"),
    ("NASDAQ DUBAI", "DIFX"),
    // Common global exchange labels reported by providers
    ("DEUTSCHE BOERSE XETRA", "XETR"),
    ("BOERSE FRANKFURT", "XFRA"),
    ("FRANKFURT STOCK EXCHANGE", "XFRA"),
    ("SIX SWISS EXCHANGE", "XSWX"),
    ("HONG KONG EXCHANGES AND CLEARING LTD", "XHKG"),
    ("HONG KONG STOCK EXCHANGE", "XHKG"),
    ("AUSTRALIAN SECURITIES EXCHANGE", "XASX"),
    ("ASX", "XASX"),
    ("TOKYO STOCK EXCHANGE", "XTKS"),
];

fn normalize_exchange_label_key(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_uppercase()
}

fn canonical_mic_value(value: &str) -> String {
    let normalized = value.trim().to_ascii_uppercase();
    LEGACY_MIC_ALIASES
        .iter()
        .find_map(|(legacy, canonical)| (*legacy == normalized).then_some(*canonical))
        .unwrap_or(normalized.as_str())
        .to_string()
}

/// Normalize casing and replace identifiers previously invented by Wealthfolio.
/// Unknown values are preserved so a newer MIC is not rejected by an older app.
pub fn canonicalize_exchange_mic(value: &str) -> String {
    canonical_mic_value(value)
}

/// Whether the bundled ISO snapshot recognizes this MIC.
pub fn is_known_mic(value: &str) -> bool {
    REGISTRY
        .iso_by_mic
        .contains_key(&canonical_mic_value(value))
}

/// Whether the bundled ISO snapshot classifies this MIC under the United States (`US`).
pub fn is_us_exchange_mic(value: &str) -> bool {
    REGISTRY
        .iso_by_mic
        .get(&canonical_mic_value(value))
        .is_some_and(|entry| entry.country_code.eq_ignore_ascii_case("US"))
}

/// Return the ISO 10383 operating MIC for `value`, if recognized in the bundled snapshot.
pub fn operating_exchange_mic(value: &str) -> Option<String> {
    REGISTRY
        .iso_by_mic
        .get(&canonical_mic_value(value))
        .map(|entry| entry.operating_mic.clone())
}

/// Map a MIC to its configured entry in `exchanges.json`, falling back to its
/// ISO 10383 operating MIC when the segment itself has no separate provider rule
/// (e.g. `XNGS` -> `XNAS`, while `ARCX` stays `ARCX`).
pub fn configured_exchange_mic(value: &str) -> Option<String> {
    let canonical = canonical_mic_value(value);
    if REGISTRY.curated_mics.contains(&canonical) {
        return Some(canonical);
    }
    let entry = REGISTRY.iso_by_mic.get(&canonical)?;
    REGISTRY
        .curated_mics
        .contains(&entry.operating_mic)
        .then(|| entry.operating_mic.clone())
}

/// Whether two US exchange MICs describe compatible listings — either identical, or
/// one is a configured operating-level MIC (`OPRT`, e.g. `XNAS`) and the other is an
/// unconfigured segment of that exchange (`SGMT`, e.g. `XNGS`) that relies on the
/// operating venue's provider route, while preserving both separately configured
/// venues (e.g. `XNYS` vs `ARCX`) and distinct segments (e.g. `XNGS` vs `XNCM`).
pub fn are_compatible_us_exchange_mics(left: &str, right: &str) -> bool {
    reconcile_us_exchange_mics(left, right).is_some()
}

/// Reconcile a requested US exchange MIC with a provider-reported US exchange MIC,
/// returning the more specific verified MIC (segment over operating MIC) if compatible,
/// or `None` if non-US, unrecognized, separately configured, or conflicting.
pub fn reconcile_us_exchange_mics(requested_mic: &str, provider_mic: &str) -> Option<String> {
    let req = canonical_mic_value(requested_mic);
    let prov = canonical_mic_value(provider_mic);
    let req_iso = REGISTRY.iso_by_mic.get(&req)?;
    let prov_iso = REGISTRY.iso_by_mic.get(&prov)?;
    if !req_iso.country_code.eq_ignore_ascii_case("US")
        || !prov_iso.country_code.eq_ignore_ascii_case("US")
    {
        return None;
    }
    if req == prov {
        return Some(req);
    }
    if req_iso.operating_mic == prov_iso.operating_mic {
        if prov_iso.kind == "OPRT"
            && REGISTRY.curated_mics.contains(&prov)
            && !REGISTRY.curated_mics.contains(&req)
        {
            return Some(req);
        }
        if req_iso.kind == "OPRT"
            && REGISTRY.curated_mics.contains(&req)
            && !REGISTRY.curated_mics.contains(&prov)
        {
            return Some(prov);
        }
    }
    None
}

/// Resolve a provider-reported exchange code, MIC, or known venue name/alias
/// into a canonical ISO 10383 MIC. Returns `None` for empty or unfamiliar labels.
pub fn resolve_exchange_label_to_mic(value: &str) -> Option<String> {
    let key = normalize_exchange_label_key(value);
    if key.is_empty() {
        return None;
    }
    if let Some(mic) = REGISTRY.yahoo_code_to_mic.get(&key) {
        return Some(mic.clone());
    }
    REGISTRY.label_to_mic.get(&key).cloned()
}

/// Return every current MIC. Curated data enriches an ISO record but is not a
/// prerequisite for the venue to appear.
pub fn get_exchange_list() -> Vec<ExchangeInfo> {
    REGISTRY.exchange_list.clone()
}

fn leak_str(value: String) -> &'static str {
    Box::leak(value.into_boxed_str())
}

pub(crate) struct ExchangeRegistry {
    pub catalog: ExchangeCatalog,
    iso_by_mic: HashMap<String, IsoMicEntry>,
    curated_mics: HashSet<String>,
    exchange_list: Vec<ExchangeInfo>,
    pub name_by_mic: HashMap<String, &'static str>,
    pub currency_by_mic: HashMap<String, &'static str>,
    pub timezone_by_mic: HashMap<String, &'static str>,
    pub close_by_mic: HashMap<String, (u8, u8)>,
    pub currency_priority_slices: HashMap<&'static str, &'static [&'static str]>,
    pub yahoo_code_to_mic: HashMap<String, String>,
    pub label_to_mic: HashMap<String, String>,
    pub yahoo_suffix_to_mic: HashMap<String, &'static str>,
    pub yahoo_suffixes: &'static [&'static str],
}

lazy_static! {
    pub(crate) static ref REGISTRY: ExchangeRegistry = ExchangeRegistry::load();
}

impl ExchangeRegistry {
    fn load() -> Self {
        let iso: IsoCatalog = serde_json::from_str(include_str!("iso10383.json"))
            .expect("iso10383.json must be valid");
        assert!(
            iso.source_url.starts_with("https://www.iso20022.org/"),
            "iso10383.json must identify its official source"
        );
        assert!(
            iso.source_sha256.len() == 64
                && iso
                    .source_sha256
                    .chars()
                    .all(|character| character.is_ascii_hexdigit()),
            "iso10383.json must identify the downloaded CSV content"
        );
        assert_eq!(iso.publication_date.len(), 10);
        assert_eq!(iso.implementation_date.len(), 10);
        let catalog: ExchangeCatalog = serde_json::from_str(include_str!("exchanges.json"))
            .expect("exchanges.json must be valid");

        let iso_by_mic: HashMap<String, IsoMicEntry> = iso
            .records
            .iter()
            .cloned()
            .map(|entry| (entry.mic.clone(), entry))
            .collect();
        assert_eq!(
            iso_by_mic.len(),
            iso.records.len(),
            "iso10383.json must not contain duplicate MICs"
        );

        let curated_by_mic: HashMap<&str, &ExchangeEntry> = catalog
            .exchanges
            .iter()
            .map(|entry| (entry.mic.as_str(), entry))
            .collect();
        assert_eq!(
            curated_by_mic.len(),
            catalog.exchanges.len(),
            "exchanges.json must not contain duplicate MICs"
        );
        for entry in &catalog.exchanges {
            assert!(
                iso_by_mic.contains_key(&entry.mic),
                "exchanges.json key '{}' is not in the bundled ISO 10383 snapshot",
                entry.mic
            );
        }
        for entry in catalog
            .exchanges
            .iter()
            .chain(catalog.legacy_exchanges.iter())
        {
            for (provider_id, rule) in &entry.providers {
                assert!(
                    !provider_id.is_empty() && provider_id == &provider_id.to_ascii_uppercase(),
                    "provider IDs in exchanges.json must be non-empty uppercase values"
                );
                let has_suffix = rule
                    .suffix
                    .as_deref()
                    .is_some_and(|suffix| !suffix.trim().is_empty());
                assert!(
                    rule.bare ^ has_suffix,
                    "{} / {} must define exactly one of bare=true or a non-empty suffix",
                    entry.mic,
                    provider_id
                );
                if let Some(suffix) = rule
                    .suffix
                    .as_deref()
                    .filter(|suffix| !suffix.trim().is_empty())
                {
                    assert!(
                        suffix.starts_with('.'),
                        "{} / {} suffix must start with a dot",
                        entry.mic,
                        provider_id
                    );
                }
            }
        }
        for mics in catalog.currency_priority.values() {
            for mic in mics {
                assert!(
                    iso_by_mic.contains_key(&canonical_mic_value(mic)),
                    "currency priority MIC '{}' is not in the ISO snapshot",
                    mic
                );
            }
        }

        let mut name_by_mic = HashMap::new();
        for entry in &iso.records {
            let name = entry
                .acronym
                .as_ref()
                .filter(|name| !name.trim().is_empty())
                .unwrap_or(&entry.name);
            name_by_mic.insert(entry.mic.clone(), leak_str(name.clone()));
        }

        let mut currency_by_mic = HashMap::new();
        let mut timezone_by_mic = HashMap::new();
        let mut close_by_mic = HashMap::new();
        for entry in catalog
            .exchanges
            .iter()
            .chain(catalog.legacy_exchanges.iter())
        {
            if let Some(name) = &entry.name {
                name_by_mic.insert(entry.mic.clone(), leak_str(name.clone()));
            }
            if let Some(currency) = &entry.currency {
                currency_by_mic.insert(entry.mic.clone(), leak_str(currency.clone()));
            }
            if let Some(timezone) = &entry.timezone {
                timezone_by_mic.insert(entry.mic.clone(), leak_str(timezone.clone()));
            }
            if let Some(close) = entry.close {
                close_by_mic.insert(entry.mic.clone(), (close[0], close[1]));
            }
        }

        let mut exchange_list: Vec<ExchangeInfo> = iso
            .records
            .iter()
            .filter(|entry| matches!(entry.status.as_str(), "ACTIVE" | "UPDATED"))
            .map(|entry| {
                let curated = curated_by_mic.get(entry.mic.as_str()).copied();
                let short_name = curated
                    .and_then(|item| item.name.clone())
                    .or_else(|| entry.acronym.clone())
                    .filter(|name| !name.trim().is_empty())
                    .unwrap_or_else(|| entry.mic.clone());
                let long_name = curated
                    .and_then(|item| item.long_name.clone())
                    .unwrap_or_else(|| entry.name.clone());
                ExchangeInfo {
                    mic: entry.mic.clone(),
                    operating_mic: entry.operating_mic.clone(),
                    mic_type: entry.kind.clone(),
                    name: short_name,
                    long_name,
                    country_code: entry.country_code.clone(),
                    status: entry.status.clone(),
                    currency: curated.and_then(|item| item.currency.clone()),
                    configured: curated.is_some(),
                }
            })
            .collect();
        exchange_list.sort_by(|left, right| {
            right
                .configured
                .cmp(&left.configured)
                .then_with(|| left.long_name.cmp(&right.long_name))
                .then_with(|| left.mic.cmp(&right.mic))
        });

        let mut yahoo_code_to_mic: HashMap<String, String> = iso
            .records
            .iter()
            .map(|entry| (entry.mic.clone(), entry.mic.clone()))
            .collect();
        for entry in &catalog.exchanges {
            if let Some(yahoo) = entry.providers.get("YAHOO") {
                for code in &yahoo.codes {
                    let key = code.trim().to_ascii_uppercase();
                    if !key.is_empty() {
                        yahoo_code_to_mic.insert(key, entry.mic.clone());
                    }
                }
            }
        }
        for (legacy, canonical) in LEGACY_MIC_ALIASES {
            yahoo_code_to_mic.insert((*legacy).to_string(), (*canonical).to_string());
        }

        let mut suffix_to_mic: HashMap<String, &'static str> = HashMap::new();
        let mut ambiguous_suffixes = HashSet::new();
        let mut suffix_set = Vec::new();
        for entry in &catalog.exchanges {
            let Some(suffix) = entry
                .providers
                .get("YAHOO")
                .and_then(|provider| provider.suffix.as_ref())
                .filter(|suffix| !suffix.is_empty())
            else {
                continue;
            };
            let suffix_key = suffix.trim_start_matches('.').to_ascii_uppercase();
            if !ambiguous_suffixes.contains(&suffix_key) {
                if let Some(existing_mic) = suffix_to_mic.get(&suffix_key) {
                    if !existing_mic.eq_ignore_ascii_case(&entry.mic) {
                        suffix_to_mic.remove(&suffix_key);
                        ambiguous_suffixes.insert(suffix_key.clone());
                    }
                } else {
                    suffix_to_mic.insert(suffix_key, leak_str(entry.mic.clone()));
                }
            }
            suffix_set.push(suffix.clone());
        }
        suffix_set.sort();
        suffix_set.dedup();
        let yahoo_suffixes = Box::leak(
            suffix_set
                .into_iter()
                .map(leak_str)
                .collect::<Vec<_>>()
                .into_boxed_slice(),
        );

        let mut currency_priority_slices = HashMap::new();
        for (currency, mics) in &catalog.currency_priority {
            let canonical_mics: Vec<&'static str> = mics
                .iter()
                .map(|mic| leak_str(canonical_mic_value(mic)))
                .collect();
            currency_priority_slices.insert(
                leak_str(currency.clone()),
                Box::leak(canonical_mics.into_boxed_slice()) as &'static [&'static str],
            );
        }

        let mut label_to_mic: HashMap<String, String> = HashMap::new();
        let mut ambiguous_labels: HashSet<String> = HashSet::new();
        for entry in &iso.records {
            let key = normalize_exchange_label_key(&entry.name);
            if key.is_empty() || ambiguous_labels.contains(&key) {
                continue;
            }
            let target_mic = if curated_by_mic.contains_key(entry.mic.as_str()) {
                &entry.mic
            } else if curated_by_mic.contains_key(entry.operating_mic.as_str()) {
                &entry.operating_mic
            } else {
                &entry.mic
            };
            if let Some(existing) = label_to_mic.get(&key) {
                if existing != target_mic {
                    label_to_mic.remove(&key);
                    ambiguous_labels.insert(key);
                }
            } else {
                label_to_mic.insert(key, target_mic.clone());
            }
        }
        for entry in &catalog.exchanges {
            for candidate in [entry.name.as_deref(), entry.long_name.as_deref()]
                .into_iter()
                .flatten()
            {
                let key = normalize_exchange_label_key(candidate);
                if !key.is_empty() {
                    label_to_mic.insert(key, entry.mic.clone());
                }
            }
        }
        for (alias, mic) in EXPLICIT_EXCHANGE_LABEL_ALIASES {
            assert!(
                iso_by_mic.contains_key(*mic),
                "EXPLICIT_EXCHANGE_LABEL_ALIASES target '{}' is not in iso10383.json",
                mic
            );
            label_to_mic.insert(normalize_exchange_label_key(alias), (*mic).to_string());
        }

        let curated_mics: HashSet<String> = catalog
            .exchanges
            .iter()
            .map(|entry| entry.mic.clone())
            .collect();

        Self {
            catalog,
            iso_by_mic,
            curated_mics,
            exchange_list,
            name_by_mic,
            currency_by_mic,
            timezone_by_mic,
            close_by_mic,
            currency_priority_slices,
            yahoo_code_to_mic,
            label_to_mic,
            yahoo_suffix_to_mic: suffix_to_mic,
            yahoo_suffixes,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_iso_snapshot_is_complete_and_identified() {
        let iso: IsoCatalog = serde_json::from_str(include_str!("iso10383.json")).unwrap();
        assert!(iso.source_url.starts_with("https://www.iso20022.org/"));
        assert_eq!(
            iso.source_sha256,
            "79de0f7704e260bd49b0d2439f3084891cabc93481da8bdbaa716e15a27211ed"
        );
        assert_eq!(iso.publication_date, "2026-09-14");
        assert_eq!(iso.implementation_date, "2026-09-28");
        assert!(iso.records.len() > 2_800);
    }

    #[test]
    fn every_curated_exchange_is_an_iso_mic() {
        for entry in &REGISTRY.catalog.exchanges {
            assert!(is_known_mic(&entry.mic), "{} is not an ISO MIC", entry.mic);
        }
        assert_eq!(
            REGISTRY
                .catalog
                .legacy_exchanges
                .iter()
                .map(|entry| entry.mic.as_str())
                .collect::<Vec<_>>(),
            ["XLON_IL"]
        );
    }

    #[test]
    fn every_provider_rule_has_one_symbol_convention() {
        for exchange in REGISTRY
            .catalog
            .exchanges
            .iter()
            .chain(REGISTRY.catalog.legacy_exchanges.iter())
        {
            for (provider, rule) in &exchange.providers {
                let has_suffix = rule
                    .suffix
                    .as_deref()
                    .is_some_and(|suffix| !suffix.trim().is_empty());
                assert!(
                    rule.bare ^ has_suffix,
                    "{} / {} must be bare or suffixed",
                    exchange.mic,
                    provider
                );
            }
        }
    }

    #[test]
    fn current_iso_exchange_needs_no_curated_entry_to_appear() {
        let exchanges = get_exchange_list();
        let fallback = exchanges
            .iter()
            .find(|entry| entry.mic == "21XX")
            .expect("21XX comes from the ISO fallback");
        assert!(!fallback.configured);
        assert_eq!(fallback.operating_mic, "21XX");
        assert_eq!(fallback.currency, None);
        assert!(exchanges
            .iter()
            .all(|entry| matches!(entry.status.as_str(), "ACTIVE" | "UPDATED")));
    }

    #[test]
    fn legacy_values_normalize_without_polluting_the_catalog() {
        assert_eq!(canonicalize_exchange_mic(" cxe "), "BCXE");
        assert_eq!(canonicalize_exchange_mic("XAQE"), "AQSE");
        assert_eq!(canonicalize_exchange_mic("XTAI_OTC"), "ROCO");
        assert!(is_known_mic("CXE"));
        assert!(!get_exchange_list().iter().any(|entry| entry.mic == "CXE"));
    }

    #[test]
    fn resolves_known_exchange_labels_and_separates_foreign_nasdaq_from_us_xnas() {
        assert_eq!(
            resolve_exchange_label_to_mic("NASDAQ NMS - GLOBAL MARKET"),
            Some("XNAS".to_string())
        );
        assert_eq!(
            resolve_exchange_label_to_mic("NASDAQ GLOBAL SELECT MARKET"),
            Some("XNAS".to_string())
        );
        assert_eq!(
            resolve_exchange_label_to_mic("NMS"),
            Some("XNAS".to_string())
        );
        assert_eq!(
            resolve_exchange_label_to_mic("NASDAQ STOCKHOLM"),
            Some("XSTO".to_string())
        );
        assert_eq!(
            resolve_exchange_label_to_mic("NASDAQ STOCKHOLM AB"),
            Some("XSTO".to_string())
        );
        assert_eq!(
            resolve_exchange_label_to_mic("NASDAQ COPENHAGEN"),
            Some("XCSE".to_string())
        );
        assert!(!is_us_exchange_mic("XSTO"));
        assert!(!is_us_exchange_mic("XCSE"));
        assert_eq!(
            resolve_exchange_label_to_mic("UNFAMILIAR EXCHANGE LABEL"),
            None
        );
        assert_eq!(
            resolve_exchange_label_to_mic("NASDAQ UNKNOWN NORDIC VENUE"),
            None
        );
    }

    #[test]
    fn reconciles_us_operating_and_segment_mics_while_preserving_segment_conflicts() {
        assert_eq!(
            reconcile_us_exchange_mics("XNGS", "XNAS"),
            Some("XNGS".to_string())
        );
        assert_eq!(
            reconcile_us_exchange_mics("XNAS", "XNGS"),
            Some("XNGS".to_string())
        );
        assert_eq!(
            reconcile_us_exchange_mics("XNMS", "XNAS"),
            Some("XNMS".to_string())
        );
        assert_eq!(
            reconcile_us_exchange_mics("XNGS", "XNGS"),
            Some("XNGS".to_string())
        );
        assert!(are_compatible_us_exchange_mics("XNGS", "XNAS"));
        // Explicit segment conflicts under the same operating exchange must fail.
        assert_eq!(reconcile_us_exchange_mics("XNGS", "XNCM"), None);
        assert_eq!(reconcile_us_exchange_mics("ARCX", "XASE"), None);
        // Separately configured venues sharing an ISO operating MIC must stay distinct.
        assert_eq!(reconcile_us_exchange_mics("XNYS", "ARCX"), None);
        assert_eq!(reconcile_us_exchange_mics("ARCX", "XNYS"), None);
        assert_eq!(reconcile_us_exchange_mics("XNYS", "XASE"), None);
        assert!(!are_compatible_us_exchange_mics("XNYS", "ARCX"));
        // Different US operating exchanges and non-US MICs must fail.
        assert_eq!(reconcile_us_exchange_mics("XNGS", "XNYS"), None);
        assert_eq!(reconcile_us_exchange_mics("XNAS", "XTSE"), None);

        assert_eq!(configured_exchange_mic("XNGS"), Some("XNAS".to_string()));
        assert_eq!(configured_exchange_mic("ARCX"), Some("ARCX".to_string()));
        assert_eq!(operating_exchange_mic("XNGS"), Some("XNAS".to_string()));
    }
}
