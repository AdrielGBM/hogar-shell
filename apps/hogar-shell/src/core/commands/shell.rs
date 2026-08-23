//! `hogar-shell shell`, `config`, `scheme` and `session` — the shell talking about itself.

use std::time::Duration;

use super::args::*;
use super::{Command, Target};
use crate::core::ipc::request_quit;

pub(crate) const SHELL: Target = Target {
    name: "shell",
    commands: &[
        Command {
            name: "ping",
            args: "",
            help: "check the shell is alive",
            run: |_| Ok("pong".to_string()),
        },
        Command {
            name: "version",
            args: "",
            help: "the running build's version",
            run: |_| Ok(env!("CARGO_PKG_VERSION").to_string()),
        },
        Command {
            name: "reload",
            args: "",
            help: "re-read config.toml and rebuild every surface",
            run: |_| {
                config::request_reload();
                Ok("reloaded".to_string())
            },
        },
        Command {
            name: "status",
            args: "",
            help: "what the shell is holding: surfaces, threads and running services",
            run: |_| Ok(status()),
        },
        Command {
            name: "caches",
            args: "",
            help: "what the renderer's caches hold, per cache: bytes, entries and budget",
            run: |_| Ok(caches()),
        },
        Command {
            name: "reclaim",
            args: "",
            help: "drop idle renderer caches and hand freed heap pages back to the kernel",
            run: |_| Ok(reclaim()),
        },
        Command {
            name: "outputs",
            args: "",
            help: "list the compositor's monitors",
            run: |_| {
                let names: Vec<String> = platform_wayland::outputs()
                    .into_iter()
                    .filter_map(|o| o.name)
                    .collect();
                Ok(names.join("\t"))
            },
        },
        Command {
            name: "screens",
            args: "",
            help: "the compositor's monitors with mode, scale and make",
            run: |_| {
                use services::hyprland;
                let dir = hyprland::socket_dir().ok_or("not running under Hyprland")?;
                let rows: Vec<String> = hyprland::current_screens()
                    .unwrap_or_else(|| hyprland::screens(&dir))
                    .iter()
                    .map(|s| {
                        format!(
                            "{}\t{}x{}@{:.2}\t{:.2}x\t{} {}",
                            s.name, s.width, s.height, s.refresh, s.scale, s.make, s.model
                        )
                    })
                    .collect();
                Ok(rows.join("\n"))
            },
        },
        Command {
            name: "clients",
            args: "",
            help: "every open window: address, workspace, class and title",
            run: |_| {
                use services::hyprland;
                let dir = hyprland::socket_dir().ok_or("not running under Hyprland")?;
                let rows: Vec<String> = hyprland::current_clients()
                    .unwrap_or_else(|| hyprland::clients(&dir))
                    .iter()
                    .map(|c| format!("{}\t{}\t{}\t{}", c.address, c.workspace, c.class, c.title))
                    .collect();
                Ok(rows.join("\n"))
            },
        },
        Command {
            name: "dpms",
            args: "<on|off>",
            help: "switch every monitor's output on or off",
            run: |args| {
                let on = match arg(args, 0, "state")? {
                    "on" => true,
                    "off" => false,
                    other => return Err(format!("expected on|off, got '{other}'")),
                };
                services::power::set_screens(on)?;
                Ok(on_off(on).to_string())
            },
        },
        Command {
            name: "quit",
            args: "",
            help: "shut the shell down",
            run: |_| {
                request_quit();
                Ok("bye".to_string())
            },
        },
    ],
};

pub(crate) const SESSION: Target = Target {
    name: "session",
    commands: &[
        Command {
            name: "list",
            args: "",
            help: "which session actions this machine supports",
            run: |_| {
                let ids: Vec<&str> = services::session::available()
                    .iter()
                    .map(|a| a.id())
                    .collect();
                Ok(ids.join("\t"))
            },
        },
        Command {
            name: "do",
            args: "<lock|logout|suspend|hibernate|reboot|shutdown>",
            help: "perform a session action",
            run: |args| {
                use services::session;
                let id = arg(args, 0, "action")?;
                let action = session::Action::from_id(id)
                    .ok_or_else(|| format!("unknown session action '{id}'"))?;
                if !session::is_available(action) {
                    return Err(format!("this machine cannot '{id}'"));
                }
                session::perform(action);
                Ok(id.to_string())
            },
        },
    ],
};

