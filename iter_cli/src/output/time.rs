//! Relative-time renderer for the human ps view.
//!
//! Output buckets:
//!
//! - `< 5s`               → `"just now"`
//! - `< 60s`              → `"<n> seconds ago"`
//! - `< 60m`              → `"<n> minutes ago"`
//! - `< 24h`              → `"<n> hours ago"`
//! - `< 14d`              → `"<n> days ago"`
//! - `< 8 weeks`          → `"<n> weeks ago"`
//! - otherwise            → `"<n> months ago"` (30-day months)
//!
//! Future timestamps clamp to `"just now"`. Machine-mode (`--format
//! json`, `inspect`) emits ISO-8601 UTC instead of using this helper.

use chrono::{DateTime, Utc};

/// Render `when` as a relative-to-now phrase.
#[must_use]
pub(crate) fn relative_time(when: DateTime<Utc>) -> String {
    let now = Utc::now();
    let delta = now.signed_duration_since(when);
    let seconds = delta.num_seconds();
    if seconds < 5 {
        return "just now".to_owned();
    }
    if seconds < 60 {
        return format!("{seconds} seconds ago");
    }
    let minutes = delta.num_minutes();
    if minutes < 60 {
        return pluralize(minutes, "minute");
    }
    let hours = delta.num_hours();
    if hours < 24 {
        return pluralize(hours, "hour");
    }
    let days = delta.num_days();
    if days < 14 {
        return pluralize(days, "day");
    }
    if days < 56 {
        let weeks = days / 7;
        return pluralize(weeks, "week");
    }
    let months = days / 30;
    pluralize(months, "month")
}

fn pluralize(value: i64, unit: &str) -> String {
    if value == 1 {
        format!("1 {unit} ago")
    } else {
        format!("{value} {unit}s ago")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    fn render_ago(seconds: i64) -> String {
        let when = Utc::now() - Duration::seconds(seconds);
        relative_time(when)
    }

    #[test]
    fn future_collapses_to_just_now() {
        let when = Utc::now() + Duration::seconds(60);
        assert_eq!(relative_time(when), "just now");
    }

    #[test]
    fn one_second_in_the_future_is_just_now() {
        let when = Utc::now() + Duration::seconds(1);
        assert_eq!(relative_time(when), "just now");
    }

    #[test]
    fn within_five_seconds_is_just_now() {
        assert_eq!(render_ago(0), "just now");
        assert_eq!(render_ago(4), "just now");
    }

    #[test]
    fn seconds_bucket() {
        assert_eq!(render_ago(30), "30 seconds ago");
    }

    #[test]
    fn minutes_bucket() {
        assert_eq!(render_ago(90), "1 minute ago");
        assert_eq!(render_ago(120), "2 minutes ago");
    }

    #[test]
    fn hours_bucket() {
        assert_eq!(render_ago(2 * 3600), "2 hours ago");
    }

    #[test]
    fn days_bucket() {
        assert_eq!(render_ago(3 * 86400), "3 days ago");
    }

    #[test]
    fn weeks_bucket() {
        assert_eq!(render_ago(2 * 7 * 86400), "2 weeks ago");
    }

    #[test]
    fn months_bucket() {
        assert_eq!(render_ago(60 * 86400), "2 months ago");
    }
}
