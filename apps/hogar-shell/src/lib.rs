telar::rsx_modules!();

// What the `hogar-shell` binary reaches for; everything else now belongs to the crate that owns it.
pub use crate::core::commands::describe as ipc_describe;
pub use crate::core::commands::dispatch_locally;
pub use crate::core::ipc::call as ipc_call;
pub use crate::core::man::FORMS as USAGE_FORMS;
pub use config::schema::render as config_schema;

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use config::fingerprint::{Fingerprint, Reload, Stamp};
use config::{Config, LoadError};
use platform_wayland::LayerShellPlatform;
use telar::{App, AppPathsProvider, run_multi_with_platform};

use surfaces::reconcile::{Content, Surfaces};

/// Every crate's `rsx_modules!` emits its own `telar_all_preview_entries`, so the list is per crate rather than per process and the app is the only place that has all eight.
///
/// The second list is the previews written in Rust: a surface's content is built by a function, not by a `.rsx` component, so there is no `[preview]` block to hang one off. They are entries of exactly the same kind — `cargo telar preview`/`test` cannot tell the two apart — and each replaces a `TELAR_VISUAL_*` test that only rendered when an environment variable asked it to.
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

/// The page a preview is rendered on. Wider and taller than telar's 800×600 default because half of what this app previews is a whole surface — a 920×680 settings float, a bar the width of a screen — and a preview clipped by the page shows a layout problem that isn't there.
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

/// What every surface this shell opens shapes its text in. A family belongs to a surface's configuration now rather than to the process, and this is where the shell says which one — the same place, and the same three moments, that used to set the global.
fn surface_fonts(config: &Config) -> telar::AppConfig {
    telar::AppConfig {
        font_family: config.theme.font_family.clone(),
        ..telar::AppConfig::default()
    }
}

/// The ambient world a `[preview]` builds against: config, locale, font, icon store and theme. Deliberately not `apply_config` — that also arms the idle stages, the notification policy and the toast watchers, which reach the machine and have no business running to render a component.
///
/// [`install_hooks`] is part of it because three of the previews are surfaces that dispatch by module id — the bar, the drawer and the popout all ask a registry what to draw, and a registry nobody installed answers "nothing". It publishes tables and function pointers and starts nothing.
fn seed_preview_world() {
    let config = Arc::new(Config::load_or_default(&Config::default_path()));
    services::locale::init(config.language());
    platform_wayland::set_surface_fonts(surface_fonts(&config));
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
    // One shell per compositor instance: a second one would fight over the notification bus name and the IPC socket, and the user would see two of every bar. Checked before anything is opened so the failure is a clean message rather than a half-started shell.
    if crate::core::ipc::another_instance_is_running() {
        eprintln!(
            "hogar-shell: already running (IPC socket {} is live). Use `hogar-shell shell quit` to stop it.",
            crate::core::ipc::socket_path().display()
        );
        std::process::exit(1);
    }
    let config_path = Config::default_path();
    // Once, and here rather than on the driver thread: the locale, the fonts and the notification daemon start from it before the driver exists, and a second load for the surfaces could read a different file from the one they started with.
    let startup = load_at_startup(&config_path);
    services::locale::init(startup.config.language());
    platform_wayland::set_surface_fonts(surface_fonts(&startup.config));
    // Once, before the driver, so it keeps owning the D-Bus name across every reload; `apply_config` hands it an edited policy instead.
    services::notifications::init(notification_policy(&startup.config));

    // Non-destructive reload: one persistent driver. Every surface is opened dynamically on the driver thread (via `setup_shell`, deferred with `run_on_start`) and reconciled on config change, so a reload never tears down the connection, the popup, or the shared services — only the surfaces that changed.
    platform_wayland::run_on_start(move || setup_shell(config_path, startup));
    if let Err(e) = run_multi_with_platform(
        LayerShellPlatform::new(),
        Vec::new(),
        |_| std::sync::Arc::new(util::paths::ShellPaths) as std::sync::Arc<dyn AppPathsProvider>,
        |_id| -> Box<dyn App> { unreachable!("hogar-shell opens every surface dynamically") },
        "hogar-shell",
    ) {
        eprintln!("hogar-shell exited with error: {e}");
        std::process::exit(1);
    }
}

