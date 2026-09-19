//! Every external thing hogar-shell needs, in one list.
//!
//! The shell reaches outside itself in five different ways — it runs programs, calls D-Bus peers, reads kernel interfaces, `dlopen`s libraries and binds Wayland protocols — and until this file existed each of those was described in three places that could disagree: the README's dependency table, the packaging metadata's optional-depends list, and the runtime check at the call site. Three copies of one fact means two of them are wrong the moment a dependency moves.
//!
//! So the declaration is made **load-bearing** rather than documentary. [`output`] and [`available`] take a [`Dep`], not a program name, which means a program cannot be run without first having a row in [`ALL`] — and a row carries what the panel and the CLI need to say about it. A dependency added without a row is a compile error rather than a documentation drift, which is the whole point.
//!
//! Lives in `util` because it has to sit below everything that reaches outside: the callers are spread across this crate and `services`, and `util` is the only crate under both.
//!
//! Nothing here probes at startup. A probe costs a process start or a bus round trip, and the answer is only wanted when something asks — the panel, the CLI, or a service deciding whether to bother starting.

use std::collections::HashMap;
use std::path::Path;
use std::sync::RwLock;
use std::time::Duration;

use crate::process;

/// How long a probe may take before it counts as absent. Generous for a bus round trip, mean enough that probing the whole list cannot become a visible pause.
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

/// One external thing the shell can reach for.
///
/// The variants are the *identity*; everything else about them lives in [`ALL`]. Adding one without adding its row there fails `every_dep_has_exactly_one_row`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Dep {
    // Programs.
    PwDump,
    Wpctl,
    WfRecorder,
    GpuScreenRecorder,
    Ddcutil,
    Qalc,
    // D-Bus peers.
    NetworkManager,
    BlueZ,
    UPower,
    Logind,
    Fprintd,
    GameMode,
    Portal,
    // Kernel interfaces.
    Backlight,
    PowerSupply,
    Drm,
    Leds,
    // Libraries loaded at runtime.
    LibPam,
    LibPipeWire,
    Nvml,
    // Wayland protocols.
    LayerShell,
    SessionLock,
    LockNotify,
    IdleNotify,
    ImageCopyCapture,
    Screencopy,
    Workspaces,
    ToplevelManagement,
    GammaControl,
    OutputPower,
}

/// How a dependency is found, which is also how it is probed.
#[derive(Clone, Copy, Debug)]
pub enum Kind {
    /// A program on `PATH`. Probed by running it — usually with `--version`, because "is it on the path" and "does it run on this machine" are different questions and only the second one matters.
    Program {
        name: &'static str,
        probe: &'static [&'static str],
    },
    /// A well-known D-Bus name. Probed by asking the broker who owns it, *and* whether it is activatable: a service that starts on demand is present even while nothing is running.
    Bus { name: &'static str, system: bool },
    /// A kernel interface. Probed by whether the directory exists and has anything in it — `/sys/class/backlight` exists on a desktop with no backlight at all, and an empty one means the same as an absent one here.
    Kernel { path: &'static str },
    /// A shared library opened at runtime, so no linker records it and `ldd` cannot see it. Probed by opening it, in the same order and with the same names the real loader uses.
    ///
    /// Bare sonames come first, so the loader answers with whatever the running system linked against — including, on a packaged build, the binary's own RUNPATH. The absolute paths after them are not belt-and-braces: **a store-based distribution keeps libraries under hashed paths and leaves nothing on the loader's default search path**, so a `cargo`-built shell finds no bare soname at all on a machine where the library plainly works. NixOS's system profile and its driver directory are the stable names there — symlinks into the current generation, so they survive a rebuild and a garbage collection in a way a store path pinned in a config would not.
    Library { sonames: &'static [&'static str] },
    /// A Wayland protocol, probed by asking the compositor's registry for its interfaces by name. Present only when *every* one of them is: `ext-image-copy-capture` is two globals — a capture manager and the factory that makes the sources it takes — and a compositor carrying one without the other can capture nothing.
    ///
    /// Named rather than given a probe function on purpose: the crate's own `lock_supported` and `idle_supported` read state the *driver* owns, so outside a running shell they answer `false` for a compositor that implements the protocol perfectly well — which is the one case a dependency report exists to serve.
    Protocol { interfaces: &'static [&'static str] },
}

