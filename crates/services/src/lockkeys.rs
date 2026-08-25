//! Caps Lock and Num Lock, read off the keyboard LEDs in sysfs.
//!
//! This is the one service in the shell with no event source to subscribe to. Wayland reports locked modifiers only to a surface holding keyboard focus, which a bar deliberately does not take, and Hyprland's event stream carries no lock state. So it polls — two small sysfs reads on a lazily-started thread, so a shell without the `lockstatus` module never runs it at all, and the poller retires the moment the last indicator goes away rather than reading sysfs for nobody.

use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use platform_wayland::EventSender;

use util::broadcast::{Broadcast, Service};

const LEDS_DIR: &str = "/sys/class/leds";

/// Fast enough that pressing Caps Lock and glancing at the bar shows the new state, slow enough that the cost is two file reads a few times a second on a thread that only exists when something is watching.
const POLL: Duration = Duration::from_millis(300);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LockKeys {
    pub caps: bool,
    pub num: bool,
}

/// Whether the LED at `entry` is lit, reading its brightness through `buf` rather than a fresh `String`.
fn is_lit(entry: &fs::DirEntry, buf: &mut String) -> bool {
    use std::io::Read;
    buf.clear();
    fs::File::open(entry.path().join("brightness"))
        .and_then(|mut file| file.read_to_string(buf))
        .is_ok()
        && buf.trim().parse::<u32>().is_ok_and(|value| value > 0)
}

/// Whether the machine exposes lock LEDs at all. A virtual machine or a laptop whose firmware hides them has nothing to report, and the producer retires rather than polling files that will never exist.
fn has_leds(leds: &Path) -> bool {
    fs::read_dir(leds).is_ok_and(|entries| {
        entries.flatten().any(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.ends_with("::capslock") || name.ends_with("::numlock"))
        })
    })
}

/// Both modifiers off one walk of the LED directory.
///
/// Each attached keyboard gets its own `inputN::capslock` entry and they all track the same modifier, so one lit LED is the answer for the machine. Read in a single pass, with one buffer reused across the entries, because this runs three times a second forever: as two passes building a `String` per file it accounted for 25,989 allocations in twenty minutes, of which 25,991 were transient — 99.99%.
fn read_from(leds: &Path) -> LockKeys {
    let mut keys = LockKeys::default();
    let Ok(entries) = fs::read_dir(leds) else {
        return keys;
    };
    let mut buf = String::new();
    for entry in entries.flatten() {
        let file_name = entry.file_name();
        let Some(name) = file_name.to_str() else {
            continue;
        };
        let caps = name.ends_with("::capslock");
        if !caps && !name.ends_with("::numlock") {
            continue;
        }
        // Already established by another keyboard's LED; no need to read this one.
        if (caps && keys.caps) || (!caps && keys.num) {
            continue;
        }
        if is_lit(&entry, &mut buf) {
            if caps {
                keys.caps = true;
            } else {
                keys.num = true;
            }
        }
    }
    keys
}

/// The current lock state; all-off on a machine that exposes no lock LEDs.
pub fn read() -> LockKeys {
    read_from(Path::new(LEDS_DIR))
}

static LOCK_KEYS: Service<LockKeys> = Service::new("hogar-shell-lock-keys", run);

fn run(out: &Arc<Broadcast<LockKeys>>) {
    let leds = Path::new(LEDS_DIR);
    if !has_leds(leds) {
        tracing::info!("no keyboard lock LEDs in {LEDS_DIR}; the lock-key indicators stay idle");
        return;
    }
    let mut last = read_from(leds);
    out.publish(last);
    while out.wanted() {
        std::thread::sleep(POLL);
        let current = read_from(leds);
        if current != last {
            last = current;
            out.publish(current);
        }
    }
}

/// Registers `tx` for live lock-key readings, starting the single shared poller on first use.
pub fn subscribe(tx: EventSender<LockKeys>) {
    LOCK_KEYS.subscribe(tx);
}

/// The last published reading, without touching sysfs.
pub fn current() -> Option<LockKeys> {
    LOCK_KEYS.current()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(tag: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("hogar-shell-leds-{}-{tag}", std::process::id()));
        fs::create_dir_all(dir.join("input3::capslock")).unwrap();
        fs::create_dir_all(dir.join("input3::numlock")).unwrap();
        fs::create_dir_all(dir.join("input3::scrolllock")).unwrap();
        // An unrelated LED that must not be mistaken for a keyboard one.
        fs::create_dir_all(dir.join("phy0-led")).unwrap();
        fs::write(dir.join("phy0-led/brightness"), "1").unwrap();
        dir
    }

    #[test]
    fn a_lit_led_reads_as_engaged_and_an_unlit_one_does_not() {
        let dir = fixture("state");
        fs::write(dir.join("input3::capslock/brightness"), "1").unwrap();
        fs::write(dir.join("input3::numlock/brightness"), "0").unwrap();
        assert_eq!(
            read_from(&dir),
            LockKeys {
                caps: true,
                num: false
            }
        );

        fs::write(dir.join("input3::capslock/brightness"), "0").unwrap();
        fs::write(dir.join("input3::numlock/brightness"), "1").unwrap();
        assert_eq!(
            read_from(&dir),
            LockKeys {
                caps: false,
                num: true
            }
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn any_keyboard_with_the_led_lit_counts() {
        let dir = fixture("multi");
        fs::create_dir_all(dir.join("input9::capslock")).unwrap();
        fs::write(dir.join("input3::capslock/brightness"), "0").unwrap();
        fs::write(dir.join("input9::capslock/brightness"), "1").unwrap();
        fs::write(dir.join("input3::numlock/brightness"), "0").unwrap();
        assert!(
            read_from(&dir).caps,
            "the external keyboard's LED reports the same modifier"
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_machine_without_lock_leds_reports_nothing_rather_than_polling() {
        let missing = Path::new("/nonexistent-leds");
        assert!(!has_leds(missing));
        assert_eq!(read_from(missing), LockKeys::default());

        let dir =
            std::env::temp_dir().join(format!("hogar-shell-leds-{}-bare", std::process::id()));
        fs::create_dir_all(dir.join("phy0-led")).unwrap();
        assert!(!has_leds(&dir), "a wifi LED is not a keyboard lock LED");
        fs::remove_dir_all(&dir).ok();
    }
}