/// Runs on the driver thread once its loop is up (deferred via `run_on_start`): brings up the popup host and the shell's own surfaces, then watches the config file and reconciles them on change — in place, without tearing the driver, the connection, the popup, the services or the surfaces themselves down.
fn setup_shell(config_path: PathBuf, startup: Startup) {
    install_hooks();
    let Startup {
        config,
        baseline,
        applied,
        failed,
    } = startup;
    let config = Arc::new(config);
    apply_config(&config);
    let reloader = Rc::new(RefCell::new(Reloader::starting(
        config_path.clone(),
        Arc::clone(&config),
        applied,
        failed.as_ref(),
    )));
    if platform_wayland::outputs().is_empty() {
        eprintln!("hogar-shell: no Wayland outputs found (is a compositor running?)");
        std::process::exit(1);
    }

    // The one column of cards — notification popups, toasts, the OSD. Long-lived: set up once, it persists across reloads, and it holds no surface at all until one of the three has something to say. The toast watchers are installed by `apply_config`, which has already run, so an event switched on later gets its watcher on the next reload.
    modules::stack::host();

    // One pass brings the surfaces in line with a config — at startup, at every reload that applies one, and when the screens change — so there is one description of what should be on screen rather than an opening path and a reloading path that can disagree. It reports what it did itself, through tracing rather than `println!`, because this runs on the driver thread where a direct write to a pipe nobody is draining blocks forever. See `init_tracing`.
    let surfaces = Rc::new(RefCell::new(Surfaces::default()));
    surfaces.borrow_mut().reconcile(
        &config_path,
        &config,
        &platform_wayland::outputs(),
        Content::Rebuild,
    );

    // The config having changed, whoever noticed: the file watcher, `hogar-shell shell reload`, a keybind. The toast belongs here rather than in the surface pass, which also runs at startup — a toast saying the config was reloaded is only true of a reload, and only of one that applied something.
    let on_config_change: Rc<dyn Fn(Reload)> = {
        let reloader = Rc::clone(&reloader);
        let surfaces = Rc::clone(&surfaces);
        let config_path = config_path.clone();
        Rc::new(move |reload| {
            let Some((config, seen)) = reloader.borrow_mut().reload(reload) else {
                return;
            };
            apply_config(&config);
            surfaces.borrow_mut().reconcile(
                &config_path,
                &config,
                &platform_wayland::outputs(),
                Content::Rebuild,
            );
            // What the user opened and the column of cards are not the surface pass's to rebuild, so they take the new config here, in the same pass.
            surfaces::shell::rebuild_all(&seen, reload);
            modules::stack::reconcile_config();
            reloader.borrow_mut().applied(config, seen);
            modules::toast::config_reloaded();
        })
    };

    // Asked for rather than noticed, so it runs whatever the files hold: `shell reload` and a moved palette exist to deliver what no fingerprint of them covers.
    config::set_reload_hook({
        let on_config_change = Rc::clone(&on_config_change);
        move || on_config_change(Reload::Always)
    });

    // Live language switching. At app level, so the subscription outlives the surface rebuilds a reload does — one taken out from inside a bar would be removed with that bar's sources on the first one.
    services::locale::follow_switches();

    // The idle timers are armed by `apply_config`, which has already run — one path for startup and reload, so a saved `[idle]` re-arms without a second entry point that could disagree with it.
    for eager in eager_subscriptions(config_path.clone(), baseline, on_config_change) {
        tracing::debug!("subscribing at startup: {} — {}", eager.name, eager.reason);
        (eager.subscribe)();
    }
    // A monitor arriving or leaving changes which surfaces exist and nothing about what they draw, so the screens that were already there keep the trees they have.
    platform_wayland::on_outputs_changed(move || {
        let config = reloader.borrow().live();
        surfaces.borrow_mut().reconcile(
            &config_path,
            &config,
            &platform_wayland::outputs(),
            Content::Keep,
        );
    });
}