/// Whether the shell can run at all without it.
///
/// Exactly one honest meaning: **`Required` is "the process does not start"**. Everything else is `Optional` by construction, because no feature may hard-require a daemon that is not already a dependency without a graceful degraded path. A module that looks empty without something is not a reason to call it required — that is what [`Entry::without`] is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Need {
    Required,
    Optional,
}

/// One row: what it is, how to find it, and what its absence costs — in the user's terms, not the code's.
#[derive(Clone, Copy, Debug)]
pub struct Entry {
    pub dep: Dep,
    /// Stable id for the CLI and any config that names it. Never translated.
    pub id: &'static str,
    pub kind: Kind,
    pub need: Need,
    /// What the shell uses it for.
    pub what: &'static str,
    /// What stops working without it. The one line a user reads to decide whether to install it.
    pub without: &'static str,
}

impl Entry {
    /// Whether reaching it means talking to a daemon in the user's live session — a bus name, or the audio server behind PipeWire's tools and library — which only a process that installed [`crate::live`] may do.
    fn lives_in_the_session(&self) -> bool {
        matches!(self.kind, Kind::Bus { .. })
            || matches!(self.dep, Dep::PwDump | Dep::Wpctl | Dep::LibPipeWire)
    }

    fn sealed(&self) -> bool {
        self.lives_in_the_session() && !crate::live::installed()
    }

    /// The program this row names, for the rows that are programs.
    pub fn program(&self) -> Option<&'static str> {
        match self.kind {
            Kind::Program { name, .. } => Some(name),
            _ => None,
        }
    }
}

