telar::rsx_modules!(::config::theme::NordTheme);

// What the `hyprshell` binary reaches for; everything else now belongs to the crate that owns it.
pub use crate::core::commands::describe as ipc_describe;
pub use crate::core::commands::dispatch_locally;
pub use crate::core::ipc::call as ipc_call;
pub use crate::core::man::FORMS as USAGE_FORMS;
pub use config::schema::render as config_schema;

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use config::Config;
use platform_wayland::LayerShellPlatform;
use telar::{App, AppPathsProvider, run_multi_with_platform};

use surfaces::reconcile::{Content, Surfaces};

/// Every crate's `rsx_modules!` emits its own `telar_all_preview_entries`, so the list is per crate rather than
/// per process and the app is the only place that has all eight.
///
/// The second list is the previews written in Rust: a surface's content is built by a function, not by a `.rsx`
/// component, so there is no `[preview]` block to hang one off. They are entries of exactly the same kind —
/// `cargo telar preview`/`test` cannot tell the two apart — and each replaces a `TELAR_VISUAL_*` test that only
/// rendered when an environment variable asked it to.
fn preview_entries() -> Vec<telar::PreviewEntry> {
    let mut entries = telar_all_preview_entries();
    for crate_entries in [
        config::telar_all_preview_entries,
        modules::telar_all_preview_entries,
        services::telar_all_preview_entries,
        settings::telar_all_preview_entries,
        surfaces::telar_all_preview_entries,
        ui::telar_all_preview_entries,
        util::telar_all_preview_entries,
    ] {
        entries.extend(crate_entries());
    }
    for hand_written in [
        modules::preview::entries,
        settings::preview::entries,
        surfaces::preview::entries,
        ui::preview::entries,
    ] {
        entries.extend(hand_written());
    }
    entries
}

/// The page a preview is rendered on. Wider and taller than telar's 800×600 default because half of what this
/// app previews is a whole surface — a 920×680 settings float, a bar the width of a screen — and a preview
/// clipped by the page shows a layout problem that isn't there.
fn preview_window() -> telar::AppConfig {
    telar::AppConfig {
        window: telar::WindowConfig {
            width: 1000,
            height: 760,
            ..telar::WindowConfig::default()
        },
        ..telar::AppConfig::default()
    }
}

/// The ambient world a `[preview]` builds against: config, locale, font, icon store and theme. Deliberately not
/// `apply_config` — that also arms the idle stages, the notification policy and the toast watchers, which reach
/// the machine and have no business running to render a component.
///
/// [`install_hooks`] is part of it because three of the previews are surfaces that dispatch by module id — the
/// bar, the drawer and the popout all ask a registry what to draw, and a registry nobody installed answers
/// "nothing". It publishes tables and function pointers and starts nothing.
fn seed_preview_world() {
    let config = Arc::new(Config::load_or_default(&Config::default_path()));
    services::locale::init(config.language());
    telar::set_default_font_family(config.theme.font_family.clone());
    ui::icon::init_store(&config.icons);
    telar::set_theme(config.resolve_theme());
    config::set_config(config);
    install_hooks();
}

pub fn run() {
    // `cargo telar preview`/`test` has to answer while a shell is already up, so this precedes the single-instance check below — it opens no surface and takes no bus name.
    if telar::dev_entry(preview_entries, preview_window(), seed_preview_world) {
        return;
    }
    // One shell per compositor instance: a second one would fight over the notification bus name and the IPC
    // socket, and the user would see two of every bar. Checked before anything is opened so the failure is a
    // clean message rather than a half-started shell.
    if crate::core::ipc::another_instance_is_running() {
        eprintln!(
            "hyprshell: already running (IPC socket {} is live). Use `hyprshell shell quit` to stop it.",
            crate::core::ipc::socket_path().display()
        );
        std::process::exit(1);
    }
    let config_path = Config::default_path();
    // Start the notification daemon once, before the reload loop, so it keeps owning the D-Bus name across bar
    // config reloads (§8 "persists across reloads"). The popup surface itself is (re)established inside
    // `run_once` on the driver thread — notification state lives in the daemon, so recreating it loses nothing.
    let initial = Arc::new(Config::load_or_default(&config_path));
    // Seed the shared UI-language source so every surface starts in the configured locale and stays live.
    services::locale::init(initial.language());
    // Process-wide so every surface — bars, drawers, popups, OSD — renders in the theme's font family.
    // `run_once` re-applies (and warns) on every reload, so the popup host spawned here also gets it.
    telar::set_default_font_family(initial.theme.font_family.clone());
    services::notifications::init(notification_policy(&initial));

    // Non-destructive reload: one persistent driver. Every surface is opened dynamically on the driver thread
    // (via `setup_shell`, deferred with `run_on_start`) and reconciled on config change, so a reload never tears
    // down the connection, the popup, or the shared services — only the surfaces that changed.
    platform_wayland::run_on_start(move || setup_shell(config_path));
    if let Err(e) = run_multi_with_platform(
        LayerShellPlatform::new(),
        Vec::new(),
        |_| std::sync::Arc::new(util::paths::ShellPaths) as std::sync::Arc<dyn AppPathsProvider>,
        |_id| -> Box<dyn App> { unreachable!("hyprshell opens every surface dynamically") },
        "hyprshell",
    ) {
        eprintln!("hyprshell exited with error: {e}");
        std::process::exit(1);
    }
}

