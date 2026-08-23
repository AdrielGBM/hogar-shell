//! Live upload/download rates, from the kernel's own byte counters.
//!
//! Separate from [`network`](super::network), which answers "am I connected, and how well" from link state:
//! this answers "how much is moving right now", which needs a second sample to mean anything. Kept apart so a
//! bar showing only a Wi-Fi icon never starts a throughput poller it has no use for.

use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use platform_wayland::EventSender;

use crate::resources::History;
use util::broadcast::{Broadcast, Service};

const NET_DIR: &str = "/sys/class/net";
const POLL: Duration = Duration::from_secs(1);

/// Bytes per second in each direction, plus enough history to draw a sparkline.
#[derive(Clone, Debug, Default)]
pub struct NetSpeed {
    pub down: f64,
    pub up: f64,
    pub down_history: History,
    pub up_history: History,
    /// Cumulative totals since boot, for a "transferred" readout.
    pub total_down: u64,
    pub total_up: u64,
}

/// Cumulative received/transmitted bytes across every physical interface. Virtual devices (`docker0`, `veth*`,
/// `lo`) are skipped: counting a bridge would double every byte that crosses it.
fn read_totals(net_dir: &Path) -> (u64, u64) {
    let mut rx = 0;
    let mut tx = 0;
    let Ok(entries) = fs::read_dir(net_dir) else {
        return (0, 0);
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.join("device").exists() {
            continue;
        }
        let stat = |name: &str| -> u64 {
            fs::read_to_string(path.join("statistics").join(name))
                .ok()
                .and_then(|v| v.trim().parse().ok())
                .unwrap_or(0)
        };
        rx += stat("rx_bytes");
        tx += stat("tx_bytes");
    }
    (rx, tx)
}

/// Bytes per second between two cumulative samples. A counter that went backwards means an interface was
/// removed or wrapped, which reports as zero rather than as a negative or absurd rate.
fn rate(previous: u64, now: u64, elapsed: Duration) -> f64 {
    let seconds = elapsed.as_secs_f64();
    if seconds <= 0.0 {
        return 0.0;
    }
    now.saturating_sub(previous) as f64 / seconds
}

static NETSPEED: Service<NetSpeed> = Service::new("hogar-shell-netspeed", run);

fn run(out: &Arc<Broadcast<NetSpeed>>) {
    let dir = Path::new(NET_DIR);
    let (mut last_rx, mut last_tx) = read_totals(dir);
    let mut last_at = Instant::now();
    let mut down_history = History::default();
    let mut up_history = History::default();
    loop {
        std::thread::sleep(POLL);
        let (rx, tx) = read_totals(dir);
        let now = Instant::now();
        let elapsed = now.duration_since(last_at);
        let down = rate(last_rx, rx, elapsed);
        let up = rate(last_tx, tx, elapsed);
        (last_rx, last_tx, last_at) = (rx, tx, now);
        down_history.push(down as f32);
        up_history.push(up as f32);
        out.publish(NetSpeed {
            down,
            up,
            down_history: down_history.clone(),
            up_history: up_history.clone(),
            total_down: rx,
            total_up: tx,
        });
        if !out.wanted() {
            return;
        }
    }
}

pub fn subscribe(tx: EventSender<NetSpeed>) {
    NETSPEED.subscribe(tx);
}

pub fn current() -> Option<NetSpeed> {
    NETSPEED.current()
}

/// A rate as a bar chip shows it: `1.2 MB/s`. Decimal units, which is how link speeds are quoted, unlike the
/// binary units used for stored bytes.
pub fn format_rate(bytes_per_second: f64) -> String {
    const UNITS: [&str; 4] = ["B/s", "kB/s", "MB/s", "GB/s"];
    let mut value = bytes_per_second;
    let mut unit = 0;
    while value >= 1000.0 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    // A decimal on a three-digit reading, or on raw bytes, is a digit that changes every frame and says nothing.
    if unit == 0 || value >= 100.0 {
        format!("{value:.0} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_is_the_delta_over_elapsed_time() {
        assert_eq!(rate(1_000, 3_000, Duration::from_secs(2)), 1_000.0);
        assert_eq!(rate(1_000, 1_000, Duration::from_secs(1)), 0.0);
    }

    #[test]
    fn a_counter_going_backwards_reads_as_idle() {
        // An interface was unplugged, or the counter wrapped; neither is a negative transfer rate.
        assert_eq!(rate(9_000, 10, Duration::from_secs(1)), 0.0);
        assert_eq!(
            rate(0, 100, Duration::ZERO),
            0.0,
            "no elapsed time, no rate"
        );
    }

    #[test]
    fn totals_skip_virtual_interfaces() {
        let dir = std::env::temp_dir().join(format!("hogar-shell-net-{}", std::process::id()));
        let physical = dir.join("eth0");
        let virt = dir.join("docker0");
        fs::create_dir_all(physical.join("statistics")).unwrap();
        fs::create_dir_all(physical.join("device")).unwrap();
        fs::create_dir_all(virt.join("statistics")).unwrap();
        fs::write(physical.join("statistics").join("rx_bytes"), "500").unwrap();
        fs::write(physical.join("statistics").join("tx_bytes"), "100").unwrap();
        fs::write(virt.join("statistics").join("rx_bytes"), "9999").unwrap();
        fs::write(virt.join("statistics").join("tx_bytes"), "9999").unwrap();

        assert_eq!(
            read_totals(&dir),
            (500, 100),
            "a bridge with no backing device is not counted"
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn missing_sysfs_is_not_fatal() {
        assert_eq!(read_totals(Path::new("/nonexistent-net")), (0, 0));
    }

    #[test]
    fn rates_render_in_decimal_units() {
        assert_eq!(format_rate(0.0), "0 B/s");
        assert_eq!(format_rate(1_500.0), "1.5 kB/s");
        assert_eq!(format_rate(12_300_000.0), "12.3 MB/s");
        assert_eq!(format_rate(250_000.0), "250 kB/s");
    }
}