/// Every external dependency, in the order a reader wants them: what the shell cannot start without, then the things people actually go looking for when a feature is missing.
pub const ALL: &[Entry] = &[
    Entry {
        dep: Dep::LayerShell,
        id: "wlr-layer-shell",
        kind: Kind::Protocol {
            interfaces: &["zwlr_layer_shell_v1"],
        },
        need: Need::Required,
        what: "every surface the shell draws — bars, panels, the launcher, the wallpaper layer",
        without: "the shell cannot place a single surface and does not start",
    },
    Entry {
        dep: Dep::LibPam,
        id: "libpam",
        kind: Kind::Library {
            sonames: &[
                "libpam.so.0",
                "libpam.so",
                "/run/current-system/sw/lib/libpam.so.0",
            ],
        },
        need: Need::Optional,
        what: "checking the password on the lock screen",
        without: "`lock status` reports that the session cannot be locked, before the screen is ever covered",
    },
    Entry {
        dep: Dep::SessionLock,
        id: "ext-session-lock",
        kind: Kind::Protocol {
            interfaces: &["ext_session_lock_manager_v1"],
        },
        need: Need::Optional,
        what: "covering every output with a lock surface the compositor keeps up",
        without: "the session cannot be locked",
    },
    Entry {
        dep: Dep::LockNotify,
        id: "hyprland-lock-notify",
        kind: Kind::Protocol {
            interfaces: &[platform_wayland::LOCK_NOTIFIER_INTERFACE],
        },
        need: Need::Optional,
        what: "asking whether the session is still locked when the shell restarts after dying with it locked, so it can take the lock back behind a password prompt — Hyprland only",
        without: "the shell never takes a lock back after a crash; the compositor's own recovery is the way back in",
    },
    Entry {
        dep: Dep::IdleNotify,
        id: "ext-idle-notify",
        kind: Kind::Protocol {
            interfaces: &["ext_idle_notifier_v1"],
        },
        need: Need::Optional,
        what: "knowing the seat has gone idle, for the `[idle]` stages",
        without: "idle timers never arm, so nothing locks or blanks on its own",
    },
    Entry {
        dep: Dep::Workspaces,
        id: "ext-workspace",
        kind: Kind::Protocol {
            interfaces: &[platform_wayland::WORKSPACE_INTERFACE],
        },
        need: Need::Optional,
        what: "listing the compositor's workspaces and activating one, for the `workspaces` module",
        without: "the workspace pills are empty on any compositor that is not Hyprland",
    },
    Entry {
        dep: Dep::ToplevelManagement,
        id: "wlr-foreign-toplevel-management",
        kind: Kind::Protocol {
            interfaces: &[platform_wayland::TOPLEVEL_MANAGER_INTERFACE],
        },
        need: Need::Optional,
        what: "which window has focus, and switching to one — the `activewindow` chip and the launcher's `/` mode",
        without: "the active-window chip reads as no window and the launcher lists none to switch to",
    },
    Entry {
        dep: Dep::OutputPower,
        id: "wlr-output-power-management",
        kind: Kind::Protocol {
            interfaces: &[platform_wayland::OUTPUT_POWER_INTERFACE],
        },
        need: Need::Optional,
        what: "blanking and waking a screen, for `shell dpms` and the idle stage that uses it",
        without: "the screen is blanked through the compositor's own dispatcher, which cannot confirm it worked",
    },
    Entry {
        dep: Dep::GammaControl,
        id: "wlr-gamma-control",
        kind: Kind::Protocol {
            interfaces: &[platform_wayland::GAMMA_INTERFACE],
        },
        need: Need::Optional,
        what: "setting each output's gamma ramp, for the night light",
        without: "`nightlight` reports that the compositor cannot tint the screen",
    },
    Entry {
        dep: Dep::ImageCopyCapture,
        id: "ext-image-copy-capture",
        kind: Kind::Protocol {
            interfaces: platform_wayland::IMAGE_COPY_CAPTURE_INTERFACES,
        },
        need: Need::Optional,
        what: "screenshots and the window preview, straight into the shell's own buffers",
        without: "capture falls back to wlr-screencopy, and fails only if that is missing too",
    },
    Entry {
        dep: Dep::Screencopy,
        id: "wlr-screencopy",
        kind: Kind::Protocol {
            interfaces: platform_wayland::SCREENCOPY_INTERFACES,
        },
        need: Need::Optional,
        what: "the same captures on a compositor too old for ext-image-copy-capture",
        without: "nothing, as long as the compositor implements ext-image-copy-capture",
    },
    Entry {
        dep: Dep::PwDump,
        id: "pw-dump",
        kind: Kind::Program {
            name: "pw-dump",
            probe: &["--version"],
        },
        need: Need::Optional,
        what: "the audio graph: devices, streams, levels and mutes",
        without: "every audio module stays empty — volume, mic, the mixer and the per-application sliders",
    },
    Entry {
        dep: Dep::Wpctl,
        id: "wpctl",
        kind: Kind::Program {
            name: "wpctl",
            probe: &["-h"],
        },
        need: Need::Optional,
        what: "changing a volume or a mute",
        without: "audio can be read but not adjusted",
    },
    Entry {
        dep: Dep::LibPipeWire,
        id: "libpipewire",
        kind: Kind::Library {
            sonames: &[
                "libpipewire-0.3.so.0",
                "libpipewire-0.3.so",
                "/run/current-system/sw/lib/libpipewire-0.3.so.0",
            ],
        },
        need: Need::Optional,
        what: "capturing what the speakers are playing, for the visualiser",
        without: "the visualiser bars stay silent",
    },
    Entry {
        dep: Dep::NetworkManager,
        id: "networkmanager",
        kind: Kind::Bus {
            name: "org.freedesktop.NetworkManager",
            system: true,
        },
        need: Need::Optional,
        what: "the network state, the Wi-Fi list and the VPN connections",
        without: "the network module reports no connection and the VPN list is empty",
    },
    Entry {
        dep: Dep::BlueZ,
        id: "bluez",
        kind: Kind::Bus {
            name: "org.bluez",
            system: true,
        },
        need: Need::Optional,
        what: "Bluetooth adapters and devices",
        without: "the Bluetooth module is hidden entirely",
    },
    Entry {
        dep: Dep::UPower,
        id: "upower",
        kind: Kind::Bus {
            name: "org.freedesktop.UPower",
            system: true,
        },
        need: Need::Optional,
        what: "battery charge, health and time remaining",
        without: "the battery falls back to sysfs, and is hidden if that is absent too",
    },
    Entry {
        dep: Dep::PowerSupply,
        id: "power-supply",
        kind: Kind::Kernel {
            path: "/sys/class/power_supply",
        },
        need: Need::Optional,
        what: "battery readings without UPower",
        without: "nothing, as long as UPower is there",
    },
    Entry {
        dep: Dep::Logind,
        id: "logind",
        kind: Kind::Bus {
            name: "org.freedesktop.login1",
            system: true,
        },
        need: Need::Optional,
        what: "suspend, hibernate, reboot, shut down, and setting a backlight without root",
        without: "the session actions are unavailable and brightness cannot be written",
    },
    Entry {
        dep: Dep::Backlight,
        id: "backlight",
        kind: Kind::Kernel {
            path: "/sys/class/backlight",
        },
        need: Need::Optional,
        what: "the internal panel's brightness",
        without: "internal brightness is unavailable; external monitors still work through ddcutil",
    },
    Entry {
        dep: Dep::Ddcutil,
        id: "ddcutil",
        kind: Kind::Program {
            name: "ddcutil",
            probe: &["--version"],
        },
        need: Need::Optional,
        what: "the brightness of external monitors, over DDC/CI",
        without: "only internal panels are dimmable",
    },
    Entry {
        dep: Dep::Leds,
        id: "leds",
        kind: Kind::Kernel {
            path: "/sys/class/leds",
        },
        need: Need::Optional,
        what: "keyboard backlight and the state of the lock keys",
        without: "no keyboard backlight control",
    },
    Entry {
        dep: Dep::Drm,
        id: "drm",
        kind: Kind::Kernel {
            path: "/sys/class/drm",
        },
        need: Need::Optional,
        what: "GPU utilisation and VRAM on AMD and Intel, straight from the kernel",
        without: "GPU fields read unknown on an AMD or Intel card; an NVIDIA one answers through NVML instead",
    },
    Entry {
        dep: Dep::Nvml,
        id: "libnvidia-ml",
        kind: Kind::Library {
            sonames: &[
                "libnvidia-ml.so.1",
                "libnvidia-ml.so",
                "/run/opengl-driver/lib/libnvidia-ml.so.1",
                "/run/opengl-driver/lib/libnvidia-ml.so",
            ],
        },
        need: Need::Optional,
        what: "utilisation, temperature and VRAM on an NVIDIA card, which publishes none of it to the kernel",
        without: "an NVIDIA GPU reports its name and nothing else",
    },
    Entry {
        dep: Dep::WfRecorder,
        id: "wf-recorder",
        kind: Kind::Program {
            name: "wf-recorder",
            probe: &["--version"],
        },
        need: Need::Optional,
        what: "screen recording, encoded on the CPU",
        without: "nothing, if gpu-screen-recorder is installed instead",
    },
    Entry {
        dep: Dep::GpuScreenRecorder,
        id: "gpu-screen-recorder",
        kind: Kind::Program {
            name: "gpu-screen-recorder",
            probe: &["--version"],
        },
        need: Need::Optional,
        what: "screen recording encoded on the GPU, and the only backend that can pause",
        without: "recording still works through wf-recorder, but cannot be paused",
    },
    Entry {
        dep: Dep::GameMode,
        id: "gamemode",
        kind: Kind::Bus {
            name: "com.feralinteractive.GameMode",
            system: false,
        },
        need: Need::Optional,
        what: "the game-mode toggle",
        without: "the toggle is greyed out",
    },
    Entry {
        dep: Dep::Fprintd,
        id: "fprintd",
        kind: Kind::Bus {
            name: "net.reactivated.Fprint",
            system: true,
        },
        need: Need::Optional,
        what: "unlocking with a fingerprint",
        without: "the lock screen takes a password only",
    },
    Entry {
        dep: Dep::Portal,
        id: "xdg-desktop-portal",
        kind: Kind::Bus {
            name: "org.freedesktop.portal.Desktop",
            system: false,
        },
        need: Need::Optional,
        what: "registering the shell's actions as global shortcuts",
        without: "bind the IPC commands in the compositor's own config instead",
    },
    Entry {
        dep: Dep::Qalc,
        id: "qalc",
        kind: Kind::Program {
            name: "qalc",
            probe: &["-v"],
        },
        need: Need::Optional,
        what: "currencies, constants and dates in the launcher's `=` mode",
        without: "a built-in evaluator handles the ordinary arithmetic",
    },
];