pub(crate) const SCHEME: Target = Target {
    name: "scheme",
    commands: &[
        Command {
            name: "status",
            args: "",
            help: "the palette in use: theme, mode, variant and the wallpaper it came from",
            run: |_| {
                use config::scheme;
                let config = config::config().ok_or("the shell is not running")?;
                let (mode, variant) = config.scheme_selection();
                let source = match scheme::current() {
                    Some(current) => current.source.display().to_string(),
                    None => "-".to_string(),
                };
                Ok(format!(
                    "{}\t{}\t{}\t{source}",
                    config.theme.name,
                    mode.id(),
                    variant.id()
                ))
            },
        },
        Command {
            name: "colors",
            args: "",
            help: "every token of the palette now on screen, as name and hex",
            run: |_| {
                let config = config::config().ok_or("the shell is not running")?;
                Ok(palette_rows(&config.resolve_theme()))
            },
        },
        Command {
            name: "list",
            args: "",
            help: "the palettes `scheme set` accepts",
            run: |_| {
                let mut names: Vec<&str> = config::theme::BUILT_IN_THEMES.to_vec();
                names.push("custom");
                names.push(config::scheme::DYNAMIC);
                Ok(names.join("\t"))
            },
        },
        Command {
            name: "set",
            args: "<name>",
            help: "switch palette, `dynamic` for one built from the wallpaper",
            run: |args| {
                use config::scheme::{self, Choice};
                scheme::apply(Choice::Palette, arg(args, 0, "name")?)
            },
        },
        Command {
            name: "mode",
            args: "[dark|light|auto|toggle]",
            help: "read or set the light/dark mode",
            run: |args| {
                use config::scheme::{self, Mode};
                let config = config::config().ok_or("the shell is not running")?;
                let Some(wanted) = args.first().copied() else {
                    return Ok(config.theme.mode.clone());
                };
                let next = match wanted {
                    "auto" => "auto".to_string(),
                    // Toggling resolves the *effective* mode first: `auto` is not a third state to flip
                    // through, it is "whatever the palette is", and a user pressing a toggle means the
                    // other one.
                    "toggle" => match Mode::of(&config.resolve_theme()) {
                        Mode::Dark => Mode::Light,
                        Mode::Light => Mode::Dark,
                    }
                    .id()
                    .to_string(),
                    other => Mode::from_id(other)
                        .ok_or_else(|| format!("expected dark|light|auto|toggle, got '{other}'"))?
                        .id()
                        .to_string(),
                };
                scheme::apply(scheme::Choice::Mode, &next)
            },
        },
        Command {
            name: "variant",
            args: "[name]",
            help: "read or set how much colour a dynamic scheme carries",
            run: |args| {
                use config::scheme::{self, Variant};
                let Some(wanted) = args.first().copied() else {
                    let config = config::config().ok_or("the shell is not running")?;
                    return Ok(config.theme.variant.clone());
                };
                let variant = Variant::from_id(wanted).ok_or_else(|| {
                    let known: Vec<&str> = Variant::ALL.iter().map(|v| v.id()).collect();
                    format!(
                        "unknown variant '{wanted}', expected one of {}",
                        known.join("|")
                    )
                })?;
                scheme::apply(scheme::Choice::Variant, variant.id())
            },
        },
        Command {
            name: "refresh",
            args: "",
            help: "re-derive a dynamic palette from the current wallpaper",
            run: |_| {
                let config = config::config().ok_or("the shell is not running")?;
                if !config.theme.is_dynamic() {
                    return Err("[theme] name is not 'dynamic'".to_string());
                }
                config::scheme::refresh(&config, Duration::ZERO);
                Ok("refreshing".to_string())
            },
        },
        Command {
            name: "export",
            args: "",
            help: "write the palette out for the rest of the desktop, ignoring [theme.export] enabled",
            run: |_| {
                use config::scheme;
                let config = config::config().ok_or("the shell is not running")?;
                let current = scheme::current().ok_or("no dynamic palette has been derived yet")?;
                let export = config::SchemeExportConfig {
                    enabled: true,
                    ..config.theme.export.clone()
                };
                let dir = export.resolved_dir();
                scheme::export_scheme(&current, &export);
                Ok(dir.display().to_string())
            },
        },
    ],
};

