//! Profile-local family attention, deterministic summaries, and opt-in delivery.
pub mod delivery;
pub mod engine;
pub mod model;
pub mod service;
pub mod sources;
pub use model::*;
pub use service::{FamilyRepository, FamilyService};
pub use sources::FamilySources;

/// Run within the owning runtime's existing profile-scoped worker lifecycle.
/// The caller chooses its task runtime and retains its own abort handle.
pub async fn run_worker(service: std::sync::Arc<FamilyService>) {
    tokio::time::sleep(std::time::Duration::from_secs(30)).await;
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(300));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        interval.tick().await;
        if service.tick().await.is_err() {
            log::warn!("Family money evaluation unavailable; retrying on the next scheduled tick");
        }
    }
}