/// The row for `dep`. Total by construction — [`every_dep_has_exactly_one_row`] is what keeps it so.
pub fn entry(dep: Dep) -> &'static Entry {
    ALL.iter()
        .find(|entry| entry.dep == dep)
        .expect("every Dep has a row in ALL")
}

/// What a probe found — three answers, not two.
///
/// `Unknown` is the one that earns its keep. A Wayland protocol can only be asked of a compositor, so from a bare CLI on a machine with no session there is nothing to ask; reporting that as `Absent` would tell a user their compositor lacks a protocol it may implement perfectly well. The same rule the GPU service follows — a driver that publishes no counter reads as unknown, never as zero.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Presence {
    Present,
    Absent,
    Unknown,
}

impl Presence {
    pub fn is_present(self) -> bool {
        self == Presence::Present
    }
}

/// What a probe found for one dependency.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Status {
    pub dep: Dep,
    pub presence: Presence,
}

impl Status {
    /// Whether this is something the user should be told is wrong: a missing optional dependency is a choice, a missing required one is a broken install. An `Unknown` is neither — it is a question this process could not ask.
    pub fn is_a_problem(&self) -> bool {
        self.presence == Presence::Absent && entry(self.dep).need == Need::Required
    }
}

static PROBED: RwLock<Option<HashMap<Dep, Presence>>> = RwLock::new(None);