pub(crate) const DEPS: Target = Target {
    name: "deps",
    commands: &[
        Command {
            name: "list",
            args: "",
            help: "every external dependency, and whether this machine has it",
            run: |_| Ok(render_deps(util::deps::snapshot(), false)),
        },
        Command {
            name: "missing",
            args: "",
            help: "only the ones that are absent, with what each absence costs",
            run: |_| Ok(render_deps(util::deps::snapshot(), true)),
        },
        Command {
            name: "check",
            args: "",
            help: "whether everything the shell cannot start without is present",
            run: |_| {
                let broken: Vec<&str> = util::deps::snapshot()
                    .iter()
                    .filter(|status| status.is_a_problem())
                    .map(|status| util::deps::entry(status.dep).id)
                    .collect();
                if broken.is_empty() {
                    Ok("every required dependency is present".to_string())
                } else {
                    Err(format!("missing and required: {}", broken.join(", ")))
                }
            },
        },
        Command {
            name: "refresh",
            args: "",
            help: "probe again, for something installed since the shell started",
            run: |_| {
                util::deps::refresh();
                Ok(format!("{} rechecked", util::deps::ALL.len()))
            },
        },
    ],
};

/// What the shell is holding right now, as a census a script can diff.
///
/// The shell's standing rule is that nothing runs, and nothing is resident, unless something is asking for it —
/// and unmeasured that regresses silently: a module switched off in a reload leaving its poller reading the
/// system for nobody is invisible from the outside, and was only ever findable in `top`. Every number here is
/// counted from the thing itself rather than tracked alongside it, so none of them can drift from what is true.
fn status() -> String {
    let services = util::broadcast::running_services();
    let mut out = format!(
        "surfaces  {}\nthreads   {}\nservices  {}\n",
        platform_wayland::live_surfaces(),
        live_threads()
            .map(|n| n.to_string())
            .unwrap_or_else(|| "unknown".to_string()),
        services.len()
    );
    for service in services {
        out.push_str(&format!("          {service}\n"));
    }
    out
}

/// Threads in this process, counted from the kernel rather than from anything the shell bookkeeps — a thread
/// leaked by a library the shell only calls into still shows up here.
fn live_threads() -> Option<usize> {
    Some(std::fs::read_dir("/proc/self/task").ok()?.count())
}

/// What the renderer is holding, one line per cache, so a growing process can be told from a growing cache.
///
/// RSS alone cannot answer that: a shell whose memory climbs looks identical whether the climb is in a text raster
/// cache doing its job or in something with no ceiling at all. Every figure comes from the cache itself, summed
/// across the threads that render, and is at most a second old — the caches are thread-local, so a rendering thread
/// leaves its own count where this can read it rather than this reaching across for a set it cannot see.
fn caches() -> String {
    let stats = telar::cache_stats();
    if stats.is_empty() {
        return "no frame has been rendered yet\n".to_string();
    }
    let total: usize = stats.iter().map(|stat| stat.bytes).sum();
    let mut out = String::new();
    for stat in &stats {
        out.push_str(&format!(
            "{:<18} {:>9} {:>8} entries  of {}\n",
            stat.name,
            kib(stat.bytes),
            stat.entries,
            kib(stat.capacity),
        ));
    }
    out.push_str(&format!("{:<18} {:>9}\n", "total", kib(total)));
    out
}

fn kib(bytes: usize) -> String {
    format!("{:.1} MiB", bytes as f64 / (1024.0 * 1024.0))
}