/// Runs on the driver thread once its loop is up (deferred via `run_on_start`): brings up the popup host and
/// the shell's own surfaces, then watches the config file and reconciles them on change — in place, without
/// tearing the driver, the connection, the popup, the services or the surfaces themselves down.
fn setup_shell(config_path: PathBuf) {
    install_hooks();
    let config = Arc::new(Config::load_or_default(&config_path));
    apply_config(&config);
    if platform_wayland::outputs().is_empty() {
        eprintln!("hyprshell: no Wayland outputs found (is a compositor running?)");
        std::process::exit(1);
    }

    // The one column of cards — notification popups, toasts, the OSD. Long-lived: set up once, it persists
    // across reloads, and it holds no surface at all until one of the three has something to say. The toast
    // watchers are installed by `apply_config`, which has already run, so an event switched on later gets its
    // watcher on the next reload.
    modules::stack::host();

    let surfaces = Rc::new(RefCell::new(Surfaces::default()));
    // The config the shell is currently running. A reload that fails to parse keeps this one rather than
    // falling back to the starter bar, so a typo costs the user an error message, not their whole layout.
    let live = Rc::new(RefCell::new(Arc::clone(&config)));

    // One reconciliation, shared by every trigger: re-read the config and bring the surfaces in line with it.
    // Driven by a config edit, by a monitor being plugged in or unplugged, and by `hyprshell shell reload` —
    // and it is the *same* pass at startup, so there is one description of what should be on screen rather
    // than an opening path and a reloading path that can disagree.
    let reconcile = {
        let surfaces = Rc::clone(&surfaces);
        let live = Rc::clone(&live);
        let config_path = config_path.clone();
        move |content: Content| {
            let (config, content) = match Config::load(&config_path) {
                Ok(config) => (Arc::new(config), content),
                Err(e) => {
                    report_config_error(&e);
                    // The file that would have changed what the surfaces draw does not parse, so nothing about
                    // them has changed: the last working config is reconciled for the surface *set* — a monitor
                    // may still have arrived — and every tree already drawing it is left alone.
                    (Arc::clone(&live.borrow()), Content::Keep)
                }
            };
            apply_config(&config);
            surfaces.borrow_mut().reconcile(
                &config_path,
                &config,
                &platform_wayland::outputs(),
                content,
            );
            if content == Content::Rebuild {
                // Everything else that is on screen, in place and in the same pass: the panels the user
                // opened over the chrome, and the notification popup that follows the focused screen. Each
                // keeps its surface — and what the user was in the middle of — and takes the new config.
                surfaces::shell::rebuild_all();
                modules::stack::reconcile_config();
            }
            *live.borrow_mut() = config;
        }
    };
    let reconcile = Rc::new(reconcile);

    // Reports what it did itself, at startup and at every reload alike — through tracing rather than
    // `println!`, because this runs on the driver thread where a direct write to a pipe nobody is draining
    // blocks forever. See `init_tracing`.
    surfaces.borrow_mut().reconcile(
        &config_path,
        &config,
        &platform_wayland::outputs(),
        Content::Rebuild,
    );

    // The config having changed, whoever noticed: the file watcher, `hyprshell shell reload`, a keybind. The
    // toast belongs here rather than in the reconcile, which also runs at startup — a toast saying the config
    // was reloaded is only true of a reload.
    let on_config_change: Rc<dyn Fn()> = {
        let reconcile = Rc::clone(&reconcile);
        Rc::new(move || {
            reconcile(Content::Rebuild);
            modules::toast::config_reloaded();
        })
    };

    config::set_reload_hook({
        let on_config_change = Rc::clone(&on_config_change);
        move || on_config_change()
    });

    // Live language switching. At app level, so the subscription outlives the surface rebuilds a reload does —
    // one taken out from inside a bar would be removed with that bar's sources on the first one.
    services::locale::follow_switches();

    // The idle timers are armed by `apply_config`, which has already run — one path for startup and reload,
    // so a saved `[idle]` re-arms without a second entry point that could disagree with it.
    for eager in eager_subscriptions(config_path, on_config_change) {
        tracing::debug!("subscribing at startup: {} — {}", eager.name, eager.reason);
        (eager.subscribe)();
    }
    // A monitor arriving or leaving changes which surfaces exist and nothing about what they draw, so the
    // screens that were already there keep the trees they have.
    platform_wayland::on_outputs_changed(move || reconcile(Content::Keep));
}

