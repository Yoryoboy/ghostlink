//! Production system clock adapter.
//!
//! [`SystemClock`] implements the provider-neutral [`Clock`] port over the
//! operating system wall clock. It reports whole Unix seconds, so it never
//! depends on an injected test clock or an ambient environment value at the
//! call site, and it fails closed for any instant the application's nonnegative
//! [`Timestamp`] cannot represent.
//!
//! Failure posture:
//!
//! - an instant before the Unix epoch cannot be reported as a nonnegative
//!   `Timestamp`, so it fails with the sanitized
//!   [`ErrorCategory::ClockOverflow`] category;
//! - a seconds value outside the `u64` range `Timestamp` accepts fails with the
//!   same sanitized category;
//! - no failure path renders the offending instant, seconds value, or any
//!   operating-system error text.
//!
//! The conversion is factored into a private [`timestamp_from_unix_seconds`]
//! seam so edge tests can exercise the pre-epoch and overflow boundaries
//! deterministically without waiting for or fabricating a real clock.

use destiny_core::{AppError, Clock, ErrorCategory, Timestamp};
use std::time::{SystemTime, UNIX_EPOCH};

/// The single sanitized failure category this adapter reports.
fn clock_overflow() -> AppError {
    AppError::new(ErrorCategory::ClockOverflow)
}

/// Converts signed Unix seconds into the application's nonnegative [`Timestamp`].
///
/// A negative value is a pre-epoch instant and a value above `u64::MAX` is an
/// unrepresentable instant; both fail closed with the same sanitized category.
fn timestamp_from_unix_seconds(seconds: i128) -> Result<Timestamp, AppError> {
    u64::try_from(seconds)
        .map(Timestamp::new)
        .map_err(|_| clock_overflow())
}

/// Converts one absolute system time into Unix seconds.
///
/// Subsecond precision is intentionally discarded: the application's
/// [`Timestamp`] is a whole-second value. An instant before the Unix epoch has
/// no nonnegative representation, so it is mapped to the pre-epoch failure
/// rather than wrapped or clamped.
fn timestamp_from_system_time(now: SystemTime) -> Result<Timestamp, AppError> {
    let seconds = now
        .duration_since(UNIX_EPOCH)
        .map(|duration| i128::from(duration.as_secs()))
        .unwrap_or(-1);
    timestamp_from_unix_seconds(seconds)
}

/// A production [`Clock`] reading the operating system wall clock in Unix seconds.
///
/// It holds no state and is cheap to construct or copy, so a composition root
/// can build one per process instead of threading an ambient value.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SystemClock;

impl SystemClock {
    /// Builds the system clock. Construction is infallible; failures are
    /// reported per [`Clock::now`] call.
    pub const fn new() -> Self {
        Self
    }
}

impl Clock for SystemClock {
    /// The current Unix time in whole seconds.
    ///
    /// # Errors
    ///
    /// Returns a sanitized [`ErrorCategory::ClockOverflow`] error when the
    /// reported instant cannot be represented as a nonnegative second count.
    fn now(&self) -> Result<Timestamp, AppError> {
        timestamp_from_system_time(SystemTime::now())
    }
}

#[cfg(test)]
mod tests {
    use super::{SystemClock, timestamp_from_system_time, timestamp_from_unix_seconds};
    use destiny_core::{Clock, ErrorCategory, Timestamp};
    use std::time::{Duration, UNIX_EPOCH};

    #[test]
    fn reports_whole_seconds_and_discards_subsecond_precision() {
        let instant = UNIX_EPOCH + Duration::new(1_700_000_000, 999_999_999);
        assert_eq!(
            timestamp_from_system_time(instant).expect("representable instant"),
            Timestamp::new(1_700_000_000)
        );
    }

    #[test]
    fn the_epoch_itself_is_the_zero_boundary() {
        assert_eq!(
            timestamp_from_system_time(UNIX_EPOCH).expect("the epoch is representable"),
            Timestamp::new(0)
        );
    }

    #[test]
    fn pre_epoch_instants_fail_sanitized() {
        let before_epoch = UNIX_EPOCH - Duration::from_secs(1);
        let error = timestamp_from_system_time(before_epoch).expect_err("pre-epoch fails closed");
        assert_eq!(error.category(), ErrorCategory::ClockOverflow);
        assert!(!format!("{error} {error:?}").contains("18446744073709551615"));
    }

    #[test]
    fn negative_and_overflowing_seconds_fail_sanitized() {
        assert_eq!(
            timestamp_from_unix_seconds(-1)
                .expect_err("negative fails")
                .category(),
            ErrorCategory::ClockOverflow
        );
        assert_eq!(
            timestamp_from_unix_seconds(i128::from(u64::MAX) + 1)
                .expect_err("overflow fails")
                .category(),
            ErrorCategory::ClockOverflow
        );
    }

    #[test]
    fn the_u64_boundary_is_inclusive() {
        assert_eq!(
            timestamp_from_unix_seconds(i128::from(u64::MAX)).expect("boundary is representable"),
            Timestamp::new(u64::MAX)
        );
    }

    #[test]
    fn the_production_clock_reports_a_plausible_current_time() {
        let now = SystemClock::new().now().expect("the system clock reads");
        // The Unix clock is far past the year 2001 and far short of a `u64`
        // overflow; this protects the conversion without depending on wall time.
        assert!(now.seconds() > 1_000_000_000);
    }
}