/// Whether `dep` is on this machine, probing once and remembering the answer.
///
/// **Never call this from the UI thread.** A probe is a process start or a bus round trip; the whole reason [`process::output`] exists is that neither may happen on the thread composing a frame.
///
/// An `Unknown` is cached like any other answer: asking again in the same process would ask the same absent compositor. [`refresh`] is what reconsiders.
pub fn probe(dep: Dep) -> Presence {
    if let Ok(guard) = PROBED.read()
        && let Some(cache) = guard.as_ref()
        && let Some(found) = cache.get(&dep)
    {
        return *found;
    }
    let found = run_probe(entry(dep));
    if let Ok(mut guard) = PROBED.write() {
        guard.get_or_insert_with(HashMap::new).insert(dep, found);
    }
    found
}

/// Probes everything and returns the list in [`ALL`]'s order. Blocking, for the same reason [`probe`] is.
pub fn snapshot() -> Vec<Status> {
    ALL.iter()
        .map(|entry| Status {
            dep: entry.dep,
            presence: probe(entry.dep),
        })
        .collect()
}

/// Probes on a thread of its own and sends one report. The producer half of a `watch`, for the settings page.
///
/// Re-probes rather than reading the cache, because the gesture that reaches this is a user opening the page to find out what is missing — quite possibly having just installed something. A probe of the whole list is a second or two of process starts and bus round trips, which is exactly why it cannot happen on the thread composing the frame.
pub fn report(tx: platform_wayland::EventSender<Vec<Status>>) {
    let _ = std::thread::Builder::new()
        .name("hogar-shell-deps".to_string())
        .spawn(move || {
            refresh();
            tx.send(snapshot());
        });
}

