//! The wall clock as a shared source. A clock is the one thing here that genuinely has to tick rather than wait for an event, so the point of routing it through a service is that the whole shell ticks **once**: the bar chip, the clock panel and any other surface all read the same broadcast instead of each arming its own timer. The producer also sleeps to the next second boundary, so the displayed second changes when the system second does instead of drifting by however long the shell took to start.

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Local, Timelike};
use platform_wayland::EventSender;

use util::broadcast::{Broadcast, Service};

pub type Now = DateTime<Local>;

static CLOCK: Service<Now> = Service::new("hogar-shell-clock", run);

/// Registers `tx` for a value on every second boundary, starting the single shared ticker on first use. Called from a clock surface's `watch` producer.
pub fn subscribe(tx: EventSender<Now>) {
    CLOCK.subscribe(tx);
}

/// Why a moment could not be written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unwritable {
    /// The pattern is not one `strftime` can write.
    Pattern,
    /// The moment is outside what the calendar can hold.
    Moment,
}

/// `now` written with a `strftime` pattern — what every clock in the shell draws, and what an expression's `df` writes.
///
/// A pattern chrono cannot render (`%Q`, a lone `%`) is an error rather than a panic: chrono reports it only as a formatting failure, which `to_string` turns into one.
pub fn format(now: &Now, pattern: &str) -> Result<String, Unwritable> {
    use chrono::format::{Item, StrftimeItems};
    if StrftimeItems::new(pattern).any(|item| matches!(item, Item::Error)) {
        return Err(Unwritable::Pattern);
    }
    Ok(now.format(pattern).to_string())
}

/// [`format`], drawing the pattern itself where it cannot be written, so a mistyped `[clock]` pattern shows on the clock rather than blanking it.
pub fn draw(now: &Now, pattern: &str) -> String {
    format(now, pattern).unwrap_or_else(|_| pattern.to_string())
}

/// [`format`] for a moment given as seconds since the Unix epoch, in local time.
pub fn format_at(seconds: f64, pattern: &str) -> Result<String, Unwritable> {
    use chrono::TimeZone;
    let whole = seconds.floor();
    let nanos = ((seconds - whole) * 1e9) as u32;
    let moment = (whole.is_finite() && whole.abs() < 1e15)
        .then(|| Local.timestamp_opt(whole as i64, nanos).earliest())
        .flatten()
        .ok_or(Unwritable::Moment)?;
    format(&moment, pattern)
}

fn run(out: &Arc<Broadcast<Now>>) {
    loop {
        let now = Local::now();
        out.publish(now);
        if !out.wanted() {
            return;
        }
        std::thread::sleep(until_next_second(now));
    }
}

/// How long until the next whole second after `now`. Clamped to at least a millisecond so a reading taken exactly on the boundary can't spin.
fn until_next_second(now: Now) -> Duration {
    let nanos_past = now.nanosecond().min(999_999_999) as u64;
    Duration::from_nanos(1_000_000_000u64.saturating_sub(nanos_past).max(1_000_000))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn a_pattern_chrono_cannot_write_is_an_error_rather_than_a_panic() {
        let moment = Local.timestamp_opt(1_700_000_000, 0).single().unwrap();
        assert!(format(&moment, "%Q").is_err());
        assert!(format(&moment, "100%").is_err());
        assert_eq!(format(&moment, "%Y").as_deref(), Ok("2023"));
        assert_eq!(format_at(1_700_000_000.0, "%Y").as_deref(), Ok("2023"));
        assert!(format_at(1e300, "%Y").is_err());
    }

    #[test]
    fn sleeps_only_the_remainder_of_the_current_second() {
        let quarter = Local
            .timestamp_opt(1_700_000_000, 250_000_000)
            .single()
            .expect("valid timestamp");
        assert_eq!(until_next_second(quarter), Duration::from_millis(750));

        let boundary = Local
            .timestamp_opt(1_700_000_000, 0)
            .single()
            .expect("valid timestamp");
        assert_eq!(
            until_next_second(boundary),
            Duration::from_secs(1),
            "a reading exactly on the boundary waits a full second, not zero"
        );
    }
}
