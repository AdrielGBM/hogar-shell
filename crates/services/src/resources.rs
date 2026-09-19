//! CPU, memory, storage and temperature as one shared reading.
//!
//! These four are polled rather than event-driven — the kernel has no "usage changed" signal — so they are deliberately a *single* service with one timer and one publish per tick, not four. A bar with a CPU chip and a dashboard with five cards then cost one wakeup a second between them, which is the difference between a shell that idles and one that keeps a core warm.
//!
//! Everything here reads `/proc` and sysfs directly: no `lm-sensors`, no `nvidia-smi`, nothing to install. A machine that doesn't expose a reading (no hwmon, no swap) reports `None` for it and the UI omits it.

use std::collections::VecDeque;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use platform_wayland::EventSender;

use util::broadcast::{Broadcast, Service};

/// How often the reading is refreshed. A second is the granularity a person can actually read off a bar chip; anything faster costs wakeups for numbers nobody can follow.
const POLL: Duration = Duration::from_secs(1);

/// How many readings a sparkline keeps — a minute of history at [`POLL`].
pub const HISTORY: usize = 60;

/// A fixed-length window of recent readings, oldest first. Kept by the service rather than by each card, so several cards charting the same series show the same history instead of each starting blank on open.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct History(VecDeque<f32>);

impl History {
    /// Records a reading, dropping the oldest once the window is full.
    pub fn push(&mut self, value: f32) {
        if self.0.len() == HISTORY {
            self.0.pop_front();
        }
        self.0.push_back(value);
    }

    /// The readings, oldest first.
    pub fn values(&self) -> Vec<f32> {
        self.0.iter().copied().collect()
    }

    pub fn latest(&self) -> f32 {
        self.0.back().copied().unwrap_or(0.0)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The largest reading in the window, for scaling a chart whose series has no natural ceiling (byte rates).
    pub fn peak(&self) -> f32 {
        self.0.iter().copied().fold(0.0, f32::max)
    }
}

#[derive(Clone, Debug, Default)]
pub struct Memory {
    /// Bytes.
    pub total: u64,
    pub used: u64,
    pub swap_total: u64,
    pub swap_used: u64,
}

impl Memory {
    pub fn used_percent(&self) -> f32 {
        percent(self.used, self.total)
    }

