telar::rsx_modules!();

// What the `hogar-shell` binary reaches for; everything else now belongs to the crate that owns it.
pub use crate::core::commands::describe as ipc_describe;
pub use crate::core::commands::dispatch_locally;
pub use crate::core::ipc::call as ipc_call;
pub use crate::core::man::FORMS as USAGE_FORMS;

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use config::fingerprint::{Fingerprint, Reload, Stamp};
use config::{Config, LoadError};
use platform_wayland::LayerShellPlatform;
use telar::{App, AppPathsProvider, run_multi_with_platform};

use layout::{ActiveWorkspace, LayoutStore};
use services::events::{Edge, ShellEvent};
use surfaces::layer_window::Content;
use surfaces::reconcile::Shell;
use util::report::Report;

/// How far into the user's machine a mode of the binary reaches. Until a mode opens its reach, every file resolves under a scratch root and every bus, daemon and compositor probe answers as if absent, which is what a test and a preview get.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reach {
    /// The user's files: the config a check reads, the socket a command is sent over.
    Files,
    /// The files, the live session's buses and daemons, and the compositor.
    Machine,
}

impl Reach {
    /// What the mode the binary's first argument names may reach: the shell and the dependency check reach the whole machine, every other command the user's files alone.
    pub fn of(command: Option<&str>) -> Self {
        match command {
            None | Some("run" | "deps") => Self::Machine,
            Some(_) => Self::Files,
        }
    }

    /// Opens what this reach allows. The only place any of the three gates is opened.
    pub fn open(self) {
        util::paths::install();
        if self == Self::Machine {
            util::live::install();
            platform_wayland::allow_compositor_probes();
        }
    }
}

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

/// The family every surface this shell opens starts in, before its own tree sets it from the config it is drawn with and follows that config's reloads and previews from then on.
fn surface_fonts(config: &Config) -> telar::AppConfig {
    telar::AppConfig {
        font_family: config.theme.font_family.clone(),
        ..telar::AppConfig::default()
    }
}

/// Builds the ambient world a `[preview]` needs; deliberately not `apply_config`, which also arms idle stages, notifications and toast watchers that have no business running just to render a component.
fn seed_preview_world() {
    let config = Arc::new(Config::load_or_default(&Config::default_path()));
    services::locale::init(config.language());
    platform_wayland::set_surface_fonts(surface_fonts(&config));
    ui::icon::init_store(&config.icons);
    telar::set_theme(config.resolve_theme());
    config::set_config(config);
    install_hooks();
}

/// What `hogar-shell config schema [name]` prints: the annotated defaults of one config section, of all of them, or — for the one name that is not a config section — of a layout file.
///
/// `layout` sits here rather than under a verb of its own because it is the same question about the other file the user edits, and because `config schema > config.toml` has a counterpart that has to be as easy to find: `config schema layout > layouts/mine.toml`.
pub fn config_schema(section: Option<&str>) -> Result<String, String> {
    match section {
        Some("layout") => layout::schema::render(),
        named => config::schema::render(named),
    }
}

/// Which layouts a run starts from.
///
/// `--safe-layout` is for a session the user's own layout has made unusable: the store holds the built-in layout alone, refuses every edit and writes nothing, so the files it was started to rescue are exactly as they were when the shell is restarted without the flag (TA-7).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layouts {
    /// The ones in `~/.config/hogar-shell/layouts/`, drawing whichever this installation chose.
    OnDisk,
    /// The built-in one, and nothing else.
    BuiltInOnly,
}