/// A subscription the shell takes out at startup, whatever the config asks for.
///
/// Every one of these is an *exemption* from the shell's standing rule that nothing runs unless something is
/// asking for it, so the set has to be small and has to stay small — and the way it stops staying small is that
/// adding to it costs one line in a function nobody diffs closely.
struct Eager {
    name: &'static str,
    /// Why this one cannot wait to be asked for. Data rather than a comment so that adding an entry means
    /// answering the question, and so [`every_startup_subscription_says_why_it_cannot_wait`] can require it.
    reason: &'static str,
    subscribe: Box<dyn FnOnce()>,
}

/// The whole exemption list, in the order it is taken out — which is load-bearing twice over: the command
/// surface comes first so a `shell reload` arriving immediately has a reload hook to call, and the lock
/// performer comes before the logind signals that can ask for a lock.
///
/// Built rather than run so a test can read it without a driver loop under it. Nothing here starts until its
/// `subscribe` is called, and `platform_wayland::watch` starts nothing at all without a loop handle.
fn eager_subscriptions(config_path: PathBuf, on_config_change: Rc<dyn Fn()>) -> Vec<Eager> {
    vec![
        Eager {
            name: "ipc",
            reason: "the command surface: a shell with no socket cannot be told to do anything, including to \
                     put a bar back",
            subscribe: Box::new(|| {
                platform_wayland::watch(crate::core::ipc::serve, crate::core::ipc::handle);
            }),
        },
        Eager {
            name: "shortcuts",
            reason: "the same request path fed by the desktop portal, so a bound key runs what `hyprshell …` \
                     would without a process launch per keypress; silently absent with no portal",
            subscribe: Box::new(|| {
                platform_wayland::watch(services::shortcuts::serve, crate::core::ipc::handle);
            }),
        },
        Eager {
            name: "scheme",
            reason: "a wallpaper-derived palette rebuilds every surface, so it outlives any one of them — and \
                     `scheme::CURRENT` is a `Store`, so this costs a channel and not a producer",
            subscribe: Box::new(|| {
                platform_wayland::watch(config::scheme::subscribe, config::scheme::on_change);
            }),
        },
        Eager {
            name: "battery",
            reason: "low-battery warnings must fire whether or not the user put a battery chip on a bar, and \
                     the producer retires on a machine with no battery to read",
            subscribe: Box::new(|| {
                platform_wayland::watch(
                    services::battery::subscribe,
                    services::battery::on_reading,
                );
            }),
        },
        Eager {
            name: "lock",
            reason: "the performer has to be listening before anything can ask for a lock, and a reload must \
                     not tear it down — one that dropped would put the desktop back on screen",
            subscribe: Box::new(|| {
                platform_wayland::watch(services::lock::subscribe, services::lock::on_state);
            }),
        },
        Eager {
            name: "session",
            reason: "logind's signals are how `loginctl lock-session` and a suspend reach the lock, and this \
                     one holds the sleep inhibitor that makes a suspend wait rather than race",
            subscribe: Box::new(|| {
                platform_wayland::watch(services::session::watch, services::session::on_event);
            }),
        },
        Eager {
            name: "config-watcher",
            reason: "the file the user edits while the shell runs; nothing else would notice an edit",
            subscribe: Box::new(move || {
                platform_wayland::watch(
                    move |tx| watch_config_changes(config_path, tx),
                    move |_| on_config_change(),
                );
            }),
        },
    ]
}