    pub fn swap_percent(&self) -> f32 {
        percent(self.swap_used, self.swap_total)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Disk {
    pub mount: PathBuf,
    /// Bytes.
    pub total: u64,
    pub used: u64,
}

impl Disk {
    pub fn used_percent(&self) -> f32 {
        percent(self.used, self.total)
    }
}

/// One hwmon reading, named the way the kernel names it so a user can pick the sensor they care about.
#[derive(Clone, Debug, PartialEq)]
pub struct Sensor {
    /// The hwmon device's `name` (`k10temp`, `coretemp`, `nvme`).
    pub chip: String,
    /// The sensor's `tempN_label` (`Tctl`, `Package id 0`), falling back to `tempN`.
    pub label: String,
    pub celsius: f32,
}

/// One tick's view of the machine.
#[derive(Clone, Debug, Default)]
pub struct Resources {
    /// Aggregate CPU busy time, 0–100.
    pub cpu: f32,
    pub cpu_history: History,
    /// Per-core busy time, 0–100, in `/proc/stat` order.
    pub cores: Vec<f32>,
    /// The CPU's marketing name; empty when `/proc/cpuinfo` names none.
    pub cpu_model: String,
    /// Mean current clock across the cores in MHz; `None` on a machine that reports no live frequency.
    pub cpu_mhz: Option<f32>,
    /// Bytes per second read from and written to every whole disk.
    pub disk_read: f64,
    pub disk_write: f64,
    pub memory: Memory,
    pub memory_history: History,
    /// The hottest sensor the machine exposes, in °C; `None` when there is no hwmon to read.
    pub temperature: Option<f32>,
    /// Every plausible hwmon reading, so a surface can name the sensor it wants instead of taking the maximum.
    pub sensors: Vec<Sensor>,
    /// The root filesystem, plus `/home` when it is a separate mount.
    pub disks: Vec<Disk>,
}

impl Resources {
    /// The reading `[temperature] sensor` names: the first sensor whose chip or label matches, case-insensitively. An empty name — or one that matches nothing, because the user moved the config to another machine — falls back to the hottest sensor rather than blanking the chip.
    pub fn temperature_of(&self, name: &str) -> Option<f32> {
        let name = name.trim();
        if name.is_empty() {
            return self.temperature;
        }
        self.sensors
            .iter()
            .find(|s| s.chip.eq_ignore_ascii_case(name) || s.label.eq_ignore_ascii_case(name))
            .map(|s| s.celsius)
            .or(self.temperature)
    }
}

/// The CPU's marketing name, from the first `model name` line of `/proc/cpuinfo`.
///
/// Read once and cached: it cannot change while the machine is running, and re-reading a file of one block per core every second to learn a string that never moves is exactly the kind of cost this service exists to avoid. Trimmed of the padding Intel bakes into the field (`Intel(R) Core(TM) i7   @ 2.60GHz`).
fn parse_cpu_model(text: &str) -> String {
    text.lines()
        .find_map(|line| line.strip_prefix("model name"))
        .and_then(|rest| rest.split_once(':'))
        .map(|(_, name)| name.split_whitespace().collect::<Vec<_>>().join(" "))
        .unwrap_or_default()
}

/// Mean current clock across the cores, in MHz.
///
/// Averaged rather than reported per core because a bar shows one number, and the per-core spread is what `cores` already carries. `/proc/cpuinfo`'s `cpu MHz` is the live frequency on every kernel that has one; a machine that reports none (a VM, some ARM) yields `None` rather than a zero that reads as "stopped".
fn parse_cpu_mhz(text: &str) -> Option<f32> {
    let readings: Vec<f32> = text
        .lines()
        .filter_map(|line| line.strip_prefix("cpu MHz"))
        .filter_map(|rest| rest.split_once(':'))
        .filter_map(|(_, value)| value.trim().parse::<f32>().ok())
        .collect();
    if readings.is_empty() {
        return None;
    }
    Some(readings.iter().sum::<f32>() / readings.len() as f32)
}

/// Cumulative sectors read and written across every physical disk, from `/proc/diskstats`.
///
/// Partitions are skipped, not summed: `/proc/diskstats` lists `sda` *and* `sda1`, so counting both would double every byte. A partition is recognised by its parent existing in sysfs — `/sys/block/<name>` holds whole devices only, which is the kernel's own answer to the question and needs no name-shape guessing.
fn parse_diskstats(text: &str, is_whole_disk: impl Fn(&str) -> bool) -> (u64, u64) {
    let (mut read, mut written) = (0u64, 0u64);
    for line in text.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        // major minor name reads merged sectors_read ms writes merged sectors_written …
        if fields.len() < 10 || !is_whole_disk(fields[2]) {
            continue;
        }
        read += fields[5].parse::<u64>().unwrap_or(0);
        written += fields[9].parse::<u64>().unwrap_or(0);
    }
    // Sectors are 512 B here regardless of the device's own block size — a kernel ABI, not a property of the disk.
    (read * 512, written * 512)
}

fn is_whole_disk(name: &str) -> bool {
    Path::new("/sys/block").join(name).is_dir()
}

/// Bytes per second between two cumulative samples, over `POLL`. A counter that went backwards means it was reset (a device disappearing), which reports as idle rather than as a nonsense spike — the same rule the CPU sampler uses.
fn rate_between(previous: u64, now: u64) -> f64 {
    now.saturating_sub(previous) as f64 / POLL.as_secs_f64()
}

fn percent(part: u64, whole: u64) -> f32 {
    if whole == 0 {
        return 0.0;
    }
    (part as f64 / whole as f64 * 100.0) as f32
}

/// Cumulative jiffies from one `/proc/stat` cpu line: everything, and the idle share of it.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
struct CpuTimes {
    total: u64,
    idle: u64,
}

/// Parses a `/proc/stat` `cpu…` line. Fields are `user nice system idle iowait irq softirq steal …`; idle time is `idle + iowait`, since a core waiting on disk is not doing work.
fn parse_cpu_line(line: &str) -> Option<CpuTimes> {
    let mut fields = line.split_whitespace();
    let label = fields.next()?;
    if !label.starts_with("cpu") {
        return None;
    }
    let values: Vec<u64> = fields.filter_map(|f| f.parse().ok()).collect();
    if values.len() < 5 {
        return None;
    }
    Some(CpuTimes {
        total: values.iter().sum(),
        idle: values[3] + values[4],
    })
}

/// Busy percentage between two cumulative samples. Counters only ever grow, so a smaller `now` means they were reset (a suspend/resume) and the sample is reported as idle rather than as a nonsense spike.
fn busy_between(previous: CpuTimes, now: CpuTimes) -> f32 {
    let total = now.total.saturating_sub(previous.total);
    let idle = now.idle.saturating_sub(previous.idle);
    if total == 0 {
        return 0.0;
    }
    (100.0 * (total.saturating_sub(idle)) as f64 / total as f64) as f32
}

/// Every `cpu…` line of `/proc/stat`: the aggregate first, then one per core.
fn read_cpu_times(text: &str) -> Vec<CpuTimes> {
    text.lines().map_while(parse_cpu_line).collect()
}

/// Parses `/proc/meminfo`. "Used" follows what `free` reports: total minus available, which counts reclaimable cache as free — the number a user recognises as their memory pressure.
fn parse_meminfo(text: &str) -> Memory {
    let field = |name: &str| -> u64 {
        text.lines()
            .find(|l| l.starts_with(name))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|v| v.parse::<u64>().ok())
            .map(|kb| kb * 1024)
            .unwrap_or(0)
    };
    let total = field("MemTotal:");
    let available = field("MemAvailable:");
    let swap_total = field("SwapTotal:");
    let swap_free = field("SwapFree:");
    Memory {
        total,
        used: total.saturating_sub(available),
        swap_total,
        swap_used: swap_total.saturating_sub(swap_free),
    }
}

const HWMON_DIR: &str = "/sys/class/hwmon";

/// Every `tempN_input` across every hwmon device, in °C (the files are millidegrees), carrying the chip and sensor labels the kernel publishes so `[temperature] sensor` has something to name. Sorted so the list a picker shows is stable between ticks rather than in directory order.
fn read_sensors(hwmon: &Path) -> Vec<Sensor> {
    let mut sensors = Vec::new();
    let Ok(devices) = fs::read_dir(hwmon) else {
        return sensors;
    };
    for device in devices.flatten() {
        let path = device.path();
        let chip = fs::read_to_string(path.join("name"))
            .map(|n| n.trim().to_string())
            .unwrap_or_default();
        let Ok(entries) = fs::read_dir(&path) else {
            continue;
        };
        let mut inputs: Vec<String> = entries
            .flatten()
            .filter_map(|e| e.file_name().to_str().map(str::to_string))
            .filter(|n| n.starts_with("temp") && n.ends_with("_input"))
            .collect();
        inputs.sort();
        for input in inputs {
            let Some(celsius) = fs::read_to_string(path.join(&input))
                .ok()
                .and_then(|t| t.trim().parse::<f32>().ok())
                .map(|millidegrees| millidegrees / 1000.0)
            else {
                continue;
            };
            // Some sensors report obvious nonsense when unplugged or unpowered; a machine is not at 200 °C.
            if !(1.0..=150.0).contains(&celsius) {
                continue;
            }
            let stem = input.trim_end_matches("_input");
            let label = fs::read_to_string(path.join(format!("{stem}_label")))
                .ok()
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty())
                .unwrap_or_else(|| stem.to_string());
            sensors.push(Sensor {
                chip: chip.clone(),
                label,
                celsius,
            });
        }
    }
    sensors.sort_by(|a, b| (&a.chip, &a.label).cmp(&(&b.chip, &b.label)));
    sensors
}

