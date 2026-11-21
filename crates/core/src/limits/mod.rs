//! Contribution limits module - domain models, services, and traits.

mod limits_model;
mod limits_service;
mod limits_traits;

pub use limits_model::{
    normalize_limit_category, AccountDeposit, ContributionActivity, ContributionActivityDetail,
    ContributionLimit, ContributionLimitDetails, ContributionTreatment, DepositsCalculation,
    NewContributionLimit, CONTRIBUTION_LIMIT_CATEGORIES, MAX_CONTRIBUTION_YEAR,
    MIN_CONTRIBUTION_YEAR,
};
pub use limits_service::{limit_deadline_date, ContributionLimitService};
pub use limits_traits::{ContributionLimitRepositoryTrait, ContributionLimitServiceTrait};