/// Where the shell starts from: the config it runs, and what the reload path measures against it.
struct Startup {
    config: Config,
    /// What the files held when the startup load read them — read before the load, so it can only be older than the config, never newer; or, on a fresh install, the starter config the load wrote, as the bytes it wrote. The watcher's first poll compares against this, which is what makes an edit landing between startup and that poll a change rather than part of the baseline.
    baseline: Fingerprint,
    /// `baseline` when the load parsed, and empty when it fell back to the starter config: a file that did not parse was never applied, so no content — not even those same bytes — counts as already on screen.
    applied: Stamp,
    /// Why `config.toml` did not load, when it did not and `config` is the starter config standing in for it. The problems notice says so from the first moment it can.
    failed: Option<LoadError>,
}

/// The startup load. `Config::load_or_seed` rather than `load_or_default`, because whether it parsed decides what `applied` starts as, and a file it had to write is stamped as the bytes it wrote; a file that does not parse is logged the way a reload logs one, then replaced by the starter config the way `load_or_default` would replace it.
///
/// **A fresh install's first poll is no change.** The fingerprint read before the load finds no `config.toml`, and the load then writes the starter config there; stamped with the first, the watcher's first look would find a file where there was none, and reload everything to say "config reloaded" about a config nobody wrote.
fn load_at_startup(config_path: &Path) -> Startup {
    let found = Fingerprint::read(config_path);
    match Config::load_or_seed(config_path) {
        Ok((config, seeded)) => {
            let baseline = match seeded {
                Some(starter) => Fingerprint::with_config(config_path, Some(&starter)),
                None => found,
            };
            let mut applied = Stamp::default();
            applied.record(baseline.clone());
            Startup {
                config,
                baseline,
                applied,
                failed: None,
            }
        }
        Err(e) => {
            report_config_error(&e);
            Startup {
                config: Config::starter(),
                baseline: found,
                applied: Stamp::default(),
                failed: Some(e),
            }
        }
    }
}

/// What the reload path carries from one reload to the next: the config the shell runs, and what the surfaces and the problems notice were last brought up to date with.
struct Reloader {
    config_path: PathBuf,
    /// The config the shell is running. A reload that fails to load keeps this one rather than falling back to the starter bar, so a typo costs the user a notice, not their whole layout.
    live: Arc<Config>,
    /// The content every surface was last built from, starting with what the startup load applied. Moved only by a config that loaded and was drawn everywhere, so after a typo it still names what is on screen, and putting the file back is nothing to reload.
    applied: Stamp,
    /// Whether the problems notice is showing a file that did not load: set by every load that fails, and cleared by the first report made without one.
    failing: bool,
}

impl Reloader {
    /// The reload path as startup leaves it, with the problems notice raised for what startup found — `failed` being why `config.toml` did not load, when `live` is the starter config standing in for it. Called right after the startup config is applied, so the notice speaks the language it set.
    fn starting(
        config_path: PathBuf,
        live: Arc<Config>,
        applied: Stamp,
        failed: Option<&LoadError>,
    ) -> Self {
        let mut reloader = Self {
            config_path,
            live,
            applied,
            failing: false,
        };
        reloader.report(failed);
        reloader
    }