/// Forgets every cached answer, so the next ask probes again — for a package installed while the shell runs.
pub fn refresh() {
    if let Ok(mut guard) = PROBED.write() {
        *guard = None;
    }
}

fn run_probe(entry: &Entry) -> Presence {
    if entry.sealed() {
        return Presence::Unknown;
    }
    let found = |yes: bool| {
        if yes {
            Presence::Present
        } else {
            Presence::Absent
        }
    };
    match entry.kind {
        Kind::Program { name, probe } => found(process::available(name, probe, PROBE_TIMEOUT)),
        Kind::Bus { name, system } => bus_name_exists(name, system),
        // Present *and* non-empty: `/sys/class/backlight` exists on a desktop with no backlight behind it, and an empty directory means exactly what an absent one does to every caller here.
        Kind::Kernel { path } => found(
            Path::new(path)
                .read_dir()
                .is_ok_and(|mut entries| entries.next().is_some()),
        ),
        // SAFETY: the library is opened and dropped without a symbol being taken out of it — a probe only ever asks whether the loader can find it.
        Kind::Library { .. } => found(unsafe { open_library(entry.dep, None, Ok) }.is_some()),
        // The only kind that can answer `Unknown`: with no compositor to ask, "does it advertise this" has no answer, and inventing `Absent` would blame the compositor for this process having no session.
        Kind::Protocol { interfaces } => match platform_wayland::advertises_all(interfaces) {
            Some(yes) => found(yes),
            None => Presence::Unknown,
        },
    }
}

/// Whether anything owns `name`, or could be started to.
///
/// Both halves matter: most desktop services are D-Bus activatable, so "nobody owns it right now" is not the same as "it is not installed" — asking only the first would report a perfectly good fprintd as missing until something woke it.
fn bus_name_exists(name: &str, system: bool) -> Presence {
    // No bus to ask is the protocol case again: a machine with no session bus has not told us the peer is missing, only that nothing here could ask.
    let bus = if system {
        crate::live::system_bus()
    } else {
        crate::live::session_bus()
    };
    let Some(Ok(connection)) = bus.map(|builder| builder.build()) else {
        return Presence::Unknown;
    };
    let call = |method: &str| {
        connection.call_method(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            Some("org.freedesktop.DBus"),
            method,
            &(),
        )
    };
    for method in ["ListNames", "ListActivatableNames"] {
        if let Ok(reply) = call(method)
            && let Ok(names) = reply.body().deserialize::<Vec<String>>()
            && names.iter().any(|owned| owned == name)
        {
            return Presence::Present;
        }
    }
    Presence::Absent
}

/// Runs a declared program and returns its standard output.
///
/// The only way to run one. Taking a [`Dep`] rather than a name is what makes the list in this file complete by construction: a program with no row cannot be reached from here, and a row carries everything the dependency panel needs to say about it.
pub fn output(dep: Dep, args: &[&str], timeout: Duration) -> Option<String> {
    let entry = entry(dep);
    if entry.sealed() {
        return None;
    }
    let program = entry.program()?;
    process::output(program, args, timeout)
}

/// Whether a declared dependency is usable — the question a service asks before bothering to start.
///
/// Flattens `Unknown` to `false` on purpose, and only here: a caller deciding whether to shell out needs a yes or a no, and "I could not tell" has to mean "do not try". The three-state answer is for the *report*, where the distinction is the whole value.
pub fn available(dep: Dep) -> bool {
    probe(dep).is_present()
}

/// Where a declared library might be, in the order the loader should be asked — empty for a row that is not a library. For the one caller that has to *name* what it tried in a message a user will read.
pub fn library_names(dep: Dep) -> &'static [&'static str] {
    match entry(dep).kind {
        Kind::Library { sonames } => sonames,
        _ => &[],
    }
}