/// The three answers the layers below cannot reach on their own, handed to them once on the driver thread.
///
/// Each is a case of something low in the stack needing something high in it: the config derives a palette from
/// a wallpaper only the wallpaper *service* can name; a service that runs `[idle]` actions needs the command
/// table, which lives with the socket above it; and the lock service owns *when* the session is locked, never
/// what the covered screen draws. Installed before the first config is applied, since applying one derives a
/// scheme and arms the idle stages.
fn install_hooks() {
    config::set_wallpaper_source(|config| {
        let focused = surfaces::shell::focused_output();
        services::wallpaper::current_image(config, focused.as_deref())
    });
    services::command::set_runner(
        crate::core::commands::dispatch,
        crate::core::commands::resolves,
    );
    services::lock::set_session_opener(|| {
        let config = config::config();
        platform_wayland::lock_session(move |output| modules::lock::LockApp {
            config: config.clone(),
            output,
        })
    });
    ui::module::set_panel_opener(surfaces::panel::open_panel);
    // Published together because they check each other: a chip is wired for a hover card from the card list, and one that opens a panel is checked against the panel list.
    let popouts = crate::core::popouts::default_popouts();
    ui::module::install(crate::core::registry::default_registry(&popouts));
    ui::popouts::install(popouts);
    ui::panels::install(crate::core::panels::default_panels());
}

/// Everything a config change affects outside the surfaces themselves: the UI language, the process-wide font,
/// the icon store, and the context that code reached from outside a surface resolves against.
///
/// Called from the driver thread at app level — deliberately not from inside a surface build, since the icon
/// store's download worker must outlive any single surface (see [`shared::icon::init_store`]).
fn apply_config(config: &Arc<Config>) {
    services::locale::init(config.language());
    warn_if_font_missing(config.theme.font_family.as_deref());
    telar::set_default_font_family(config.theme.font_family.clone());
    config::set_config(Arc::clone(config));
    ui::icon::init_store(&config.icons);
    // After `set_config`: deriving a palette needs to know which wallpaper is up, and that answer comes from the
    // config that was just published. Cheap when the palette is already cached, which is every start after the
    // first; a miss quantises the image on a thread of its own and lands through the scheme watcher below.
    config::scheme::init(config);
    // The surfaces this reload is about to open will carry whatever `init` just resolved, so the watcher must
    // not read the delivery that follows as a change and ask for a second, identical reload.
    config::scheme::mark_painted();
    // After `set_config`, so the stages are armed from the config that was just published rather than the one
    // they were armed from last time.
    services::idle::reconcile();
    // The daemon outlives every reload, so an edited `[notifications]` reaches it this way rather than by restarting it — which would drop the bus name and the history with it.
    services::notifications::set_policy(notification_policy(config));
    // The toast watchers a switched-on event needs. Additive and idempotent: a subscription cannot be undone, so
    // this installs what is missing and leaves the rest — an event switched *off* is silenced by the toaster's own
    // gate rather than by tearing its watcher down.
    modules::toast::watch_events(config);
}

/// The daemon's slice of the config, resolved in one place so startup and reload agree on it. The timeout is the
/// column's — a notification, a toast and an OSD all go after `[stack] timeout_ms` — while what is particular to
/// a notification stays under `[notifications]`.
fn notification_policy(config: &Config) -> services::notifications::Policy {
    services::notifications::Policy {
        timeout: config.stack.lifetime(),
        critical_sticky: config.notifications.critical_sticky,
        critical_max: config.notifications.critical_ceiling(),
        sound: config.notifications.sound.clone(),
    }
}

/// Tells the user their edit didn't take, through the shell's own notification daemon so the message lands on
/// screen rather than in a log they aren't reading. Falls back to stderr when the daemon isn't up yet.
fn report_config_error(error: &config::LoadError) {
    let message = error.to_string();
    tracing::warn!("{message}; keeping the last working config");
    services::notifications::notify_local("hyprshell", &telar::t!("config.error_title"), &message);
}

/// The config-watch producer for `watch`: polls the config's mtimes (dependency-free, naturally debounced) and
/// sends a tick on each change so the driver thread reconciles the surface set.
fn watch_config_changes(path: PathBuf, tx: platform_wayland::EventSender<()>) {
    let mut last = config_fingerprint(&path);
    loop {
        std::thread::sleep(Duration::from_millis(500));
        let now = config_fingerprint(&path);
        if now != last {
            // An empty fingerprint means the main config went missing mid-edit (a rename-into-place, an editor's atomic save); reconciling against nothing would blank the screen for a moment.
            let settled = !now.is_empty();
            last = now;
            if settled && !tx.send(()) {
                return;
            }
        }
    }
}