/// The hottest plausible reading, which is what a chip shows when no sensor is named — it keeps working across AMD, Intel and laptops without a per-machine config.
fn hottest(sensors: &[Sensor]) -> Option<f32> {
    sensors
        .iter()
        .map(|s| s.celsius)
        .fold(None, |acc: Option<f32>, c| {
            Some(acc.map_or(c, |a| a.max(c)))
        })
}

/// A mount's capacity, straight from the `statvfs` syscall. "Used" is total minus what an unprivileged user can claim, which is what `df` reports: it excludes the root-reserved blocks, so a full disk reads as full.
fn read_disk(mount: &Path) -> Option<Disk> {
    let stats = rustix::fs::statvfs(mount).ok()?;
    // `f_frsize` is the fragment size the block counts are in; kernels that leave it 0 mean `f_bsize`.
    let block = if stats.f_frsize > 0 {
        stats.f_frsize
    } else {
        stats.f_bsize
    };
    let total = stats.f_blocks.checked_mul(block)?;
    let available = stats.f_bavail.saturating_mul(block);
    (total > 0).then(|| Disk {
        mount: mount.to_path_buf(),
        total,
        used: total.saturating_sub(available),
    })
}

/// The mounts worth charting: the root filesystem, and `/home` when the user put it on its own device.
fn interesting_mounts() -> Vec<PathBuf> {
    let root = PathBuf::from("/");
    let home = util::paths::home_dir();
    let mut mounts = vec![root];
    if let Some(home) = home
        && let (Ok(home_meta), Ok(root_meta)) = (fs::metadata(&home), fs::metadata("/"))
    {
        use std::os::unix::fs::MetadataExt;
        if home_meta.dev() != root_meta.dev() {
            mounts.push(home);
        }
    }
    mounts
}

