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
