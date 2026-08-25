//! The config the shell is currently running, and the per-surface slice of it.
//!
//! The context is what lets code reached from outside a surface — an IPC call, a keybind, a service thread — still answer "which config?". It lives here rather than beside the surface registry because a *producer* has no surfaces and still needs the answer: a poll interval or a backend the user configured would otherwise never take effect, with nothing to show for it.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use crate::{Config, Edge};

thread_local! {
    static CONTEXT: RefCell<Option<Arc<Config>>> = const { RefCell::new(None) };
    // What each output's chrome is running, written by the reconciler as it plans that screen's surfaces.
    static RUNNING: RefCell<HashMap<String, Arc<Config>>> = RefCell::new(HashMap::new());
    // How to rebuild the shell. Owned by the startup path, which is the only place that knows how to reconcile surfaces; everything else — the config watcher, a monitor hotplug, `hogar-shell shell reload` — asks here.
    static RELOAD: RefCell<Option<Box<dyn Fn()>>> = const { RefCell::new(None) };
    // Which image a dynamic palette is derived from. Installed by the startup path because answering it means asking the compositor which screen is focused and the wallpaper service what it is showing — neither of which the config can see from here.
    static WALLPAPER_SOURCE: RefCell<Option<WallpaperSource>> = const { RefCell::new(None) };
}

/// How the startup path answers "which image is the palette derived from".
type WallpaperSource = Box<dyn Fn(&Config) -> Option<PathBuf>>;

/// Registers how the shell rebuilds itself. Set once by the startup path, which owns surface reconciliation.
pub fn set_reload_hook(reload: impl Fn() + 'static) {
    RELOAD.with(|hook| *hook.borrow_mut() = Some(Box::new(reload)));
}

/// Re-reads the config and reconciles every surface. Safe to call before the hook is installed (a no-op).
pub fn request_reload() {
    let installed = RELOAD.with(|hook| hook.borrow().is_some());
    if !installed {
        tracing::warn!("reload requested before the shell was set up");
        return;
    }
    RELOAD.with(|hook| {
        if let Some(reload) = hook.borrow().as_ref() {
            reload();
        }
    });
}

/// Registers how to find the image a dynamic palette derives from. Set once by the startup path.
pub fn set_wallpaper_source(source: impl Fn(&Config) -> Option<PathBuf> + 'static) {
    WALLPAPER_SOURCE.with(|hook| *hook.borrow_mut() = Some(Box::new(source)));
}

/// The image a dynamic palette is derived from, or `None` before the shell has started.
pub fn wallpaper_source(config: &Config) -> Option<PathBuf> {
    WALLPAPER_SOURCE.with(|hook| hook.borrow().as_ref().and_then(|find| find(config)))
}

/// The same config as [`CONTEXT`], reachable from any thread.
///
/// Not a duplicate for its own sake: [`config`] is a thread-local, so a *service* thread — which is every producer in the services crate — reads `None` from it and silently falls back to defaults. Written on the driver thread on every reload and only read elsewhere, so the lock is uncontended.
static SHARED: std::sync::Mutex<Option<Arc<Config>>> = std::sync::Mutex::new(None);

/// Publishes the config the shell is now running. Called on startup and on every reload, so anything reached outside a surface resolves against the same config the bars were just rebuilt from.
pub fn set_config(config: Arc<Config>) {
    if let Ok(mut shared) = SHARED.lock() {
        *shared = Some(Arc::clone(&config));
    }
    CONTEXT.with(|c| *c.borrow_mut() = Some(config));
}

/// The running config, or `None` before the shell has started (a unit test, a CLI invocation).
pub fn config() -> Option<Arc<Config>> {
    CONTEXT.with(|c| c.borrow().clone())
}

/// The running config, readable from a service thread. Prefer [`config`] on the driver thread — it needs no lock — and use this wherever the caller is a producer.
pub fn shared_config() -> Option<Arc<Config>> {
    SHARED.lock().ok().and_then(|c| c.clone())
}

/// Puts the context back to how it is before the shell starts. Only a test wants this — nothing in a running shell un-publishes a config — but a test for what happens *without* one has no other way to get there.
pub fn clear_config() {
    CONTEXT.with(|c| *c.borrow_mut() = None);
}

