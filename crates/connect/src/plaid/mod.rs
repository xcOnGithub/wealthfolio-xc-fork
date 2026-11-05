//! Direct, opt-in Plaid provider boundary, independent of hosted Connect.
//!
//! This module is not wired to application routes or scheduling yet. Callers must
//! enforce authenticated profile ownership, Link-session verification, encrypted
//! token storage and consent before using it. Page methods never persist cursors
//! or apply financial records. Callers must validate account ownership, source
//! record identity, cursor progress and Link expiration before use; ingestion
//! must commit complete batches atomically. Successful deserialization alone
//! establishes none of those application-level invariants.

mod client;
mod models;

pub use client::{
    AccessToken, LinkCredential, PlaidClient, PlaidConfig, PlaidEnvironment, PlaidError,
    PlaidProduct, PlaidToken, PublicToken,
};
pub use models::*;
