use super::{invalid, StatementService};
use crate::{
    accounts::account_types::SECURITIES,
    assets::{Asset, AssetKind, InstrumentType, NewAsset, QuoteMode},
    errors::{DatabaseError, Error, Result},
    portfolio::coordinator::blocking,
};
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

fn is_ticker(symbol: &str) -> bool {
    let (base, class) = symbol
        .split_once(['.', '-'])
        .map_or((symbol, None), |(base, class)| (base, Some(class)));
    symbol != "CASH"
        && (1..=6).contains(&base.len())
        && base.bytes().all(|c| c.is_ascii_uppercase())
        && class.is_none_or(|class| class.len() == 1 && class.as_bytes()[0].is_ascii_uppercase())
}

impl StatementService {
    /// Register symbol-only catalog entries. These are for historical review and
    /// don't schedule enrichment or create ledger entries.
    pub async fn register_statement_securities(
        &self,
        account_id: String,
        tickers: Vec<String>,
    ) -> Result<BTreeMap<String, Asset>> {
        if tickers.iter().any(|symbol| !is_ticker(symbol)) {
            return Err(invalid());
        }
        let tickers: BTreeSet<_> = tickers.into_iter().collect();
        let sources = self.pricing.as_ref().ok_or_else(invalid)?;
        let accounts = self.accounts.clone();
        blocking(move || {
            let account = accounts.get_by_id(&account_id)?;
            if account.account_type != SECURITIES || account.currency != "USD" {
                return Err(invalid());
            }
            Ok(())
        })
        .await?;

        // If a concurrent writer inserts the same instrument, the batch rolls
        // back on its unique key. Resolve again using the newly committed entry.
        let mut retry = true;
        loop {
            let repo = sources.assets.clone();
            let assets = blocking(move || repo.list()).await?;
            let (mut resolved, missing) = match_tickers(&tickers, assets);
            match sources.assets.create_batch(missing).await {
                Ok(created) => {
                    for asset in created {
                        if let Some(ticker) = &asset.instrument_symbol {
                            resolved.insert(ticker.clone(), asset);
                        }
                    }
                    return Ok(resolved);
                }
                Err(Error::Database(DatabaseError::UniqueViolation(_))) if retry => retry = false,
                Err(error) => return Err(error),
            }
        }
    }
}

fn match_tickers(
    tickers: &BTreeSet<String>,
    assets: Vec<Asset>,
) -> (BTreeMap<String, Asset>, Vec<NewAsset>) {
    let mut candidates = BTreeMap::<String, Vec<Asset>>::new();
    for asset in assets {
        let symbol = asset
            .instrument_symbol
            .as_ref()
            .or(asset.display_code.as_ref());
        if let Some(symbol) = symbol
            .map(|s| s.to_ascii_uppercase())
            .filter(|s| tickers.contains(s))
        {
            candidates.entry(symbol).or_default().push(asset);
        }
    }

    let mut resolved = BTreeMap::new();
    let mut missing = Vec::new();
    for ticker in tickers {
        match candidates.remove(ticker).as_deref() {
            Some([asset])
                if asset.kind == AssetKind::Investment
                    && asset.instrument_type == Some(InstrumentType::Equity)
                    && asset.quote_ccy == "USD" =>
            {
                resolved.insert(ticker.clone(), asset.clone());
            }
            // A ticker alone cannot choose between listings or reinterpret an
            // existing instrument or quote currency.
            Some(_) => {}
            None => missing.push(NewAsset {
                id: Some(Uuid::new_v4().to_string()),
                kind: AssetKind::Investment,
                name: None,
                display_code: Some(ticker.clone()),
                is_active: false,
                quote_mode: QuoteMode::Market,
                quote_ccy: "USD".into(),
                instrument_type: Some(InstrumentType::Equity),
                instrument_type_label: None,
                instrument_symbol: Some(ticker.clone()),
                instrument_exchange_mic: None,
                provider_config: None,
                provider_id: None,
                provider_symbol: None,
                notes: None,
                metadata: None,
            }),
        }
    }
    (resolved, missing)
}