pub fn run(layouts: Layouts) {
    // `cargo telar preview`/`test` has to answer while a shell is already up, so this precedes the single-instance check below — it opens no surface and takes no bus name.
    if telar::dev_entry(preview_entries, preview_window(), seed_preview_world) {
        return;
    }
    Reach::of(None).open();
    // One shell per compositor instance: a second one would fight over the notification bus name and the IPC socket, and the user would see two of every bar. Checked before anything is opened so the failure is a clean message rather than a half-started shell.
    if !crate::core::ipc::claim_instance() {
        eprintln!(
            "hogar-shell: already running for this compositor (IPC socket {}). Use `hogar-shell shell quit` to stop it.",
            crate::core::ipc::socket_path().display()
        );
        std::process::exit(1);
    }
    let desktop_motion = services::motion::read_in_background();
    let config_path = Config::default_path();
    // Once, and here rather than on the driver thread: the locale, the fonts and the notification daemon start from it before the driver exists, and a second load for the surfaces could read a different file from the one they started with.
    let startup = load_at_startup(&config_path);
    services::locale::init(startup.config.language());
    platform_wayland::set_surface_fonts(surface_fonts(&startup.config));
    // Once, before the driver, so it keeps owning the D-Bus name across every reload; `apply_config` hands it an edited policy instead.
    services::notifications::init(notification_policy(&startup.config));

    // Non-destructive reload: one persistent driver. Every surface is opened dynamically on the driver thread (via `setup_shell`, deferred with `run_on_start`) and reconciled on config change, so a reload never tears down the connection, the popup, or the shared services — only the surfaces that changed.
    platform_wayland::run_on_start(move || {
        setup_shell(config_path, startup, layouts, desktop_motion)
    });
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
fn setup_shell(
    config_path: PathBuf,
    startup: Startup,
    layouts: Layouts,
    desktop_motion: Option<std::thread::JoinHandle<bool>>,
) {
    install_hooks();
    let Startup {
        config,
        baseline,
        applied,
        failed,
    } = startup;
    let config = Arc::new(config);
    // Before the first build, so the surfaces start with the desktop's motion and the portal's first delivery, the same answer, changes nothing — at login above all, where it would otherwise flip from the default and draw everything twice.
    config::motion::set_desktop_reduced(
        desktop_motion
            .and_then(|read| read.join().ok())
            .unwrap_or(false),
    );
    apply_config(&config);
    let reloader = Rc::new(RefCell::new(Reloader::starting(
        config_path.clone(),
        Arc::clone(&config),
        applied,
        failed.as_ref(),
    )));
    platform_wayland::app_watch(automation::failures::subscribe, {
        let reloader = Rc::clone(&reloader);
        move |failures: Report| reloader.borrow_mut().failures_changed(failures)
    });
    if platform_wayland::outputs().is_empty() {
        eprintln!("hogar-shell: no Wayland outputs found (is a compositor running?)");
        std::process::exit(1);
    }
    // As early as a lock can be taken — driver globals bound, opener installed, config published — because until then a session this shell died holding shows the compositor's fallback screen instead of a prompt.
    services::lock::restore();

    // The one column of cards — notification popups, toasts, the OSD. Long-lived: set up once, it persists across reloads, and it holds no surface at all until one of the three has something to say. The toast watchers are installed by `apply_config`, which has already run, so an event switched on later gets its watcher on the next reload.
    modules::stack::host();

    // Where everything the shell draws is written down. Owned here, because the layer windows read it on every pass and the `layout` IPC target writes it; a broken layout file is reported and skipped, and the built-in one is always there to fall back to.
    let (mut store, mut problems) = match layouts {
        Layouts::OnDisk => LayoutStore::load(surfaces::layouts::dir()),
        Layouts::BuiltInOnly => (
            LayoutStore::safe(surfaces::layouts::dir()),
            Report::default(),
        ),
    };
    if layouts == Layouts::OnDisk {
        store.set_trust(surfaces::bundles::trust_of(
            &services::state::get(),
            crate::core::commands::runs_unasked,
        ));
        surfaces::layouts::select_active(&mut store, &mut problems);
    }
    let store = Rc::new(RefCell::new(store));
    surfaces::layouts::report_problems(&problems);

    // One pass brings the screen in line with a layout — at startup, at every reload that applies one, and when the screens change — so there is one description of what should be on screen rather than an opening path and a reloading path that can disagree. It reports what it did itself, through tracing rather than `println!`, because this runs on the driver thread where a direct write to a pipe nobody is draining blocks forever. See `init_tracing`.
    let shell = Rc::new(RefCell::new(Shell::new()));
    surfaces::area::set_stack_builder(modules::stack::area);
    surfaces::transient::install(shell.borrow().windows().holder());
    editor::install();
    // Which screens are under a fullscreen window is read from the management protocol's own list, which only fills while something watches it.
    platform_wayland::watch_managed_toplevels(&platform_wayland::Interest::new(), |_| {});
    surfaces::transient::set_hidden_layers(hidden_under_fullscreen);
    let apply = {
        let shell = Rc::clone(&shell);
        let store = Rc::clone(&store);
        let config_path = config_path.clone();
        move |config: &Arc<Config>, content: Content| {
            let mut store = store.borrow_mut();
            let (desktops, mut report) = surfaces::reconcile::plan(
                &config_path,
                config,
                store.active(),
                store.all(),
                &platform_wayland::outputs(),
                &active_workspace,
            );
            let (sources, sourcing) =
                automation::sources::of_layout(store.active(), store.all(), &config.automation);
            automation::sources::declare(sources);
            report.merge(sourcing);
            // The pass that proves a layout resolves is the one that keeps the copy to fall back to (TA-7). Nothing is written unless the copy would differ, since this runs on every reload and every monitor change.
            if report.is_clean() {
                let active = store.active_id().clone();
                if let Err(why) = store.keep_last_good(&active) {
                    tracing::warn!("could not keep a copy of `{active}` that works: {why}");
                }
            }
            // Said with the rest but merged after the copy is kept: a cost the user chose, or what waits for their trust, is no reason to stop keeping the copy that works.
            report.merge(layout::held(store.active(), store.all()));
            let file = format!("layouts/{}.toml", store.active_id());
            for desktop in &desktops {
                report.merge(crate::core::commands::layout::scanout(
                    &desktop.resolved,
                    &file,
                ));
            }
            surfaces::layouts::report_problems(&report);
            shell.borrow_mut().reconcile(&desktops, content);
            modules::stack::reconcile_config();
        }
    };

    apply(&config, Content::Rebuild);

    let apply = Rc::new(apply);

    // What the `layout` verbs and the lock session opener act on, and the pass an edit is redrawn with. Installed once that pass exists: an edit nothing redraws is one the user has no way to judge. An edit changes the arrangement and not the config, so only the areas it touched are built again.
    surfaces::layouts::install(Rc::clone(&store), {
        let apply = Rc::clone(&apply);
        let reloader = Rc::clone(&reloader);
        Rc::new(move || apply(&reloader.borrow().live(), Content::Changed))
    });
    surfaces::trust_dialog::install();

    // The config having changed, whoever noticed: the file watcher, `hogar-shell shell reload`, a keybind. The toast belongs here rather than in the surface pass, which also runs at startup — a toast saying the config was reloaded is only true of a reload, and only of one that applied something.
    let on_config_change: Rc<dyn Fn(Reload)> = {
        let reloader = Rc::clone(&reloader);
        let apply = Rc::clone(&apply);
        Rc::new(move |reload| {
            let Some(Loaded {
                config,
                seen,
                announced,
            }) = reloader.borrow_mut().reload(reload)
            else {
                return;
            };
            apply_config(&config);
            // A layout edit reaches the shell the same way a config edit does — the watcher fingerprints the layout files too — and `layout use` is a write to machine state followed by a reload, so the store is read again, the active name with it, before the pass that draws from it.
            surfaces::layouts::reload();
            apply(&config, Content::Rebuild);
            // What the user opened and the column of cards are not the surface pass's to rebuild, so they take the new config here, in the same pass.
            surfaces::transient::rebuild_all(&seen, reload);
            reloader.borrow_mut().applied(config, seen);
            if announced {
                modules::toast::config_reloaded();
            }
        })
    };

    // What changes how the running config looks without changing the config — the desktop's reduced-motion setting, a palette derived from a wallpaper — is drawn again from it, as a monitor arriving is: no file is read, no toast says the config was reloaded, and an open transient being typed into keeps its tree.
    config::set_restyle_hook({
        let reloader = Rc::clone(&reloader);
        let apply = Rc::clone(&apply);
        move || {
            let config = reloader.borrow().live();
            apply_look(&config);
            apply(&config, Content::Rebuild);
            surfaces::transient::restyle_all();
        }
    });

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
    // A monitor arriving or leaving changes which windows exist and nothing about what they draw, so the screens that were already there keep the trees they have.
    platform_wayland::on_outputs_changed(move || {
        let config = reloader.borrow().live();
        apply(&config, Content::Changed);
    });
    services::events::emit(ShellEvent::Started);
}

/// Whether `layer` is out of sight on `output`: the top layer is, under a fullscreen window (F-6.2), so what hangs off a bar there opens over it instead.
fn hidden_under_fullscreen(output: Option<&str>, layer: layout::LayerKind) -> bool {
    layer == layout::LayerKind::Top
        && platform_wayland::current_managed_toplevels()
            .iter()
            .any(|window| {
                window.fullscreen
                    && output.is_none_or(|output| window.outputs.iter().any(|on| on == output))
            })
}

/// What the compositor says is on `output` right now, as much of it as a workspace rule can match on.
///
/// The name is what `ext-workspace-v1` gives portably; `id:` and `special:` are Hyprland's alone and come through its own socket, so a rule using one on another compositor is reported as inactive rather than quietly never firing (DEC-16).
fn active_workspace(output: Option<&str>) -> Option<ActiveWorkspace> {
    let on_this_screen = |workspace: &platform_wayland::Workspace| {
        workspace.active
            && output.is_none_or(|name| workspace.outputs.iter().any(|out| out == name))
    };
    let named = platform_wayland::current_workspaces()
        .into_iter()
        .find(on_this_screen)?;
    let hyprland = services::hyprland::current_workspaces().and_then(|snapshot| {
        snapshot
            .workspaces
            .into_iter()
            .find(|workspace| workspace.name == named.name)
    });
    Some(ActiveWorkspace {
        name: named.name,
        id: hyprland.as_ref().map(|workspace| workspace.id as i64),
        special: hyprland.as_ref().map(|workspace| workspace.is_special()),
    })
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

/// A config the files hold for the shell to take.
struct Loaded {
    config: Arc<Config>,
    /// The content it was loaded from.
    seen: Fingerprint,
    /// Whether the user is told the config was reloaded: for an edit made outside the shell and for a reload somebody asked for, and not for the shell's own write coming back, which is a change the user just made through the shell and is watching apply.
    announced: bool,
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
    /// What the files said when the notice was last brought up to date with them.
    files: Report,
    /// What is failing as the shell runs, as [`automation::failures`] last published it.
    failures: Report,
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
            files: Report::default(),
            failures: Report::default(),
        };
        reloader.report(failed);
        reloader
    }

    /// Reads the files and answers whether they hold a config for the shell to take: the config, the content it was loaded from — for the caller to apply and then hand to [`applied`](Self::applied) — and whether to say so. `None` when there is nothing to take — the files hold what the surfaces already show, or something that does not load, which is logged and put on the problems notice here.
    ///
    /// **Nothing to reload can still be something to report.** After a failed load, putting the file back to what is on screen is the fix, and it is no reload at all: the surfaces never stopped showing that content. The notice is the one thing still describing the failure, so it is redrawn from the running config.
    fn reload(&mut self, reload: Reload) -> Option<Loaded> {
        // Before the load, never after: a fingerprint newer than the config it is recorded against names content the shell never applied, and would suppress the reload that applies it.
        let seen = Fingerprint::read(&self.config_path);
        if !self.applied.needs(reload, &seen) {
            if self.failing {
                self.report(None);
            }
            return None;
        }
        let own_write = config::fingerprint::written_by_shell(&seen);
        match Config::load(&self.config_path) {
            Ok(config) => Some(Loaded {
                config: Arc::new(config),
                seen,
                announced: reload == Reload::Always || !own_write,
            }),
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

    /// Brings the problems notice up to date with what is failing as the shell runs, without reading the files again. A config that did not load is what the notice is about until it does, so that notice is left as it is.
    fn failures_changed(&mut self, failures: Report) {
        self.failures = failures;
        if !self.failing {
            self.announce();
        }
    }

    /// The config for the screens to be planned against when they change: the running one, without reading the files. A monitor arriving or leaving is not a reload — it changes which surfaces exist and nothing the files hold — so it neither loads a file the watcher has not delivered yet, which would draw it on the new screen alone, nor reports again a failure the notice already shows.
    fn live(&self) -> Arc<Config> {
        Arc::clone(&self.live)
    }

    /// Brings the problems notice up to date with the files and, while they load, with what is failing as the shell runs; the running config stands in for `config.toml` unless `failed` says it did not load — see [`crate::core::check::running`] for why the file and not the running config decides.
    fn report(&mut self, failed: Option<&LoadError>) {
        self.files = crate::core::check::running(&self.live, &self.config_path, failed);
        self.failing = failed.is_some();
        self.announce();
    }

    fn announce(&self) {
        let mut report = self.files.clone();
        if !self.failing {
            report.merge(self.failures.clone());
        }
        crate::core::check::announce(&report);
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
            name: "motion",
            reason: "the desktop's reduced-motion setting reshapes every surface's transitions, so it \
                     rebuilds them all rather than belonging to any one; silently absent with no portal",
            subscribe: Box::new(|| {
                platform_wayland::watch(services::motion::subscribe, services::motion::on_change);
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

/// The answers the layers below cannot reach on their own, handed to them once on the driver thread.
///
/// Each is a case of something low in the stack needing something high in it: the config derives a palette from a wallpaper only the wallpaper *service* can name, and announces a palette landing on an event stream only the services own; a service that runs `[idle]` actions needs the command table, which lives with the socket above it; and the lock service owns *when* the session is locked, never what the covered screen draws. Installed before the first config is applied, since applying one derives a scheme and arms the idle stages.
fn install_hooks() {
    config::set_wallpaper_source(|config| {
        let focused = surfaces::transient::focused_output();
        services::wallpaper::current_image(config, focused.as_deref())
    });
    config::scheme::set_landed_hook(|| {
        if let Some(config) = config::shared_config() {
            palette_landed(
                &PALETTE,
                config.resolve_theme().colors(),
                announce_colors_changed,
            );
        }
    });
    services::command::set_runner(
        crate::core::commands::dispatch,
        crate::core::commands::resolves,
    );
    services::lock::set_session_opener(|screen| {
        let config = config::config();
        // Resolved once, here, as the lock is taken: every surface of this session — the ones created now and any monitor plugged in while the screen is covered — builds from this one snapshot, so a layout edit made while locked applies at the next lock rather than under a user who is typing a password (TA-8).
        //
        // Not resolved at all for a lock that asked for the minimal screen, which is a lock taken back after a crash: nothing would read the answer, and working one out would report a fallback to the user for a screen they were never going to get.
        let lock = match screen {
            services::lock::Screen::Minimal => None,
            services::lock::Screen::Configured => lock_layer().map(Arc::new),
        };
        platform_wayland::lock_session(move |output| modules::lock::LockApp {
            config: config.clone(),
            output,
            screen,
            lock: lock.clone(),
        })
    });
    ui::module::set_panel_opener(surfaces::panel::open_panel);
    ui::descriptor::install(crate::core::modules::MODULES);
}

/// The lock layer the session is about to be covered with, or `None` for the minimal lock.
///
/// Three things have to hold before a lock screen is drawn from a layout, and any one of them failing is the minimal lock rather than a half-drawn one: the store has to be there, the layout has to validate — every instance a reading, no action bound, a prompt that cannot be hidden or covered — and it has to resolve for every screen with a prompt on it. Checked here, once, because after `take()` the compositor is already showing whatever this returns and a message is no use to whoever is standing at it: the reason is held and said as a toast once the session is unlocked (`services::lock::fell_back`).
fn lock_layer() -> Option<modules::lock::LockLayout> {
    let Some(built) = surfaces::layouts::read(|store| {
        let outputs = platform_wayland::outputs();
        let names: Vec<Option<&str>> = outputs.iter().map(|out| out.name.as_deref()).collect();
        modules::lock::LockLayout::checked(
            store.active(),
            store.all(),
            &crate::core::commands::layout::catalogue(),
            &crate::core::commands::layout::lock_theme(),
            &names,
            &store.path_of(store.active_id()).display().to_string(),
        )
        .map_err(|report| report.summary())
    }) else {
        services::lock::fell_back("there is no layout store to draw the lock screen from");
        return None;
    };
    match built {
        Ok(lock) => Some(lock),
        Err(why) => {
            services::lock::fell_back(why);
            None
        }
    }
}

/// Everything a config change affects outside the surfaces themselves: the UI language, the family a surface opens in, the icon store, and the context that code reached from outside a surface resolves against.
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
    apply_look(config);
    // After `set_config`, so the stages are armed from the config that was just published rather than the one they were armed from last time.
    services::idle::reconcile();
    // The daemon outlives every reload, so an edited `[notifications]` reaches it this way rather than by restarting it — which would drop the bus name and the history with it.
    services::notifications::set_policy(notification_policy(config));
    // The toast watchers a switched-on event needs. Additive and idempotent: a subscription cannot be undone, so this installs what is missing and leaves the rest — an event switched *off* is silenced by the toaster's own gate rather than by tearing its watcher down.
    modules::toast::watch_events(config);
    automation::rules::install(
        config.rules.clone(),
        crate::core::check::rules_environment(&config.lock),
        config.automation,
    );
}

/// Announces what the palette the surfaces are about to carry changed: on every config applied, and on every restyle, which is how a derived palette reaches them.
fn apply_look(config: &Config) {
    announce_theme_mode(config);
    palette_applied(
        &PALETTE,
        config.resolve_theme().colors(),
        config::scheme::derives_palette(config),
        announce_colors_changed,
    );
}

/// The colours the shell last painted with, as far as `colors_changed` has been told.
static PALETTE: Edge<Vec<telar::Color>> = Edge::new();

/// Announces the shell's palette changing, whatever changed it: another built-in theme, a `[theme]` edit, or a derived palette landing.
///
/// A config that derives its palette from a wallpaper is not judged here: its colours are announced by [`palette_landed`] once the export files are on disk, which is after this reload, so announcing here would fire before whatever reads those files could see them. It only records where the palette stands, so a reload that changes nothing is not a change when the export lands. Every other way the palette moves reaches the shell through [`apply_look`], so this is the one place that sees them.
fn palette_applied(
    seen: &Edge<Vec<telar::Color>>,
    palette: Vec<telar::Color>,
    announced_on_landing: bool,
    announce: impl FnOnce(),
) {
    if announced_on_landing {
        seen.seed(palette);
    } else if seen.observe(palette).is_some() {
        announce();
    }
}

/// Announces a derived palette once its export files are on disk, or its fallback once the derivation has failed, unless the shell already paints with it.
fn palette_landed(
    seen: &Edge<Vec<telar::Color>>,
    palette: Vec<telar::Color>,
    announce: impl FnOnce(),
) {
    if seen.observe(palette).is_some() {
        announce();
    }
}

fn announce_colors_changed() {
    services::events::emit(ShellEvent::ColorsChanged);
}

/// Whether the palette the shell last applied was dark or light.
static THEME_MODE: Edge<config::scheme::Mode> = Edge::new();

/// Announces the shell's palette switching between dark and light.
///
/// Judged from the palette the surfaces are about to carry rather than from `[theme] mode`, which `auto` leaves to the palette: going from a dark theme to a light one is a mode change whatever the key says. Every way the palette moves — an edited `[theme]`, `scheme mode`, a derived palette landing — reaches the shell through [`apply_look`], on a reload or a restyle, so this is the one place that sees them all.
fn announce_theme_mode(config: &Config) {
    let painted = config::scheme::Mode::of(&config.resolve_theme());
    if let Some(mode) = THEME_MODE.observe(painted) {
        services::events::emit(ShellEvent::ThemeModeChanged { mode });
    }
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
                "motion",
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
        let dir = util::paths::isolated_root()
            .expect("a test process resolves under its scratch root")
            .join(format!("startup-{name}"));
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
        ui::descriptor::install(crate::core::modules::MODULES);
        let dir = util::paths::isolated_root()
            .expect("a test process resolves under its scratch root")
            .join(format!("reloader-{name}"));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(dir.join("monitors/DP-1")).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(&path, SAVED).unwrap();
        std::fs::write(
            dir.join("monitors/DP-1/config.toml"),
            "[general]\nlanguage = \"es\"\n",
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
            [
                "[general] can only be set in config.toml, so a monitor override's copy of it is ignored"
            ],
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
        let loaded = reloader
            .reload(Reload::IfChanged)
            .expect("a fix to new content is a config to take");
        reloader.applied(loaded.config, loaded.seen);
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

    fn clock(format: &str) -> toml::Table {
        toml::Table::from_iter([("format".to_string(), toml::Value::from(format))])
    }

    /// **The shell's own save applies without a toast, and an edit from outside it is announced.** A pin dragged on the dock, a theme picked in a popover, a field saved in settings: the user is watching that change apply, and "config reloaded" about it is noise. A hand edit is something the shell only noticed, and saying so is how the user knows it took.
    #[test]
    fn the_shells_own_write_applies_quietly_and_an_edit_from_outside_is_announced() {
        let (path, mut reloader) = started("own-write");

        Config::save_section(&path, "clock", &clock("%H:%S")).expect("the shell saves");
        let own = reloader
            .reload(Reload::IfChanged)
            .expect("the shell's own write is a config to take");
        assert!(!own.announced, "the shell's own write comes back quietly");
        assert_eq!(
            own.config.clock.format.as_deref(),
            Some("%H:%S"),
            "and is applied all the same"
        );
        reloader.applied(own.config, own.seen);

        std::fs::write(&path, SAVED).unwrap();
        let edited = reloader
            .reload(Reload::IfChanged)
            .expect("an edit from outside is a config to take");
        assert!(
            edited.announced,
            "an edit from outside the shell is announced"
        );
        reloader.applied(edited.config, edited.seen);

        Config::save_section(&path, "clock", &clock("%M")).expect("the shell saves");
        std::fs::write(&path, EDITED).unwrap();
        let behind = reloader
            .reload(Reload::IfChanged)
            .expect("the edit behind the save is a config to take");
        assert!(
            behind.announced,
            "an edit landing behind the shell's own write is the user's, and is announced"
        );
        reloader.applied(behind.config, behind.seen);

        Config::save_section(&path, "clock", &clock("%S")).expect("the shell saves");
        assert!(
            reloader
                .reload(Reload::Always)
                .expect("a reload somebody asked for always takes the files")
                .announced,
            "a reload somebody asked for is announced, whoever wrote the files"
        );
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    /// What the registry publishes for `file` alone, since other tests in this process fail sites of their own.
    fn published(file: &str) -> Report {
        let mut report = Report::default();
        for finding in automation::failures::report().errors {
            if finding.file == Path::new(file) {
                report.error(finding);
            }
        }
        report
    }

    /// **What fails as the shell runs is on the notice as it happens, and leaves it as it recovers, with no reload either way.** The shell hands each report the registry publishes to [`Reloader::failures_changed`]; while a config that did not load is on the notice, it waits for the next report made without one.
    #[test]
    fn a_failure_while_running_reaches_the_notice_without_a_reload_and_leaves_as_it_recovers() {
        use automation::failures::{self, Drawn, Site};

        let (path, mut reloader) = started("running");
        let at_start = check::showing();
        let file = "layouts/reloader-running.toml";
        let site = Site::Expression(Drawn {
            file: file.to_string(),
            key: "outputs.*.layers.top.areas.bar.visible".to_string(),
            output: Some("DP-1".to_string()),
            copy: None,
        });

        failures::fail(
            site.clone(),
            util::report::Message::verbatim("`$weather.temp` is not a number"),
        );
        reloader.failures_changed(published(file));
        assert!(
            check::showing().contains(&"`$weather.temp` is not a number".to_string()),
            "{:?}",
            check::showing()
        );

        std::fs::write(&path, BROKEN).unwrap();
        reloader.reload(Reload::IfChanged);
        failures::recover(&site);
        reloader.failures_changed(published(file));
        std::fs::write(&path, SAVED).unwrap();
        reloader.reload(Reload::IfChanged);
        assert_eq!(
            check::showing(),
            at_start,
            "a recovery heard while the config did not load is on the notice once it loads"
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
        assert_eq!(telar::t!("notice.error_title"), "Configuration not applied");
        telar::set_locale("es");
        assert_eq!(telar::t!("notice.error_title"), "Configuración no aplicada");
    }
}

#[cfg(test)]
mod lock_layer_tests {
    use super::*;

    /// Held for the length of every test below.
    ///
    /// The reason a lock fell back is one per process — it has to be, since the toast that says it is raised from wherever the lock ends — so two of these running at once would each take the other's. Observed rather than reasoned about: the pair failed together, one for a reason it had not recorded and one for a reason it had not expected.
    static ONE_AT_A_TIME: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn alone() -> std::sync::MutexGuard<'static, ()> {
        ONE_AT_A_TIME
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// A store holding one layout whose lock layer is `lock`, installed as the one the shell owns.
    fn shell_locked_with(name: &str, lock: &str) -> Arc<Config> {
        ui::descriptor::install(crate::core::modules::MODULES);
        let dir = util::paths::isolated_root()
            .expect("a test process resolves under its scratch root")
            .join(format!("lock-layer-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a layouts directory");
        std::fs::write(dir.join("mine.toml"), lock).expect("a layout to lock with");

        let (mut store, report) = LayoutStore::load(&dir);
        assert!(report.is_clean(), "{}", report.render());
        store
            .use_layout(&layout::LayoutId::new("mine"))
            .expect("the store holds it");
        surfaces::layouts::install(Rc::new(RefCell::new(store)), Rc::new(|| {}));
        let config = Arc::new(Config::starter());
        config::set_config(Arc::clone(&config));
        config
    }

    const PROMPT: &str = r#"
        id = "mine"
        [[outputs]]
        match = "*"
        [[outputs.layers.lock.areas]]
        id = "prompt"
        kind = "prompt"
    "#;

    /// **Every way a lock layer can be wrong is the minimal lock**, decided before the compositor is asked for the lock rather than discovered by a screen that is already covering it.
    ///
    /// Each of these is a layout that parses and resolves: what is wrong with it is a rule about the lock layer, and the rules are load-bearing — a control on a screen anyone can touch, a gesture that runs a command from behind a password prompt, a prompt an expression could hide, a layer with no prompt at all. A half-corrected lock screen is worse than a plain one (TA-8), so none of them is patched up.
    #[test]
    fn a_lock_layer_that_breaks_a_rule_is_refused_and_says_why() {
        let _alone = alone();
        let faults = [
            (
                "no-prompt",
                "[[outputs.layers.lock.areas]]\nid = \"grid\"\nkind = \"grid\"",
            ),
            (
                "a-control",
                "[[outputs.layers.lock.areas]]\nid = \"prompt\"\nkind = \"prompt\"\n\
                 [[outputs.layers.lock.areas]]\nid = \"grid\"\nkind = \"grid\"\n\
                 [[outputs.layers.lock.areas.groups]]\nid = \"cell\"\nplace = \"cell\"\ncol = 0\nrow = 0\n\
                 [[outputs.layers.lock.areas.groups.children]]\nid = \"t\"\nmodule = \"tray\"",
            ),
            (
                "an-action",
                "[[outputs.layers.lock.areas]]\nid = \"prompt\"\nkind = \"prompt\"\n\
                 [[outputs.layers.lock.areas]]\nid = \"grid\"\nkind = \"grid\"\n\
                 [[outputs.layers.lock.areas.groups]]\nid = \"cell\"\nplace = \"cell\"\ncol = 0\nrow = 0\n\
                 [[outputs.layers.lock.areas.groups.children]]\nid = \"c\"\nmodule = \"clock\"\nrepresentation = \"widget_m\"\n\
                 [outputs.layers.lock.areas.groups.children.actions]\npress = [\"lock off\"]",
            ),
            (
                "a-hideable-prompt",
                "[[outputs.layers.lock.areas]]\nid = \"prompt\"\nkind = \"prompt\"\nvisible = \"gaming\"",
            ),
        ];

        for (name, lock) in faults {
            shell_locked_with(
                name,
                &format!("id = \"mine\"\n[[outputs]]\nmatch = \"*\"\n{lock}\n"),
            );
            let _ = services::lock::take_fallback();
            assert!(
                lock_layer().is_none(),
                "`{name}` should not be drawn on a locked screen"
            );
            assert!(
                services::lock::take_fallback().is_some(),
                "`{name}` was refused without saying why, so nothing would be said after unlocking"
            );
        }
    }

    /// A mistake somewhere else in the layout is not the lock layer's problem: a bar naming a module this build does not have costs the user that chip, not the lock screen they configured.
    #[test]
    fn a_broken_bar_does_not_cost_the_lock_screen() {
        let _alone = alone();
        shell_locked_with(
            "broken-bar",
            &format!(
                "{PROMPT}\n[[outputs.layers.top.areas]]\nid = \"bar\"\nkind = \"bar\"\nedge = \"top\"\nthickness = 32\n\
                 [[outputs.layers.top.areas.groups]]\nid = \"start\"\nplace = \"zone\"\nzone = \"start\"\n\
                 [[outputs.layers.top.areas.groups.children]]\nid = \"typo\"\nmodule = \"clokc\"\n"
            ),
        );
        let _ = services::lock::take_fallback();
        assert!(lock_layer().is_some());
        assert!(services::lock::take_fallback().is_none());
    }

    /// And a layer that breaks none of them is drawn, or the check above would pass by refusing everything.
    #[test]
    fn a_lock_layer_that_breaks_no_rule_is_what_the_session_is_covered_with() {
        let _alone = alone();
        let config = shell_locked_with("good", PROMPT);
        let _ = services::lock::take_fallback();
        let lock = lock_layer().expect("a prompt alone is a lock layer");
        assert!(
            services::lock::take_fallback().is_none(),
            "and nothing to say about it afterwards"
        );

        telar::reset_layout_runtime();
        telar::set_locale("en");
        let app = modules::lock::LockApp {
            config: Some(config),
            output: None,
            screen: services::lock::Screen::Configured,
            lock: Some(Arc::new(lock)),
        };
        // Built rather than only resolved: what the acceptance asks is that the session ends up covered by something with a field in it.
        let _ = app.root();
    }
}

#[cfg(test)]
mod colors_changed_tests {
    use std::cell::Cell;

    use telar::Color;

    use super::*;

    fn palette(shade: u8) -> Vec<Color> {
        vec![Color::from_rgb_u8(shade, 40, 40); 3]
    }

    struct Heard(Cell<usize>);

    impl Heard {
        fn applied(&self, seen: &Edge<Vec<Color>>, shade: u8, derived: bool) {
            palette_applied(seen, palette(shade), derived, || self.bump());
        }

        fn landed(&self, seen: &Edge<Vec<Color>>, shade: u8) {
            palette_landed(seen, palette(shade), || self.bump());
        }

        fn bump(&self) {
            self.0.set(self.0.get() + 1);
        }

        fn count(&self) -> usize {
            self.0.get()
        }
    }

    fn heard() -> Heard {
        Heard(Cell::new(0))
    }

    #[test]
    fn switching_to_another_built_in_theme_announces_once() {
        let (seen, heard) = (Edge::new(), heard());
        heard.applied(&seen, 10, false);
        assert_eq!(
            heard.count(),
            0,
            "the palette the shell starts with is not news"
        );
        heard.applied(&seen, 200, false);
        assert_eq!(heard.count(), 1);
        heard.applied(&seen, 200, false);
        assert_eq!(heard.count(), 1);
    }

    #[test]
    fn a_reload_that_leaves_the_colours_alone_announces_nothing() {
        let (seen, heard) = (Edge::new(), heard());
        heard.applied(&seen, 10, false);
        heard.applied(&seen, 10, false);
        heard.applied(&seen, 10, false);
        assert_eq!(heard.count(), 0);
    }

    #[test]
    fn a_derived_palette_is_announced_once_when_it_lands() {
        let (seen, heard) = (Edge::new(), heard());
        heard.applied(&seen, 10, true);
        assert_eq!(
            heard.count(),
            0,
            "applying it says nothing before its files are written"
        );
        heard.landed(&seen, 90);
        assert_eq!(heard.count(), 1);
    }

    #[test]
    fn the_reload_a_landed_palette_causes_does_not_announce_it_again() {
        let (seen, heard) = (Edge::new(), heard());
        heard.applied(&seen, 10, true);
        heard.landed(&seen, 90);
        heard.applied(&seen, 90, true);
        heard.landed(&seen, 90);
        assert_eq!(heard.count(), 1);
    }

    #[test]
    fn a_derived_palette_seen_before_is_announced_again_after_the_shell_left_it() {
        let (seen, heard) = (Edge::new(), heard());
        heard.applied(&seen, 10, true);
        heard.landed(&seen, 90);
        heard.applied(&seen, 200, false);
        heard.applied(&seen, 90, true);
        heard.landed(&seen, 90);
        assert_eq!(heard.count(), 3);
    }

    #[test]
    fn a_derivation_that_fails_announces_its_fallback_once_and_a_later_success_its_own_palette() {
        let (seen, heard) = (Edge::new(), heard());
        heard.applied(&seen, 10, true);
        heard.landed(&seen, 50);
        assert_eq!(heard.count(), 1, "the fallback is a real palette change");
        heard.applied(&seen, 50, true);
        heard.landed(&seen, 50);
        assert_eq!(heard.count(), 1, "the same fallback is not announced twice");
        heard.landed(&seen, 90);
        assert_eq!(
            heard.count(),
            2,
            "the palette derived later is its own change"
        );
        heard.landed(&seen, 90);
        assert_eq!(heard.count(), 2);
    }

    #[test]
    fn a_derived_palette_the_shell_started_with_is_not_news_when_its_files_land() {
        let (seen, heard) = (Edge::new(), heard());
        heard.applied(&seen, 90, true);
        heard.landed(&seen, 90);
        assert_eq!(heard.count(), 0);
    }
}