/// Opens a declared library and builds something out of its symbols, trying each candidate name in turn.
///
/// The library twin of [`command`], and load-bearing for the same reason: taking a [`Dep`] rather than a soname is what stops a second copy of a candidate list existing somewhere else, which is exactly how `libpam`'s list came to be written twice and NVML's not to be declared at all.
///
/// `build` receives the opened library and returns the caller's own handle, holding it so the symbols stay mapped. Returning `Err` rejects *that* candidate and moves to the next, which is what lets a caller refuse a library that opens but has no usable symbols — or one whose own initialiser fails.
///
/// `preferred` is tried ahead of the row, for the single case where a user can point at a library the row cannot know about: `[lock] pam_library`.
///
/// # Safety
///
/// Opening a shared object runs its initialisers, and the symbols `build` takes out of it are trusted to match the signatures the caller declares — neither is something the type system checks.
pub unsafe fn open_library<T>(
    dep: Dep,
    preferred: Option<&str>,
    build: impl Fn(libloading::Library) -> Result<T, libloading::Error>,
) -> Option<T> {
    if entry(dep).sealed() {
        return None;
    }
    let candidates = preferred
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .into_iter()
        .chain(library_names(dep).iter().copied());
    for name in candidates {
        match unsafe { libloading::Library::new(name) }.and_then(&build) {
            Ok(built) => {
                tracing::debug!("{}: loaded from '{name}'", entry(dep).id);
                return Some(built);
            }
            Err(reason) => tracing::debug!("{}: '{name}' did not load: {reason}", entry(dep).id),
        }
    }
    None
}

