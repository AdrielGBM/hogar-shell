//! The graphics processor: how busy it is, how hot, and how much of its memory is in use.
//!
//! Two backends, because the kernel only tells half the story. AMD's `amdgpu` publishes utilisation and VRAM
//! straight into sysfs, so reading it costs four file reads and no process; NVIDIA's driver publishes none of
//! that and answers only NVML, which this shell `dlopen`s rather than paying a `nvidia-smi` fork per reading.
//! Intel sits in between — a temperature from
//! hwmon, and no utilisation counter outside the perf interface — and reports what it has rather than
//! inventing the rest.
//!
//! Which is why every field is an `Option`: a card that cannot answer says so, and a card reads as absent only
//! when there is genuinely no GPU to read.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use platform_wayland::EventSender;

use crate::resources::History;
use config::GpuConfig;
use util::broadcast::{Broadcast, Service};

const DRM_DIR: &str = "/sys/class/drm";

/// Slower than the CPU's one-second tick on purpose: a GPU load that matters is one that lasts longer than
/// two seconds, and every backend here reads several counters per tick.
const POLL: Duration = Duration::from_secs(2);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Vendor {
    Amd,
    Intel,
    Nvidia,
    #[default]
    Unknown,
}

impl Vendor {
    /// The PCI vendor id sysfs reports for each of them.
    fn from_pci(id: &str) -> Self {
        match id.trim().trim_start_matches("0x") {
            "1002" => Self::Amd,
            "8086" => Self::Intel,
            "10de" => Self::Nvidia,
            _ => Self::Unknown,
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Some(match id.trim().to_ascii_lowercase().as_str() {
            "amd" | "amdgpu" | "radeon" => Self::Amd,
            "intel" | "i915" | "xe" => Self::Intel,
            "nvidia" => Self::Nvidia,
            _ => return None,
        })
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Amd => "AMD",
            Self::Intel => "Intel",
            Self::Nvidia => "NVIDIA",
            Self::Unknown => "GPU",
        }
    }
}

/// One reading. Every measurement is optional because which of them a driver publishes is a property of the
/// driver, not of the machine — showing a hard zero where there is no counter would read as an idle GPU.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Gpu {
    pub vendor: Vendor,
    /// The card's name where the backend knows one (NVML does), else the vendor.
    pub name: String,
    /// Utilisation 0–100.
    pub usage: Option<f32>,
    pub usage_history: History,
    /// Degrees Celsius.
    pub temperature: Option<f32>,
    pub vram_used: Option<u64>,
    pub vram_total: Option<u64>,
}

impl Gpu {
    /// VRAM in use as a 0..1 fraction, for a meter. `None` when the card reports no memory at all — an
    /// integrated one shares system RAM, which the memory card already shows.
    pub fn vram_fraction(&self) -> Option<f32> {
        let (used, total) = (self.vram_used?, self.vram_total?);
        (total > 0).then(|| used as f32 / total as f32)
    }
}

/// Where a card's readings live: its sysfs device directory, plus the driver behind it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Card {
    /// `/sys/class/drm/card1`.
    path: PathBuf,
    /// The name the `drm` subsystem gives it (`card1`), which is what `[gpu] card` selects on.
    id: String,
    vendor: Vendor,
}

fn read_trimmed(path: &Path) -> Option<String> {
    fs::read_to_string(path).ok().map(|s| s.trim().to_string())
}

fn read_number<T: std::str::FromStr>(path: &Path) -> Option<T> {
    read_trimmed(path)?.parse().ok()
}

/// Every GPU the `drm` subsystem knows about, in card order.
///
/// The connector entries (`card1-DP-1`) share the directory and are not cards; they are filtered by the dash,
/// which no card name contains.
fn cards() -> Vec<Card> {
    let Ok(entries) = fs::read_dir(DRM_DIR) else {
        return Vec::new();
    };
    let mut found: Vec<Card> = entries
        .flatten()
        .filter_map(|entry| {
            let id = entry.file_name().to_str()?.to_string();
            if !id.starts_with("card") || id.contains('-') {
                return None;
            }
            let path = entry.path().join("device");
            let vendor = read_trimmed(&path.join("vendor"))
                .map(|v| Vendor::from_pci(&v))
                .unwrap_or_default();
            Some(Card { path, id, vendor })
        })
        .collect();
    found.sort_by(|a, b| a.id.cmp(&b.id));
    found
}

