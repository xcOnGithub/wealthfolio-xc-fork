//! Holding period classification following IRS Pub. 550 calendar-year rules.

use chrono::{Datelike, NaiveDate};

use super::model::HoldingTerm;

/// Returns the 1-year calendar anniversary of `open_date`.
///
/// For leap-day acquisitions (Feb 29), the anniversary in the following non-leap
/// year is Feb 28, so holding through Mar 1 is > 1 year (long-term).
pub fn one_year_anniversary(open_date: NaiveDate) -> NaiveDate {
    let next_year = open_date.year() + 1;
    if let Some(exact) = NaiveDate::from_ymd_opt(next_year, open_date.month(), open_date.day()) {
        return exact;
    }
    // Only possible when open_date is Feb 29 and next_year is not a leap year.
    NaiveDate::from_ymd_opt(next_year, 2, 28)
        .expect("February 28 is a valid date in every supported calendar year")
}

/// Classifies a disposal's holding period.
///
/// Under IRS rules:
/// - Short sales (`is_short_sale == true`) default to `ShortTerm` (IRC §1233).
/// - Long positions held **more than 1 year** (`disposal_date > one_year_anniversary(open_date)`)
///   are `LongTerm`; held 1 year or less are `ShortTerm`.
/// - Missing `open_date` yields `Unknown`.
pub fn classify_holding_period(
    open_date: Option<NaiveDate>,
    disposal_date: NaiveDate,
    is_short_sale: bool,
) -> (HoldingTerm, Option<i64>) {
    let holding_days = open_date.map(|open| (disposal_date - open).num_days());
    if is_short_sale {
        return (HoldingTerm::ShortTerm, holding_days);
    }
    let Some(open) = open_date else {
        return (HoldingTerm::Unknown, None);
    };
    let anniversary = one_year_anniversary(open);
    let term = if disposal_date > anniversary {
        HoldingTerm::LongTerm
    } else {
        HoldingTerm::ShortTerm
    };
    (term, holding_days)
}

/// For an open long lot on `as_of_date`, returns `Some(days)` remaining until the
/// first long-term day (`one_year_anniversary(open_date) + 1 day`), or `None` if
/// already long-term.
pub fn days_until_long_term(open_date: NaiveDate, as_of_date: NaiveDate) -> Option<i64> {
    let anniversary = one_year_anniversary(open_date);
    let first_long_term_day = anniversary
        .succ_opt()
        .expect("tax dates are far before NaiveDate::MAX");
    if as_of_date >= first_long_term_day {
        None
    } else {
        Some((first_long_term_day - as_of_date).num_days())
    }
}
