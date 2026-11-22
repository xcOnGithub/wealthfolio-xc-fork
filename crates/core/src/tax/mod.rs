//! Family Tax Center domain module.

mod export;
mod holding_period;
mod model;
mod service;
mod wash_sale;

#[cfg(test)]
mod tests;

pub use export::{build_tax_packet_export, csv_decimal_cell, csv_text_cell};
pub use holding_period::{classify_holding_period, days_until_long_term, one_year_anniversary};
pub use model::*;
pub use service::{TaxCenterService, TaxCenterServiceTrait};
pub use wash_sale::{
    active_wash_sale_cooldown, detect_wash_sales, split_factor_between, AssetSplitEvent,
    LossDisposalInput, ReplacementBuyInput,
};