    /// Reads the files and answers whether they hold a config for the shell to take: the config, and the content it was loaded from, for the caller to apply and then hand to [`applied`](Self::applied). `None` when there is nothing to take — the files hold what the surfaces already show, or something that does not load, which is logged and put on the problems notice here.
    ///
    /// **Nothing to reload can still be something to report.** After a failed load, putting the file back to what is on screen is the fix, and it is no reload at all: the surfaces never stopped showing that content. The notice is the one thing still describing the failure, so it is redrawn from the running config.
    fn reload(&mut self, reload: Reload) -> Option<(Arc<Config>, Fingerprint)> {
        // Before the load, never after: a fingerprint newer than the config it is recorded against names content the shell never applied, and would suppress the reload that applies it.
        let seen = Fingerprint::read(&self.config_path);
        if !self.applied.needs(reload, &seen) {
            if self.failing {
                self.report(None);
            }
            return None;
        }
        match Config::load(&self.config_path) {
            Ok(config) => Some((Arc::new(config), seen)),
            Err(e) => {
                report_config_error(&e);
                self.report(Some(&e));
                None
            }
        }
    }

    /// Records that `config`, loaded from `seen`, is what the shell runs and every surface shows, and brings the problems notice up to date with it. Called once the config has been applied, so the notice speaks the language it set.
    fn applied(&mut self, config: Arc<Config>, seen: Fingerprint) {
        self.live = config;
        self.applied.record(seen);
        self.report(None);
    }

    /// The config for the screens to be planned against when they change: the running one, without reading the files. A monitor arriving or leaving is not a reload — it changes which surfaces exist and nothing the files hold — so it neither loads a file the watcher has not delivered yet, which would draw it on the new screen alone, nor reports again a failure the notice already shows.
    fn live(&self) -> Arc<Config> {
        Arc::clone(&self.live)
    }

    /// Brings the problems notice up to date with the files, the running config standing in for `config.toml` unless `failed` says it did not load — see [`crate::core::check::running`] for why the file and not the running config decides.
    fn report(&mut self, failed: Option<&LoadError>) {
        crate::core::check::announce(&crate::core::check::running(
            &self.live,
            &self.config_path,
            failed,
        ));
        self.failing = failed.is_some();
    }
}

/// A subscription the shell takes out at startup, whatever the config asks for.
///
/// Every one of these is an *exemption* from the shell's standing rule that nothing runs unless something is asking for it, so the set has to be small and has to stay small — and the way it stops staying small is that adding to it costs one line in a function nobody diffs closely.
struct Eager {
    name: &'static str,
    /// Why this one cannot wait to be asked for. Data rather than a comment so that adding an entry means answering the question, and so [`every_startup_subscription_says_why_it_cannot_wait`] can require it.
    reason: &'static str,
    subscribe: Box<dyn FnOnce()>,
}

/// The whole exemption list, in the order it is taken out — which is load-bearing twice over: the command surface comes first so a `shell reload` arriving immediately has a reload hook to call, and the lock performer comes before the logind signals that can ask for a lock.
///
/// Built rather than run so a test can read it without a driver loop under it. Nothing here starts until its `subscribe` is called, and `platform_wayland::watch` starts nothing at all without a loop handle.
fn eager_subscriptions(
    config_path: PathBuf,
    baseline: Fingerprint,
    on_config_change: Rc<dyn Fn(Reload)>,
) -> Vec<Eager> {
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
            reason: "the same request path fed by the desktop portal, so a bound key runs what `hogar-shell …` \
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
                    move |tx| watch_config_changes(config_path, baseline, tx),
                    // Noticed rather than asked for: a write that left the files holding what is already on screen has nothing to deliver.
                    move |_| on_config_change(Reload::IfChanged),
                );
            }),
        },
    ]
}

/// The three answers the layers below cannot reach on their own, handed to them once on the driver thread.
///
/// Each is a case of something low in the stack needing something high in it: the config derives a palette from a wallpaper only the wallpaper *service* can name; a service that runs `[idle]` actions needs the command table, which lives with the socket above it; and the lock service owns *when* the session is locked, never what the covered screen draws. Installed before the first config is applied, since applying one derives a scheme and arms the idle stages.
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