/// Records what `output`'s chrome resolved to, so a surface opened on that screen later can be given the same answer. Called by the reconciler as it plans each output.
pub fn set_output_config(output: &str, config: Arc<Config>) {
    RUNNING.with(|running| running.borrow_mut().insert(output.to_string(), config));
}

/// The config a surface on `output` resolves against: the same per-monitor merge the chrome on that screen is running, so a panel opened from a bar follows the same overrides — and the same edits — as the bar did.
///
/// Falls back to the global config, which is also the right answer for a surface that names no screen.
pub fn config_for(output: Option<&str>) -> Arc<Config> {
    output
        .and_then(|name| RUNNING.with(|running| running.borrow().get(name).cloned()))
        .or_else(config)
        .unwrap_or_default()
}

/// The config a live surface builds against.
///
/// A shared cell rather than a plain value because the surface outlives any one config: after an edit it is the same surface, and the only thing that changed is what it should look like. The reconcile writes, the surface's next build reads.
#[derive(Clone)]
pub struct LiveConfig(Rc<RefCell<Arc<Config>>>);

impl LiveConfig {
    pub fn new(config: Arc<Config>) -> Self {
        Self(Rc::new(RefCell::new(config)))
    }

    pub fn get(&self) -> Arc<Config> {
        Arc::clone(&self.0.borrow())
    }

    pub fn set(&self, config: Arc<Config>) {
        *self.0.borrow_mut() = config;
    }

    /// Whether two handles are the same cell — which is what "the same surface" means across a reload, and so the identity a reconcile test compares.
    pub fn ptr_eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

impl From<Arc<Config>> for LiveConfig {
    fn from(config: Arc<Config>) -> Self {
        Self::new(config)
    }
}

/// What a module needs to know about its bar; carried into the parameterless `.rsx` module entrypoints as per-surface context (rsx `provide`/`inject`, scoped to each surface) with no prop plumbing.
#[derive(Clone)]
pub struct SurfaceEnv {
    pub edge: Edge,
    /// The bar's thickness in px (height for top/bottom, width for left/right).
    pub bar_size: u32,
    /// The monitor this bar lives on, so panels it opens (drawer/float/OSD) land on the same screen; `None` = the compositor's active/default output.
    pub output: Option<String>,
    pub config: Arc<Config>,
}

/// Per-surface context: resolves against this surface's own scope, so a module reading [`surface_env`] — including from an effect — gets THIS bar's env even though all surfaces share one UI thread under M3 (the reactive flush re-enters the surface). Written on every build, so a rebuilt bar's modules read the config the edit produced rather than the one the surface opened under.
pub fn set_surface_env(env: SurfaceEnv) {
    util::state::set_context(env);
}

pub fn surface_env() -> Option<SurfaceEnv> {
    util::state::context::<SurfaceEnv>()
}

/// The margin `edge`'s bar sits at while it is on screen, as `(top, right, bottom, left)`: its own outer gap on the edge it hangs off, plus — for a vertical bar — the insets that keep it clear of a perpendicular one.
///
/// Shared rather than derived per caller because an auto-hiding bar needs the same answer from the other side: the surface is created at its *hidden* margin and animates back to this one, and the two deriving the gap separately is how a revealed bar ends up a few pixels off the position it was configured for.
pub fn bar_margin_for(config: &Config, edge: Edge) -> (i32, i32, i32, i32) {
    let gap = config.edge_gap(edge) as i32;
    let top_inset = perpendicular_inset(config, Edge::Top, gap);
    let bottom_inset = perpendicular_inset(config, Edge::Bottom, gap);
    match edge {
        Edge::Top => (gap, gap, 0, gap),
        Edge::Bottom => (0, gap, gap, gap),
        Edge::Left => (top_inset, 0, bottom_inset, gap),
        Edge::Right => (top_inset, gap, bottom_inset, 0),
    }
}

/// A vertical bar stops short of a horizontal one: by that bar's reserved strip when it has one, and by its own gap when it does not, so the two never overlap at the corner.
fn perpendicular_inset(config: &Config, perp: Edge, own_gap: i32) -> i32 {
    match config.edge_reserved(perp) {
        0 => own_gap,
        reserved => reserved as i32,
    }
}