/// Ticks between disk re-reads. Free space moves in minutes, not seconds, so `statvfs`-ing every mount once a second would be pure wakeup cost for a number that hasn't changed.
const DISK_EVERY: u32 = 30;

const CPUINFO: &str = "/proc/cpuinfo";
const DISKSTATS: &str = "/proc/diskstats";

static RESOURCES: Service<Resources> = Service::new("hogar-shell-resources", run);

fn run(out: &Arc<Broadcast<Resources>>) {
    let mut previous = read_cpu_times(&fs::read_to_string("/proc/stat").unwrap_or_default());
    let mut cpu_history = History::default();
    let mut memory_history = History::default();
    let mounts = interesting_mounts();
    let mut disks: Vec<Disk> = mounts.iter().filter_map(|m| read_disk(m)).collect();
    // The model never changes while the machine runs, so it is read once rather than every tick.
    let cpu_model = parse_cpu_model(&fs::read_to_string(CPUINFO).unwrap_or_default());
    let mut io = parse_diskstats(
        &fs::read_to_string(DISKSTATS).unwrap_or_default(),
        is_whole_disk,
    );
    let mut tick: u32 = 0;
    loop {
        std::thread::sleep(POLL);
        let now = read_cpu_times(&fs::read_to_string("/proc/stat").unwrap_or_default());
        let busy: Vec<f32> = now
            .iter()
            .zip(previous.iter())
            .map(|(now, was)| busy_between(*was, *now))
            .collect();
        previous = now;

        let memory = parse_meminfo(&fs::read_to_string("/proc/meminfo").unwrap_or_default());
        let cpu = busy.first().copied().unwrap_or(0.0);
        cpu_history.push(cpu);
        memory_history.push(memory.used_percent());

        tick = tick.wrapping_add(1);
        if tick.is_multiple_of(DISK_EVERY) {
            disks = mounts.iter().filter_map(|m| read_disk(m)).collect();
        }

        let cpuinfo = fs::read_to_string(CPUINFO).unwrap_or_default();
        let now_io = parse_diskstats(
            &fs::read_to_string(DISKSTATS).unwrap_or_default(),
            is_whole_disk,
        );
        let (disk_read, disk_write) = (rate_between(io.0, now_io.0), rate_between(io.1, now_io.1));
        io = now_io;

        let sensors = read_sensors(Path::new(HWMON_DIR));
        out.publish(Resources {
            cpu,
            cpu_history: cpu_history.clone(),
            cores: busy.into_iter().skip(1).collect(),
            cpu_model: cpu_model.clone(),
            cpu_mhz: parse_cpu_mhz(&cpuinfo),
            disk_read,
            disk_write,
            memory_history: memory_history.clone(),
            memory,
            temperature: hottest(&sensors),
            sensors,
            disks: disks.clone(),
        });
        if !out.wanted() {
            return;
        }
    }
}