/// The card to read: the one `[gpu] card` names, else the first with a vendor we have a backend for, else the
/// first card at all. A laptop with switchable graphics lists the integrated GPU first, so "first with a
/// backend" is not enough on its own — which is exactly what `card` is there to override.
fn select(cards: &[Card], config: &GpuConfig) -> Option<Card> {
    let wanted = config.card.trim();
    if !wanted.is_empty() {
        return cards.iter().find(|c| c.id == wanted).cloned();
    }
    if let Some(forced) = Vendor::from_id(&config.backend) {
        return cards.iter().find(|c| c.vendor == forced).cloned();
    }
    cards
        .iter()
        .find(|c| c.vendor != Vendor::Unknown)
        .or_else(|| cards.first())
        .cloned()
}

/// The first `temp*_input` under the card's hwmon directory, in millidegrees. Which index carries the die
/// temperature differs by driver (`temp1` on amdgpu, but not universally), so this takes the lowest-numbered
/// one rather than assuming.
fn hwmon_temperature(device: &Path) -> Option<f32> {
    let hwmon = fs::read_dir(device.join("hwmon")).ok()?.flatten().next()?;
    let mut inputs: Vec<PathBuf> = fs::read_dir(hwmon.path())
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("temp") && n.ends_with("_input"))
        })
        .collect();
    inputs.sort();
    let millidegrees: f32 = read_number(inputs.first()?)?;
    Some(millidegrees / 1000.0)
}

/// Reads a card straight out of sysfs. `gpu_busy_percent` and the `mem_info_vram_*` pair are amdgpu's; Intel
/// publishes neither, so an Intel card comes back with a temperature and honest `None`s.
fn read_sysfs(card: &Card) -> Gpu {
    Gpu {
        vendor: card.vendor,
        name: card.vendor.label().to_string(),
        usage: read_number::<f32>(&card.path.join("gpu_busy_percent")),
        usage_history: History::default(),
        temperature: hwmon_temperature(&card.path),
        vram_used: read_number(&card.path.join("mem_info_vram_used")),
        vram_total: read_number(&card.path.join("mem_info_vram_total")),
    }
}

/// The NVIDIA card, through NVML.
///
/// This used to fork `nvidia-smi` and parse a CSV line, once per poll tick. NVML is the library that tool is
/// itself a front end for, so the readings are identical and the process start is gone — see [`crate::nvml`]
/// for why it is `dlopen`ed rather than linked.
fn read_nvidia() -> Option<Gpu> {
    crate::nvml::read(0).map(from_nvml)
}

/// NVML's reading as this service's. Split out so the rule every field here exists for — a counter the card
/// does not publish reads as unknown, never as zero — is testable without an NVIDIA GPU present.
fn from_nvml(reading: crate::nvml::Reading) -> Gpu {
    Gpu {
        vendor: Vendor::Nvidia,
        name: reading
            .name
            .unwrap_or_else(|| Vendor::Nvidia.label().to_string()),
        usage: reading.usage.map(|percent| percent as f32),
        usage_history: History::default(),
        temperature: reading.temperature.map(|celsius| celsius as f32),
        vram_used: reading.vram_used,
        vram_total: reading.vram_total,
    }
}

/// The current reading, or `None` when there is no GPU this shell can read.
pub fn read(config: &GpuConfig) -> Option<Gpu> {
    if config.backend.trim().eq_ignore_ascii_case("none") {
        return None;
    }
    let card = select(&cards(), config);
    let vendor = match Vendor::from_id(&config.backend) {
        Some(forced) => forced,
        None => card.as_ref().map(|c| c.vendor).unwrap_or_default(),
    };
    // NVIDIA's card *is* in sysfs, and reading it there yields nothing but a directory: the driver publishes
    // no utilisation, no temperature and no memory outside its own tool.
    if vendor == Vendor::Nvidia {
        return read_nvidia();
    }
    card.map(|card| read_sysfs(&card))
}

static GPU: Service<Gpu> = Service::new("hogar-shell-gpu", run);

/// The `[gpu]` settings, or the defaults outside a started shell. Read through the cross-thread snapshot: the
/// only caller is the producer, and the driver thread's own copy is invisible from there.
fn settings() -> GpuConfig {
    config::shared_config()
        .map(|c| c.gpu.clone())
        .unwrap_or_default()
}

/// Polls, because neither backend has an event source: sysfs counters are files with no notification, and
/// One poll for the whole shell, not one per surface.
fn run(out: &Arc<Broadcast<Gpu>>) {
    let config = settings();
    let mut history = History::default();
    let mut last = Gpu::default();
    loop {
        match read(&config) {
            Some(mut gpu) => {
                if let Some(usage) = gpu.usage {
                    history.push(usage);
                }
                gpu.usage_history = history.clone();
                last = gpu.clone();
                out.publish(gpu);
            }
            // Said once, so a subscriber knows the answer is "no GPU" rather than "not yet", and not repeated:
            // a desktop with no readable card would otherwise wake every surface twice a second forever.
            None if last != Gpu::default() || out.current().is_none() => {
                last = Gpu::default();
                out.publish(Gpu::default());
            }
            None => {}
        }
        if !out.wanted() {
            return;
        }
        std::thread::sleep(POLL);
    }
}