/// A [`Command`](std::process::Command) for a declared program, for the callers that must own the child rather than wait for its output: a graph monitor that streams for the life of the shell, or a recorder that runs until it is stopped.
///
/// `None` for a row that is not a program, which is what stops a bus name or a sysfs path being spawned.
pub fn command(dep: Dep) -> Option<std::process::Command> {
    let entry = entry(dep);
    if entry.sealed() {
        return None;
    }
    entry.program().map(process::command)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// The guard that makes [`entry`] total, and the reason adding a `Dep` without a row is a test failure rather than a panic in front of a user.
    #[test]
    fn every_dep_has_exactly_one_row() {
        for entry in ALL {
            let matching = ALL.iter().filter(|other| other.dep == entry.dep).count();
            assert_eq!(matching, 1, "{:?} has {matching} rows", entry.dep);
        }
    }

    #[test]
    fn every_row_says_what_it_is_for_and_what_its_absence_costs() {
        for entry in ALL {
            assert!(!entry.id.trim().is_empty(), "{:?} has no id", entry.dep);
            assert!(
                !entry.what.trim().is_empty() && !entry.without.trim().is_empty(),
                "{} must say what it is for and what breaks without it",
                entry.id
            );
            // The panel prints these as sentences beside each other; an id that is not a package-ish name is a row a user cannot act on.
            assert!(
                entry
                    .id
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '.'),
                "'{}' is not a usable id",
                entry.id
            );
        }
    }

    #[test]
    fn ids_are_unique() {
        for entry in ALL {
            let matching = ALL.iter().filter(|other| other.id == entry.id).count();
            assert_eq!(matching, 1, "'{}' is used by {matching} rows", entry.id);
        }
    }

    /// "Required" has one meaning — the process does not start — so the list of them must stay tiny and deliberate. Growing it is a decision someone makes here rather than something a green run hides.
    #[test]
    fn required_means_the_shell_does_not_start() {
        let required: Vec<&str> = ALL
            .iter()
            .filter(|entry| entry.need == Need::Required)
            .map(|entry| entry.id)
            .collect();
        assert_eq!(
            required,
            vec!["wlr-layer-shell"],
            "everything else degrades, by the standing rule that no feature may hard-require a daemon \
             without a graceful path"
        );
    }

    #[test]
    fn a_program_row_names_its_program_and_the_others_do_not() {
        assert_eq!(entry(Dep::Ddcutil).program(), Some("ddcutil"));
        assert_eq!(entry(Dep::BlueZ).program(), None);
        assert_eq!(entry(Dep::Backlight).program(), None);
        // Which is what makes `output` refuse a dep that is not a program rather than running something odd.
        assert_eq!(output(Dep::BlueZ, &[], Duration::from_millis(1)), None);
    }

    /// A kernel interface that exists but is empty answers the same as one that is absent, because that is what it means to every caller: a desktop with no backlight has the directory and nothing in it.
    #[test]
    fn an_empty_kernel_directory_reads_as_absent() {
        let absent = Entry {
            kind: Kind::Kernel {
                path: "/sys/class/hogar-shell-no-such-class-9e3f",
            },
            ..*entry(Dep::Backlight)
        };
        assert_eq!(run_probe(&absent), Presence::Absent);
    }

    #[test]
    fn a_probe_is_remembered_and_can_be_forgotten() {
        refresh();
        let first = probe(Dep::PowerSupply);
        assert_eq!(
            probe(Dep::PowerSupply),
            first,
            "the second ask is the cache"
        );
        refresh();
        assert_eq!(
            probe(Dep::PowerSupply),
            first,
            "and it probes to the same answer"
        );
    }

    /// A registry is only the source of truth if it cannot be bypassed, and in Rust nothing stops a new call site reaching for the standard library directly — at which point the dependency panel goes on cheerfully reporting a list that is missing what the shell just failed to find.
    ///
    /// Two front doors, one rule. Programs go through [`command`], which takes a [`Dep`]; libraries go through [`open_library`], which takes one too. Both guards live here rather than beside the code they police, because what they protect is this file's completeness.
    ///
    /// One spelling is allowed past the process guard beyond `process`'s own: `process::command(…)` at a site that runs a command the **user** wrote — a launcher action, a scheme hook, the configured annotator or `howdy` line. Those have no row because there is nothing stable to put in one.
    #[test]
    fn nothing_reaches_outside_this_process_without_a_row() {
        for (needle, exempt, fix) in [
            (
                "Command::new",
                "util/src/process.rs",
                "use `deps::command(Dep::…)`, or `process::command` if it is a command the user wrote",
            ),
            (
                "libloading::Library::new",
                "util/src/deps.rs",
                "use `deps::open_library(Dep::…)`",
            ),
        ] {
            let offenders = sources_containing(needle, &[exempt]);
            assert!(
                offenders.is_empty(),
                "these reach outside without declaring it — {fix}: {offenders:#?}"
            );
        }
    }

    /// Walks the workspace's own sources for `needle`, skipping `exempt`, this file — which holds every needle as a literal — and the transpiler's output, which is generated rather than written.
    pub(crate) fn sources_containing(needle: &str, exempt: &[&str]) -> Vec<String> {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(std::path::Path::parent)
            .expect("the workspace root is two levels above this crate")
            .to_path_buf();
        let mut offenders = Vec::new();
        let mut stack = vec![root.join("crates"), root.join("apps")];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = dir.read_dir() else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    if !matches!(path.file_name().and_then(|n| n.to_str()), Some(".telar")) {
                        stack.push(path);
                    }
                    continue;
                }
                let is_source = path
                    .extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| e == "rs" || e == "rsx");
                if !is_source
                    || path.ends_with("util/src/deps.rs")
                    || exempt.iter().any(|e| path.ends_with(e))
                {
                    continue;
                }
                if std::fs::read_to_string(&path).is_ok_and(|text| text.contains(needle)) {
                    offenders.push(
                        path.strip_prefix(&root)
                            .unwrap_or(&path)
                            .display()
                            .to_string(),
                    );
                }
            }
        }
        offenders.sort();
        offenders
    }

    /// Not an assertion about this machine — only that probing every row answers, in order, without panicking on a kind whose probe is missing.
    #[test]
    fn every_row_can_be_probed() {
        let statuses = snapshot();
        assert_eq!(statuses.len(), ALL.len());
        for (status, entry) in statuses.iter().zip(ALL) {
            assert_eq!(status.dep, entry.dep, "snapshot follows ALL's order");
        }
    }
}