/// The mtimes the shell's layout depends on: `config.toml` and every `monitors/<output>/config.toml`. The
/// per-monitor files are part of the same answer, so editing one has to trigger the same reload as editing the
/// global file — a watcher that only knew about `config.toml` would leave a monitor override needing a restart.
fn config_fingerprint(path: &Path) -> Vec<(PathBuf, SystemTime)> {
    let mut stamps: Vec<(PathBuf, SystemTime)> = config_mtime(path)
        .map(|t| (path.to_path_buf(), t))
        .into_iter()
        .collect();
    let Ok(entries) = std::fs::read_dir(Config::monitor_dir(path)) else {
        return stamps;
    };
    for entry in entries.flatten() {
        let file = entry.path().join("config.toml");
        if let Some(stamp) = config_mtime(&file) {
            stamps.push((file, stamp));
        }
    }
    stamps.sort();
    stamps
}

fn config_mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// Logs whether a configured `[theme] font_family` resolves against the installed fonts. A wrong family name
/// (e.g. `"Fira Code Nerd Font"` instead of the installed `"FiraCode Nerd Font"`) otherwise falls back to the
/// default font silently; this turns that into a visible log line.
///
/// The verdict comes from the shaper's own database, so a hit here means the shell will actually render in that
/// family — where the second `fontdb` this used to load could answer differently, and cost a full font scan to
/// do it. Each family's verdict is still remembered, because every save from the settings panel reloads the
/// config and asks again.
fn warn_if_font_missing(family: Option<&str>) {
    let Some(family) = family else { return };
    thread_local! {
        static CHECKED: RefCell<std::collections::HashMap<String, bool>> =
            RefCell::new(std::collections::HashMap::new());
    }
    if CHECKED.with(|c| c.borrow().contains_key(family)) {
        return;
    }
    let found = telar::font_family_available(family);
    CHECKED.with(|c| c.borrow_mut().insert(family.to_string(), found));
    if found {
        tracing::info!("theme font_family '{family}' resolved");
    } else {
        tracing::warn!(
            "theme font_family '{family}' is not installed; using the default font. List exact names with `fc-list : family`."
        );
    }
}

#[cfg(test)]
mod startup_tests {
    use super::*;

    /// The exemption list, pinned.
    ///
    /// Everything else in the shell waits to be asked: a service starts when a chip subscribes and stops when
    /// the last one goes. These seven do not, so they are the one way an idle shell can start working again —
    /// and the way that set grows is one more line in a startup function, which reads as nothing in a diff.
    /// Pinning the names is what turns that into a decision someone has to make on purpose.
    ///
    /// Reading the list runs none of it. `subscribe` is never called here, and could not do anything if it
    /// were: `platform_wayland::watch` returns without spawning a producer when there is no loop handle under
    /// it, which is also why "nothing is running" cannot be asserted from a test at all.
    #[test]
    fn every_startup_subscription_says_why_it_cannot_wait() {
        let eager = eager_subscriptions(PathBuf::from("config.toml"), Rc::new(|| {}));

        assert_eq!(
            eager.iter().map(|e| e.name).collect::<Vec<_>>(),
            [
                "ipc",
                "shortcuts",
                "scheme",
                "battery",
                "lock",
                "session",
                "config-watcher"
            ],
            "the set of subscriptions exempt from 'nothing runs unless something asks' changed — which is a \
             decision, not a detail: add the entry with the reason it cannot wait, or take it off the list"
        );

        for entry in &eager {
            assert!(
                entry.reason.len() > 30,
                "'{}' is exempt without saying why",
                entry.name
            );
        }
    }
}

#[cfg(test)]
mod i18n_tests {
    /// The shell's own catalog resolves, and a locale switch changes what it answers. The modules' catalogs are
    /// checked in their own crate — a `t!` key is resolved against the catalog of the crate that writes it.
    #[test]
    fn catalog_translates_and_switches() {
        telar::set_locale("en");
        assert_eq!(telar::t!("config.error_title"), "Configuration not applied");
        telar::set_locale("es");
        assert_eq!(telar::t!("config.error_title"), "Configuración no aplicada");
    }
}