/// Registers `tx` for live readings — unless `[gpu] enabled` is off, in which case nothing is registered and
/// the producer is never started. Guarding here rather than inside the producer is what makes a disabled
/// section cost *zero* threads rather than one that exits: `Service` spawns on first touch.
pub fn subscribe(tx: EventSender<Gpu>) {
    if !settings().enabled {
        return;
    }
    GPU.subscribe(tx);
}

pub fn current() -> Option<Gpu> {
    if !settings().enabled {
        return None;
    }
    GPU.current()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(id: &str, vendor: Vendor) -> Card {
        Card {
            path: PathBuf::from(DRM_DIR).join(id).join("device"),
            id: id.to_string(),
            vendor,
        }
    }

    #[test]
    fn the_pci_vendor_id_names_the_driver_behind_the_card() {
        assert_eq!(Vendor::from_pci("0x1002"), Vendor::Amd);
        assert_eq!(Vendor::from_pci("0x8086"), Vendor::Intel);
        assert_eq!(Vendor::from_pci("0x10de"), Vendor::Nvidia);
        assert_eq!(Vendor::from_pci("0xffff"), Vendor::Unknown);
    }

    #[test]
    fn selection_prefers_a_card_with_a_backend_and_obeys_an_override() {
        let cards = vec![
            card("card0", Vendor::Unknown),
            card("card1", Vendor::Intel),
            card("card2", Vendor::Nvidia),
        ];
        let auto = GpuConfig::default();
        assert_eq!(
            select(&cards, &auto).unwrap().id,
            "card1",
            "the first card with a driver we can read, not just the first card"
        );

        let forced = GpuConfig {
            backend: "nvidia".into(),
            ..auto.clone()
        };
        assert_eq!(select(&cards, &forced).unwrap().id, "card2");

        // Naming a card wins over the backend: it is the more specific answer, and on a laptop with two cards
        // from one vendor it is the only one that can distinguish them.
        let named = GpuConfig {
            backend: "nvidia".into(),
            card: "card1".into(),
            ..auto.clone()
        };
        assert_eq!(select(&cards, &named).unwrap().id, "card1");
        let missing = GpuConfig {
            card: "card9".into(),
            ..auto.clone()
        };
        assert!(
            select(&cards, &missing).is_none(),
            "a named card that isn't there is not silently swapped"
        );
    }

    #[test]
    fn a_machine_with_no_readable_card_still_picks_something_rather_than_nothing() {
        let cards = vec![card("card0", Vendor::Unknown)];
        assert_eq!(select(&cards, &GpuConfig::default()).unwrap().id, "card0");
        assert!(select(&[], &GpuConfig::default()).is_none());
    }

    /// The rule the whole `Option` shape of [`Gpu`] exists for. Carried over from when this backend parsed
    /// `nvidia-smi`'s `[N/A]` columns: NVML says the same thing by failing the individual call, and the
    /// mapping must keep meaning "unknown" rather than filling in a zero that draws an idle GPU.
    #[test]
    fn a_field_the_card_does_not_measure_reads_as_unknown_not_as_zero() {
        let gpu = from_nvml(crate::nvml::Reading {
            name: Some("Quadro P400".to_string()),
            usage: None,
            temperature: Some(48),
            vram_used: None,
            vram_total: None,
        });
        assert_eq!(gpu.usage, None, "no counter is not an idle GPU");
        assert_eq!(gpu.temperature, Some(48.0));
        assert_eq!(gpu.vram_fraction(), None);
        assert_eq!(gpu.name, "Quadro P400");

        // A card that will not even give its name is still an NVIDIA card, not a nameless row.
        let unnamed = from_nvml(crate::nvml::Reading::default());
        assert_eq!(unnamed.name, "NVIDIA");
    }

    #[test]
    fn reading_never_panics_on_this_machine() {
        // Whatever hardware the test runs on: a card, no card, or a driver publishing nothing.
        let gpu = read(&GpuConfig::default());
        if let Some(gpu) = gpu {
            assert!(gpu.usage.is_none_or(|u| (0.0..=100.0).contains(&u)));
            assert!(gpu.temperature.is_none_or(|t| (-50.0..=150.0).contains(&t)));
        }
        assert!(
            read(&GpuConfig {
                backend: "none".into(),
                ..GpuConfig::default()
            })
            .is_none(),
            "`none` switches the service off rather than making it guess"
        );
    }
}