/// Gives back what the shell is holding but not using, and reports what that came to.
///
/// glibc keeps the pages it has freed inside its arenas rather than returning them to the kernel — which is
/// most of why a shell with no leak still climbs, since profiling put a quarter of its three million
/// allocations in the transient bucket, and that churn is what leaves arenas full of holes.
///
/// The renderer's caches used to be the other half of this and no longer are: each surface rasterises on its
/// own render thread, which sweeps its own caches once it has been idle for their horizon. A sweep from here
/// would reach neither those (they are thread-local to the render threads) nor anything else — the shell
/// never rasterises on the UI thread — and could not evict earlier than that horizon anyway.
///
/// A command rather than a timer because the shell already has somewhere to put it: `[[idle.stages]]` runs shell
/// commands, so `action = "shell reclaim"` on a stage makes this automatic at whatever idleness the user means.
fn reclaim() -> String {
    let before = rss_kb();
    trim_heap();
    let after = rss_kb();
    match (before, after) {
        (Some(before), Some(after)) => format!(
            "reclaimed {:.1} MiB ({:.1} → {:.1} MiB)",
            (before.saturating_sub(after)) as f64 / 1024.0,
            before as f64 / 1024.0,
            after as f64 / 1024.0,
        ),
        _ => "reclaimed".to_string(),
    }
}

/// Returns glibc's freed-but-retained pages to the kernel. A no-op anywhere `malloc_trim` does not exist.
fn trim_heap() {
    #[cfg(target_env = "gnu")]
    // SAFETY: `malloc_trim` takes a pad in bytes and touches only the allocator's own bookkeeping.
    unsafe {
        libc::malloc_trim(0);
    }
}

fn rss_kb() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    status
        .lines()
        .find_map(|line| line.strip_prefix("VmRSS:"))?
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

/// One line per dependency: whether it is there, what it is for, and — when it is not — what that costs.
///
/// Answered from the registry rather than from a written list, so it cannot describe a shell other than the one
/// running it.
fn render_deps(statuses: Vec<util::deps::Status>, only_missing: bool) -> String {
    use util::deps::{Need, Presence, entry};
    let mut out = String::new();
    for status in statuses {
        if only_missing && status.presence == Presence::Present {
            continue;
        }
        let row = entry(status.dep);
        let mark = match (status.presence, row.need) {
            (Presence::Present, _) => "ok     ",
            (Presence::Absent, Need::Required) => "MISSING",
            (Presence::Absent, Need::Optional) => "absent ",
            // Never a complaint: this process could not ask, which is not the same as the answer being no.
            (Presence::Unknown, _) => "unknown",
        };
        out.push_str(&format!("{mark} {:<22} {}\n", row.id, row.what));
        match status.presence {
            Presence::Absent => {
                out.push_str(&format!("{:8}{:<22} → {}\n", "", "", row.without));
            }
            Presence::Unknown => {
                out.push_str(&format!(
                    "{:8}{:<22} → nothing here could ask; run this from inside the session\n",
                    "", ""
                ));
            }
            Presence::Present => {}
        }
    }
    if out.is_empty() {
        out.push_str("nothing is missing\n");
    }
    out
}

pub(crate) const CONFIG: Target = Target {
    name: "config",
    commands: &[
        Command {
            name: "path",
            args: "",
            help: "where config.toml is read from",
            run: |_| Ok(config::Config::default_path().display().to_string()),
        },
        Command {
            name: "schema",
            args: "[section]",
            help: "the annotated default config, or one section of it",
            run: |args| {
                let section = args.first().copied().filter(|s| !s.is_empty());
                config::schema::render(section)
            },
        },
    ],
};

pub(crate) const MAN: Target = Target {
    name: "man",
    commands: &[
        Command {
            name: "commands",
            args: "",
            help: "hogar-shell(1) as roff, generated from this table",
            run: |_| Ok(crate::core::man::commands_page()),
        },
        Command {
            name: "config",
            args: "",
            help: "hogar-shell(5) as roff, generated from the config schema",
            run: |_| crate::core::man::config_page(),
        },
    ],
};