/// Registers `tx` for live system readings, starting the single shared poller on first use.
pub fn subscribe(tx: EventSender<Resources>) {
    RESOURCES.subscribe(tx);
}

/// The last reading, without waiting for the next tick.
pub fn current() -> Option<Resources> {
    RESOURCES.current()
}

/// Renders a byte count the way a person reads it: three significant figures and a binary unit.
pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else if value >= 100.0 {
        format!("{value:.0} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cpu_model_is_read_once_and_stripped_of_its_padding() {
        let cpuinfo = "processor\t: 0\nmodel name\t: AMD Ryzen 7 5800X   8-Core Processor\ncpu MHz\t\t: 3800.000\n\nprocessor\t: 1\nmodel name\t: AMD Ryzen 7 5800X   8-Core Processor\ncpu MHz\t\t: 2200.000\n";
        assert_eq!(
            parse_cpu_model(cpuinfo),
            "AMD Ryzen 7 5800X 8-Core Processor",
            "the run of spaces vendors bake into the field is collapsed"
        );
        assert_eq!(parse_cpu_model(""), "");
    }

    #[test]
    fn the_frequency_is_the_mean_across_cores_and_absent_when_unreported() {
        let cpuinfo = "cpu MHz\t\t: 3800.000\ncpu MHz\t\t: 2200.000\n";
        assert_eq!(
            parse_cpu_mhz(cpuinfo),
            Some(3000.0),
            "one number for one bar"
        );
        // A VM or an ARM board reports no live clock; `None` says so rather than a zero reading as "stopped".
        assert_eq!(parse_cpu_mhz("model name\t: Cortex-A72\n"), None);
    }

    #[test]
    fn disk_totals_count_whole_devices_and_not_their_partitions() {
        // `/proc/diskstats` lists sda and sda1; summing both would double every byte.
        let stats = "\
   8       0 sda 100 0 200 0 50 0 400 0
   8       1 sda1 90 0 180 0 40 0 360 0
 259       0 nvme0n1 10 0 20 0 5 0 40 0
";
        let whole = |name: &str| matches!(name, "sda" | "nvme0n1");
        let (read, written) = parse_diskstats(stats, whole);
        assert_eq!(read, (200 + 20) * 512, "sectors are 512 B by kernel ABI");
        assert_eq!(written, (400 + 40) * 512);
    }

    #[test]
    fn a_reset_disk_counter_reports_idle_rather_than_a_spike() {
        assert_eq!(rate_between(1_000, 1_000), 0.0);
        assert_eq!(
            rate_between(5_000, 1_000),
            0.0,
            "a device that went away must not read as a burst of gigabytes"
        );
        assert!(rate_between(0, 4_096) > 0.0);
    }

    #[test]
    fn cpu_busy_is_the_non_idle_share_between_two_samples() {
        let was = CpuTimes {
            total: 1000,
            idle: 800,
        };
        let now = CpuTimes {
            total: 1100,
            idle: 850,
        };
        assert_eq!(busy_between(was, now), 50.0);
    }

    #[test]
    fn cpu_busy_survives_a_counter_reset_and_a_still_frame() {
        let high = CpuTimes {
            total: 10_000,
            idle: 5_000,
        };
        let reset = CpuTimes { total: 10, idle: 5 };
        assert_eq!(
            busy_between(high, reset),
            0.0,
            "a reset counter reads as idle, not as a spike"
        );
        assert_eq!(
            busy_between(high, high),
            0.0,
            "no elapsed time cannot divide by zero"
        );
    }

    #[test]
    fn proc_stat_yields_the_aggregate_then_one_entry_per_core() {
        let text = "\
cpu  100 0 50 800 50 0 0 0 0 0
cpu0 50 0 25 400 25 0 0 0 0 0
cpu1 50 0 25 400 25 0 0 0 0 0
intr 12345
";
        let times = read_cpu_times(text);
        assert_eq!(times.len(), 3, "aggregate + two cores, stopping at `intr`");
        assert_eq!(times[0].total, 1000);
        assert_eq!(times[0].idle, 850, "idle + iowait");
    }

    #[test]
    fn meminfo_reports_used_as_total_minus_available() {
        let text = "\
MemTotal:       16000000 kB
MemFree:         1000000 kB
MemAvailable:    8000000 kB
SwapTotal:       4000000 kB
SwapFree:        3000000 kB
";
        let memory = parse_meminfo(text);
        assert_eq!(memory.total, 16_000_000 * 1024);
        assert_eq!(
            memory.used,
            8_000_000 * 1024,
            "cache counts as free, matching `free`"
        );
        assert_eq!(memory.swap_used, 1_000_000 * 1024);
        assert_eq!(memory.used_percent(), 50.0);
    }

    #[test]
    fn meminfo_missing_fields_do_not_divide_by_zero() {
        let memory = parse_meminfo("");
        assert_eq!(memory.total, 0);
        assert_eq!(memory.used_percent(), 0.0);
        assert_eq!(memory.swap_percent(), 0.0);
    }

    /// A scratch hwmon tree: one chip with a labelled sensor, an unlabelled one, and an implausible reading.
    fn hwmon_fixture(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("hogar-shell-hwmon-{}-{tag}", std::process::id()));
        let device = dir.join("hwmon0");
        fs::create_dir_all(&device).unwrap();
        fs::write(device.join("name"), "coretemp").unwrap();
        fs::write(device.join("temp1_input"), "45000").unwrap();
        fs::write(device.join("temp1_label"), "Package id 0").unwrap();
        fs::write(device.join("temp2_input"), "61500").unwrap();
        // An unplugged sensor reporting an impossible value must not become "the temperature".
        fs::write(device.join("temp3_input"), "200000").unwrap();
        dir
    }

    #[test]
    fn temperature_takes_the_hottest_plausible_sensor() {
        let dir = hwmon_fixture("hottest");
        assert_eq!(hottest(&read_sensors(&dir)), Some(61.5));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn sensors_carry_the_chip_and_label_a_config_can_name() {
        let dir = hwmon_fixture("named");
        let sensors = read_sensors(&dir);
        assert_eq!(sensors.len(), 2, "the implausible reading is dropped");
        assert_eq!(sensors[0].chip, "coretemp");
        assert_eq!(sensors[0].label, "Package id 0", "the kernel's label wins");
        assert_eq!(
            sensors[1].label, "temp2",
            "an unlabelled sensor keeps its file name"
        );

        let resources = Resources {
            temperature: hottest(&sensors),
            sensors,
            ..Resources::default()
        };
        assert_eq!(resources.temperature_of("Package id 0"), Some(45.0));
        assert_eq!(
            resources.temperature_of("coretemp"),
            Some(45.0),
            "a chip name matches its first sensor"
        );
        assert_eq!(
            resources.temperature_of("PACKAGE ID 0"),
            Some(45.0),
            "matching ignores case"
        );
        assert_eq!(
            resources.temperature_of("k10temp"),
            Some(61.5),
            "a name from another machine falls back to the hottest rather than blanking the chip"
        );
        assert_eq!(resources.temperature_of(""), Some(61.5));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn no_hwmon_at_all_reports_no_temperature() {
        let sensors = read_sensors(Path::new("/nonexistent-hwmon"));
        assert!(sensors.is_empty());
        assert_eq!(hottest(&sensors), None);
        assert_eq!(Resources::default().temperature_of("anything"), None);
    }

    #[test]
    fn history_is_a_bounded_window_that_keeps_the_newest() {
        let mut history = History::default();
        for i in 0..(HISTORY + 10) {
            history.push(i as f32);
        }
        let values = history.values();
        assert_eq!(values.len(), HISTORY, "old readings fall off the front");
        assert_eq!(history.latest(), (HISTORY + 9) as f32);
        assert_eq!(
            values[0], 10.0,
            "the oldest kept reading is the 11th pushed"
        );
        assert_eq!(history.peak(), (HISTORY + 9) as f32);
    }

    #[test]
    fn bytes_render_with_a_binary_unit_and_three_significant_figures() {
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1536), "1.5 KiB");
        assert_eq!(format_bytes(16 * 1024 * 1024 * 1024), "16.0 GiB");
        assert_eq!(format_bytes(900 * 1024 * 1024), "900 MiB");
    }
}
