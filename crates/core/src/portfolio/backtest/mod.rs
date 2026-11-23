//! In-session portfolio backtest experiments.

pub mod engine;
pub mod model;
pub mod service;

pub use model::*;
pub use service::run_backtest;