/// Everything a config change affects outside the surfaces themselves: the UI language, the process-wide font, the icon store, and the context that code reached from outside a surface resolves against.
///
/// Called from the driver thread at app level — deliberately not from inside a surface build, since the icon store's download worker must outlive any single surface (see [`shared::icon::init_store`]).
///
/// The problems notice is not part of it: what the notice says depends on the files as well as on the config applied, which only the reload path knows, so [`Reloader`] reports after each config it applies.
fn apply_config(config: &Arc<Config>) {
    services::locale::init(config.language());
    warn_if_font_missing(config.theme.font_family.as_deref());
    platform_wayland::set_surface_fonts(surface_fonts(config));
    config::set_config(Arc::clone(config));
    ui::icon::init_store(&config.icons);
    // After `set_config`: deriving a palette needs to know which wallpaper is up, and that answer comes from the config that was just published. Cheap when the palette is already cached, which is every start after the first; a miss quantises the image on a thread of its own and lands through the scheme watcher below.
    config::scheme::init(config);
    // The surfaces this reload is about to open will carry whatever `init` just resolved, so the watcher must not read the delivery that follows as a change and ask for a second, identical reload.
    config::scheme::mark_painted();
    // After `set_config`, so the stages are armed from the config that was just published rather than the one they were armed from last time.
    services::idle::reconcile();
    // The daemon outlives every reload, so an edited `[notifications]` reaches it this way rather than by restarting it — which would drop the bus name and the history with it.
    services::notifications::set_policy(notification_policy(config));
    // The toast watchers a switched-on event needs. Additive and idempotent: a subscription cannot be undone, so this installs what is missing and leaves the rest — an event switched *off* is silenced by the toaster's own gate rather than by tearing its watcher down.
    modules::toast::watch_events(config);
}

/// The daemon's slice of the config, resolved in one place so startup and reload agree on it. The timeout is the column's — a notification, a toast and an OSD all go after `[stack] timeout_ms` — while what is particular to a notification stays under `[notifications]`.
fn notification_policy(config: &Config) -> services::notifications::Policy {
    services::notifications::Policy {
        timeout: config.stack.lifetime(),
        critical_sticky: config.notifications.critical_sticky,
        critical_max: config.notifications.critical_ceiling(),
        sound: config.notifications.sound.clone(),
    }
}

/// Logs a load that failed, with the whole error — the line and the text around it, which the notice leaves to `config check`.
///
/// The user is told on screen by the problems notice, where the failure is one of the problems ([`Reloader::report`]) and goes when the file loads again. This is the log's record of each attempt: every failed load is a line here, where the notice changes only when what is wrong does.
fn report_config_error(error: &LoadError) {
    tracing::warn!("{error}; the file was not applied");
}

/// The config-watch producer for `watch`: reads the config files every 500 ms and sends a tick when what they hold has changed, so the driver thread reloads — or, finding them holding what it already applied, does nothing.
///
/// **The bytes, read in full on every poll, rather than modification times.** A few kilobytes read and hashed twice a second is microseconds of work, and it is the only check that sees every change. The kernel stamps a write from a clock that moves every few milliseconds on most filesystems and every second or two on some, so two writes close together can share a modification time — and a user's edit landing just behind the shell's own write, and the same length as it, would read as no change at all. Adding the inode does not close that: an editor that writes in place keeps it. A change notification would need this same comparison behind it, since it fires on a `touch` and on the shell's own write too, and would cost a dependency to get there.
///
/// **The first poll compares against the startup load's own read, handed over rather than taken here.** This thread starts once the bars are up, and a baseline it read for itself would already hold an edit landing in between — never a change, so never reloaded. The fingerprint is a path and a hash for each of a handful of files, so moving it across costs nothing worth a second read.
///
/// **A startup that failed to parse is not reloaded until the files change.** Its baseline is the broken file the load already reported; reloading those same bytes would apply nothing and report the same error again. The first edit reloads whatever it holds, because the driver's stamp of what is applied starts empty after a failed load.
fn watch_config_changes(
    path: PathBuf,
    baseline: Fingerprint,
    tx: platform_wayland::EventSender<()>,
) {
    let mut last = baseline;
    while tx.alive() {
        std::thread::sleep(Duration::from_millis(500));
        if noticed(&mut last, Fingerprint::read(&path)) && !tx.send(()) {
            return;
        }
    }
}

