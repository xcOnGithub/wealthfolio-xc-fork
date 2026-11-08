# Patched dependency

`rig-core/` is the published MIT-licensed `rig-core` 0.46.0 crate, with its
license and upstream sources retained. Registry cache metadata and the crate's
standalone lockfile are omitted; the workspace lockfile controls resolution.

Source: <https://crates.io/crates/rig-core/0.46.0>

Original crate SHA-256:
`c46316e31b3e80a050ad35790f1b612b4dfdb46982293cc78641caf4271f1b05`

The only source change is in `src/catalog/details.rs`. `CostTier` deserializes
flattened rates through `serde_json::Value` before reading their typed values.
Serde's flatten buffer otherwise exposes arbitrary-precision JSON numbers as
maps and rejects fractional `f64` rates. Rig's bundled catalog then panics during
AI provider initialization whenever Plaid enables exact JSON numbers in the
shared Cargo feature graph.

This patch retains the flattened serialized shape, all catalog rows, pricing
values, and Plaid's exact decimal handling. Docker's planner, dependency cook,
and backend stages all receive the same patched source. The dependency is
excluded from workspace membership and application formatting; application Rust
tests, formatting, Clippy, and CI checks remain enabled.

The application regression in `crates/ai/src/providers.rs` enables the production
exact-number feature combination, checks fractional tier decoding and wire
shape, and loads the complete built-in catalog. Remove this patch once a
compatible published Rig release passes that regression and the AI/Plaid suites.
