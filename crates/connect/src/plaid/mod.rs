//! Direct, opt-in Plaid provider boundary, independent of hosted Connect.
//!
//! Shared services enforce profile ownership, verified Hosted Link results,
//! encrypted credentials, and atomic reviewed imports. The web runtime opts in;
//! native linking is explicitly unsupported.

/// Account binding marker, distinct from Plaid accounts managed by hosted Connect.
pub use wealthfolio_core::accounts::DIRECT_PLAID_PROVIDER;

mod client;
mod models;

pub use client::{
    AccessToken, LinkCredential, PlaidClient, PlaidConfig, PlaidEnvironment, PlaidError,
    PlaidProduct, PlaidToken, PublicToken,
};
pub use models::*;

pub mod ingest;
pub mod service;
pub mod state;

#[cfg(test)]
mod ingest_tests;