/// Whether the files, now holding `now`, have changed in a way worth a reload. Moves `last` on either way, so the moment with no `config.toml` in some editors' saves is not reported itself, and the file that lands after it is.
fn noticed(last: &mut Fingerprint, now: Fingerprint) -> bool {
    if now == *last {
        return false;
    }
    let settled = now.settled();
    *last = now;
    settled
}

/// Logs whether a configured `[theme] font_family` resolves against the installed fonts. A wrong family name (e.g. `"Fira Code Nerd Font"` instead of the installed `"FiraCode Nerd Font"`) otherwise falls back to the default font silently; this turns that into a visible log line.
///
/// The verdict comes from the shaper's own database, so a hit here means the shell will actually render in that family — where the second `fontdb` this used to load could answer differently, and cost a full font scan to do it. Each family's verdict is still remembered, because every save from the settings panel reloads the config and asks again.
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
    /// Everything else in the shell waits to be asked: a service starts when a chip subscribes and stops when the last one goes. These seven do not, so they are the one way an idle shell can start working again — and the way that set grows is one more line in a startup function, which reads as nothing in a diff. Pinning the names is what turns that into a decision someone has to make on purpose.
    ///
    /// Reading the list runs none of it. `subscribe` is never called here, and could not do anything if it were: `platform_wayland::watch` returns without spawning a producer when there is no loop handle under it, which is also why "nothing is running" cannot be asserted from a test at all.
    #[test]
    fn every_startup_subscription_says_why_it_cannot_wait() {
        let config = PathBuf::from("config.toml");
        let baseline = Fingerprint::read(&config);
        let eager = eager_subscriptions(config, baseline, Rc::new(|_| {}));

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
mod reload_baseline_tests {
    use super::*;

    const SAVED: &str = "[clock]\nformat = \"%H:%M\"\n";
    const EDITED: &str = "[clock]\nformat = \"%H:%S\"\n";
    const BROKEN: &str = "[clock\nformat = ";

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("hogar-shell-startup-{name}-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("config.toml")
    }

    fn ticks_within(ticks: &platform_wayland::Subscription<()>, limit: Duration) -> bool {
        let deadline = std::time::Instant::now() + limit;
        while std::time::Instant::now() < deadline {
            if ticks.try_recv().is_some() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    /// **An edit landing between the startup load and the watcher's first poll is reloaded.** The watcher starts after the bars are up; a baseline it read for itself then would already hold the edit, and the edit would never be a change. Driving the real producer is the only way to see where its baseline comes from, so this runs it on a thread of its own against a subscription the test holds.
    #[test]
    fn an_edit_between_the_startup_load_and_the_first_poll_is_reloaded() {
        let path = scratch("race");
        std::fs::write(&path, SAVED).unwrap();
        let startup = load_at_startup(&path);
        std::fs::write(&path, EDITED).unwrap();

        let (tx, ticks) = platform_wayland::detached();
        let watched = path.clone();
        let baseline = startup.baseline.clone();
        std::thread::spawn(move || watch_config_changes(watched, baseline, tx));

        assert!(
            ticks_within(&ticks, Duration::from_secs(3)),
            "the edit is a change against what the startup load read, so the watcher reports it"
        );
        assert!(
            startup
                .applied
                .needs(Reload::IfChanged, &Fingerprint::read(&path)),
            "and the driver, stamped with what startup applied, reloads it"
        );
        drop(ticks);
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    /// **What startup applied is its own read, and only when the load parsed.** A load that parsed has put those bytes on screen, so the watcher finding them unchanged — a `touch`, the settings window's first save undone — is nothing to reload. One that fell back to the starter config put nothing it read on screen, so not even the same bytes count as applied.
    #[test]
    fn startup_applies_its_own_read_only_when_the_load_parsed() {
        let path = scratch("stamp");
        std::fs::write(&path, SAVED).unwrap();
        let parsed = load_at_startup(&path);
        assert!(
            !parsed.applied.needs(Reload::IfChanged, &parsed.baseline),
            "a startup that loaded has applied exactly what it read"
        );

        std::fs::write(&path, BROKEN).unwrap();
        let broken = load_at_startup(&path);
        assert!(
            broken.applied.needs(Reload::IfChanged, &broken.baseline),
            "one that fell back to the starter config applied none of what it read"
        );
        assert_eq!(
            toml::to_string(&broken.config).unwrap(),
            toml::to_string(&Config::starter()).unwrap(),
            "and runs the starter config, as `load_or_default` would"
        );
        assert!(
            matches!(broken.failed, Some(LoadError::Parse(_))),
            "and carries why, for the notice to show once the driver is up"
        );
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    /// **A fresh install's first poll reloads nothing.** The startup load finds no `config.toml` and writes the starter config there; what it applied is those bytes, so the watcher's first look at them is no change, and there is no rebuild and no "config reloaded" toast about a file nobody edited.
    #[test]
    fn a_fresh_install_s_first_poll_reloads_nothing() {
        let path = scratch("fresh");
        let startup = load_at_startup(&path);
        assert!(path.exists(), "the startup load wrote the starter config");

        let mut last = startup.baseline.clone();
        let first_poll = Fingerprint::read(&path);
        assert!(
            !noticed(&mut last, first_poll.clone()),
            "the watcher's first poll finds the starter config the startup load wrote, which is no change"
        );
        assert!(
            !startup.applied.needs(Reload::IfChanged, &first_poll),
            "and the driver has applied exactly those bytes"
        );
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    /// **A startup that failed to parse is not reloaded until the file changes** — the decision, pinned. Reloading the unchanged broken file would apply nothing and report the error the startup load already reported; the edit that fixes it is a change, and with nothing applied it reloads whatever it holds.
    #[test]
    fn a_startup_that_failed_to_parse_reloads_on_the_first_edit_and_not_before() {
        let path = scratch("broken");
        std::fs::write(&path, BROKEN).unwrap();
        let startup = load_at_startup(&path);
        let mut last = startup.baseline.clone();

        assert!(
            !noticed(&mut last, Fingerprint::read(&path)),
            "the broken file has not changed since startup reported it"
        );

        std::fs::write(&path, SAVED).unwrap();
        let fixed = Fingerprint::read(&path);
        assert!(noticed(&mut last, fixed.clone()), "the fix is a change");
        assert!(
            startup.applied.needs(Reload::IfChanged, &fixed),
            "and reloads, because nothing was applied at startup"
        );
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }
}

#[cfg(test)]
mod reloader_tests {
    use super::*;
    use crate::core::check;

    const SAVED: &str = "[clock]\nformat = \"%H:%M\"\n";
    const EDITED: &str = "[clock]\nformat = \"%H:%S\"\n";
    const BROKEN: &str = "[clock\nformat = ";

    /// A config directory holding `SAVED`, with a monitor override naming a module nobody has — a problem in a file the edits below never touch — and the reload path as a startup from it leaves it, with this thread's notice fresh.
    fn started(name: &str) -> (PathBuf, Reloader) {
        telar::set_locale("en");
        check::fresh_notice();
        ui::module::install(crate::core::registry::default_registry(
            &crate::core::popouts::default_popouts(),
        ));
        let dir = std::env::temp_dir().join(format!(
            "hogar-shell-reloader-{name}-{}",
            std::process::id()
        ));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(dir.join("monitors/DP-1")).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(&path, SAVED).unwrap();
        std::fs::write(
            dir.join("monitors/DP-1/config.toml"),
            "[bars.left]\nstart = [\"clokc\"]\n",
        )
        .unwrap();
        let startup = load_at_startup(&path);
        let reloader = Reloader::starting(
            path.clone(),
            Arc::new(startup.config),
            startup.applied,
            startup.failed.as_ref(),
        );
        (path, reloader)
    }

    /// **A parse failure is on the one notice until it is fixed, whichever way the fix arrives, and the other problems stay on it throughout.** A fix to new content is a reload, and reports what it loaded. A fix that puts the file back to what is on screen is no reload at all — the surfaces never stopped showing that content — so the reload path has to take the failure off the notice itself, or the card would go on saying the config was not applied about a config that is running.
    #[test]
    fn a_parse_failure_is_on_the_notice_until_a_fix_whichever_way_the_fix_arrives() {
        let (path, mut reloader) = started("notice");
        let override_problem = check::showing();
        assert_eq!(
            override_problem,
            ["there is no module called 'clokc'"],
            "the override's problem is on the notice from the start"
        );

        std::fs::write(&path, BROKEN).unwrap();
        assert!(
            reloader.reload(Reload::IfChanged).is_none(),
            "a file that does not parse gives the shell nothing to take"
        );
        let failing = check::showing();
        assert!(
            failing.len() == 2 && failing.contains(&override_problem[0]),
            "the failure joins the override's problem on the notice: {failing:?}"
        );

        std::fs::write(&path, SAVED).unwrap();
        assert!(
            reloader.reload(Reload::IfChanged).is_none(),
            "putting the file back is nothing to reload"
        );
        assert_eq!(
            check::showing(),
            override_problem,
            "and still takes the failure off the notice, leaving the override's problem"
        );

        std::fs::write(&path, BROKEN).unwrap();
        reloader.reload(Reload::IfChanged);
        std::fs::write(&path, EDITED).unwrap();
        let (config, seen) = reloader
            .reload(Reload::IfChanged)
            .expect("a fix to new content is a config to take");
        reloader.applied(config, seen);
        assert_eq!(
            check::showing(),
            override_problem,
            "and so does a fix to new content, once it is applied"
        );
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    /// **A monitor arriving reloads nothing and reports nothing.** It changes which surfaces exist and nothing the files hold, so the new screens are planned from the running config as it is. Reloading for it used to load the files again: a broken one was reported again, and an edit the watcher had not delivered yet was drawn on the new screen alone, and taken off the failure notice ahead of the reload that applies it.
    #[test]
    fn a_hotplug_neither_reloads_the_files_nor_reports_them_again() {
        let (path, mut reloader) = started("hotplug");
        std::fs::write(&path, BROKEN).unwrap();
        reloader.reload(Reload::IfChanged);
        let reported = check::showing();
        std::fs::write(&path, EDITED).unwrap();

        let planned = reloader.live();

        assert_eq!(
            planned.clock.format.as_deref(),
            Some("%H:%M"),
            "the new screens are planned from the running config, not from an edit the watcher has not delivered"
        );
        assert_eq!(
            check::showing(),
            reported,
            "and the notice is left exactly as the reload that failed left it"
        );
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }
}

#[cfg(test)]
mod i18n_tests {
    /// The shell's own catalog resolves, and a locale switch changes what it answers. The modules' catalogs are checked in their own crate — a `t!` key is resolved against the catalog of the crate that writes it.
    #[test]
    fn catalog_translates_and_switches() {
        telar::set_locale("en");
        assert_eq!(telar::t!("config.error_title"), "Configuration not applied");
        telar::set_locale("es");
        assert_eq!(telar::t!("config.error_title"), "Configuración no aplicada");
    }
}
