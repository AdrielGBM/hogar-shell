use std::cell::RefCell;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

use platform_wayland::{EventSender, timeout, watch};
use serde::Deserialize;
use telar::{
    AssetState, Color, LayoutError, LayoutItem, LayoutStyle, ObjectFit, ReactiveList, ReadSignal,
    RectStyle, RwSignal, SpinnerProps, StyledContainer, Svg, SvgData, signal, spinner, use_theme,
};

use config::surface_env;
use config::theme::NordTheme;

mod freedesktop;
mod picker;
pub use freedesktop::{AppIcon, resolve_app_icon};
pub(crate) use picker::grid_preview;
pub use picker::icon_picker_overlay;

/// An **application's own** icon at `size`, or `None` when `reference` resolves to nothing.
///
/// Distinct from [`icon_view`], which fetches a themable Iconify glyph and tints it: this renders the app's artwork untinted and at its own colours, which is what a notification card, a window chip and a launcher row all want. Resolution follows the freedesktop icon spec via [`resolve_app_icon`] and is memoized per surface.
pub fn app_icon_view(
    reference: &str,
    size: f32,
) -> Result<Option<Box<dyn LayoutItem>>, LayoutError> {
    app_icon_view_tinted(reference, size, None)
}

/// [`app_icon_view`] with an optional flat tint, for a surface that wants the application's artwork to take the bar's own colour instead of its own — the tray's `recolour`.
///
/// Only vector artwork can be tinted; a raster icon is drawn as it is, since repainting decoded pixels would mean either discarding them or guessing which of them are "the shape".
pub fn app_icon_view_tinted(
    reference: &str,
    size: f32,
    tint: Option<Color>,
) -> Result<Option<Box<dyn LayoutItem>>, LayoutError> {
    let Some(icon) = resolve_app_icon(reference) else {
        return Ok(None);
    };
    let style = LayoutStyle::new().width(size).height(size).flex_shrink(0.0);
    let widget: Box<dyn LayoutItem> = match icon {
        AppIcon::Vector(svg) => Box::new(Svg::new(
            style,
            move || svg.clone(),
            move || tint,
            || ObjectFit::Contain,
        )?),
        AppIcon::Raster(data) => Box::new(telar::Image::new(
            style,
            move || data.clone(),
            || telar::Raster::Smooth,
            || ObjectFit::Contain,
        )?),
    };
    Ok(Some(widget))
}

/// A transient download failure (the shell often starts before the network is up at login) keeps the icon on its spinner and re-tries a bounded number of times, so icons self-heal once connectivity arrives without hammering the endpoint over a genuine 404.
const MAX_ATTEMPTS: u32 = 8;
const RETRY_DELAY: Duration = Duration::from_secs(4);

/// A network icon addressed as `set/name` (Iconify layout). A bare name takes the configured default set; `set:name` overrides it inline, so many sets flow through one endpoint.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
struct IconId {
    set: String,
    name: String,
}

impl IconId {
    fn parse(raw: &str, default_set: &str) -> Self {
        match raw.split_once(':') {
            Some((set, name)) if !set.is_empty() && !name.is_empty() => Self {
                set: set.to_string(),
                name: name.to_string(),
            },
            _ => Self {
                set: default_set.to_string(),
                name: raw.to_string(),
            },
        }
    }

    fn cache_path(&self, root: &Path) -> PathBuf {
        root.join(&self.set).join(format!("{}.svg", self.name))
    }

    fn url(&self, provider: &str) -> String {
        format!(
            "{}/{}/{}.svg",
            provider.trim_end_matches('/'),
            self.set,
            self.name
        )
    }
}

/// What the download worker needs; owned on its own thread, so it holds only `Send` data (no signals).
#[derive(Clone)]
struct FetchConfig {
    provider: String,
    cache_dir: PathBuf,
}

type IconResult = (IconId, Option<Arc<SvgData>>);

/// The per-surface-thread reactive icon registry: reading an icon returns a signal that starts `Loading` and advances to `Ready`/`Failed` as the download lands, re-rendering whoever read it. Transport lives in [`run_worker`]; this side only holds signals, tracks retries, and enqueues requests.
struct IconStore {
    signals: RefCell<HashMap<IconId, RwSignal<AssetState<Arc<SvgData>>>>>,
    attempts: RefCell<HashMap<IconId, u32>>,
    requests: Sender<IconId>,
    /// Where the worker keeps what it has already downloaded, so a request nobody will answer can still be resolved here — see [`IconStore::svg`].
    cache_dir: PathBuf,
    default_set: String,
    /// The `[icons]` config this store was built from. All surfaces share the UI thread, so the store is process-wide; recording its config is what lets a reload notice the endpoint or default set changed.
    config: config::IconsConfig,
}

impl IconStore {
    fn svg(&self, id: &str) -> ReadSignal<AssetState<Arc<SvgData>>> {
        let icon_id = IconId::parse(id, &self.default_set);
        let mut signals = self.signals.borrow_mut();
        // Detached: this cache outlives every scope that reads from it, and the first read is somebody's build. Attributed to that build, the glyph would be freed when it went away and every later reader would find a dead signal.
        let handle = signals.entry(icon_id.clone()).or_insert_with(|| {
            telar::detached(|| {
                // A closed channel means no worker is listening — `watch` starts none without a layer-shell event loop, which is every `[preview]` and every headless test. The glyph is then read from the disk cache here or never at all, so a preview shows real icons instead of a page of spinners.
                if self.requests.send(icon_id.clone()).is_err()
                    && let Some(svg) = cached_icon(&icon_id, &self.cache_dir)
                {
                    return signal(AssetState::Ready(svg));
                }
                signal(AssetState::Loading)
            })
        });
        handle.read_only()
    }
}

thread_local! {
    static STORE: RefCell<Option<IconStore>> = const { RefCell::new(None) };
}

/// A reactive icon widget: shows the self-animating [`spinner`] while the glyph downloads (nothing is hardcoded — the spinner is rsx's own indeterminate ring), then swaps to the tinted SVG once it lands. `name` and `tint` are reactive closures, so the icon re-resolves when either changes (e.g. battery ↔ charging). Drop this into a `.rsx` view with `widget`.
pub fn icon_view(
    name: impl Fn() -> String + 'static,
    tint: impl Fn() -> Color + Clone + 'static,
    size: f32,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let icon_stroke = use_theme::<NordTheme>().icon_stroke;
    let source = move || vec![icon_state(&name())];
    let key = |state: &AssetState<Arc<SvgData>>| state.as_ready().map(|svg| svg.id());
    let build = move |state: AssetState<Arc<SvgData>>| -> Result<Box<dyn LayoutItem>, LayoutError> {
        match state {
            AssetState::Ready(svg) => {
                let tint = tint.clone();
                let widget = Svg::new(
                    LayoutStyle::new().width(size).height(size),
                    move || svg.clone(),
                    move || Some(tint()),
                    || ObjectFit::Contain,
                )?
                .with_stroke(move || icon_stroke);
                Ok(Box::new(widget))
            }
            // A glyph that has run out of retries is not still loading, and must not keep spinning as though it were: an unreachable provider and a misspelled icon name would look identical to a working one forever. It settles into a dim placeholder instead, which reads as "this icon is missing".
            AssetState::Failed => {
                let tint = tint.clone();
                // Inset so the placeholder reads as a gap in the row rather than a filled chip, and keeps the module's footprint identical to a loaded glyph so nothing shifts when it settles.
                let inset = (size * 0.25).max(1.0);
                let side = size - inset * 2.0;
                Ok(Box::new(StyledContainer::new(
                    LayoutStyle::new()
                        .width(side)
                        .height(side)
                        .margin(telar::Margin::all(inset)),
                    move |_| RectStyle::filled(tint().with_alpha(0.3), side * 0.25),
                    vec![],
                )?))
            }
            _ => spinner(
                SpinnerProps::props()
                    .color(telar::Reactive::of(tint.clone()))
                    .size(size)
                    .build(),
                telar::Children::default(),
            ),
        }
    };
    Ok(Box::new(ReactiveList::new(source, key, build, 0.0)?))
}

/// Whether `name` has been requested from the icon store yet — i.e. some widget read it via [`icon_view`]. Test-only, so a picker test can assert a cell actually became visible and asked for its glyph.
#[cfg(test)]
pub(crate) fn was_requested(name: &str) -> bool {
    STORE.with(|s| {
        let borrow = s.borrow();
        let Some(store) = borrow.as_ref() else {
            return false;
        };
        let id = IconId::parse(name, &store.default_set);
        store.signals.borrow().contains_key(&id)
    })
}

/// The current load state of `name`, subscribing the caller so it re-renders as the icon resolves. `name` is a bare glyph (`bell`) or a `set:name` for another Iconify set (`mdi:home`).
pub(crate) fn icon_state(name: &str) -> AssetState<Arc<SvgData>> {
    ensure_store();
    STORE.with(|s| {
        s.borrow()
            .as_ref()
            .expect("ensure_store initializes the icon store")
            .svg(name)
            .get()
    })
}

/// The `[icons]` config to resolve against: the bar surface in scope, else the config the shell is running.
///
/// Falling back to `IconsConfig::default()` here would be wrong, not merely imprecise. A panel, OSD or popup has no `SurfaceEnv`, so with a customised `[icons]` it would disagree with the bar about the store's config and [`ensure_store`] would tear the store down and rebuild it on every panel open — cancelling every in-flight download in the process.
fn icons_config() -> config::IconsConfig {
    surface_env()
        .map(|env| env.config.icons.clone())
        .or_else(|| config::config().map(|c| c.icons.clone()))
        .unwrap_or_default()
}

/// Builds the process-wide icon store and starts its download worker.
///
/// **Must be called at app level, not from inside a surface build.** `watch` binds its channel to whichever surface is being built when it runs, and tears that channel down with the surface. The store is process-wide (one UI thread, one thread-local), so a worker owned by one surface dies the moment that surface's content is rebuilt — which a config reload does to every bar — and because the store then still exists with a matching config, [`ensure_store`] returns early and never starts another. Every icon requested afterwards would spin forever. Registering from the app level leaves `CURRENT_SOURCES` unset, so the channel is process-lived like the config watcher.
///
/// Idempotent: a call with the same `[icons]` config is a no-op, so the reload path can call it unconditionally. A changed config rebuilds the store, which is how editing the provider or default set takes effect.
pub fn init_store(icons: &config::IconsConfig) {
    let current = STORE.with(|s| s.borrow().as_ref().map(|store| store.config == *icons));
    match current {
        Some(true) => return,
        // Dropping the old store closes its request channel, which retires its worker thread; the cached signals go with it, so each icon re-resolves against the new endpoint (disk cache first).
        Some(false) => {
            STORE.with(|s| *s.borrow_mut() = None);
            COLLECTIONS.with(|c| c.borrow_mut().clear());
        }
        None => {}
    }

    let (requests, incoming) = channel::<IconId>();
    STORE.with(|s| {
        *s.borrow_mut() = Some(IconStore {
            signals: RefCell::new(HashMap::new()),
            attempts: RefCell::new(HashMap::new()),
            requests,
            cache_dir: cache_dir(),
            default_set: icons.default_set.clone(),
            config: icons.clone(),
        });
    });

    let fetch = FetchConfig {
        provider: icons.provider.clone(),
        cache_dir: cache_dir(),
    };
    // When there is no layer-shell event loop (headless tests), `watch` is a no-op: no worker runs and every icon stays on its spinner, which is exactly what an offline render shows.
    watch(
        move |sender| run_worker(incoming, fetch, sender),
        |(id, data)| deliver(id, data),
    );
}

/// Lazy fallback for call sites the shell's startup doesn't reach — a headless render, a unit test. The running shell builds the store up front via [`init_store`]; this only fills in when nothing has.
fn ensure_store() {
    if STORE.with(|s| s.borrow().is_some()) {
        return;
    }
    init_store(&icons_config());
}

fn deliver(id: IconId, data: Option<Arc<SvgData>>) {
    STORE.with(|s| {
        let borrow = s.borrow();
        let Some(store) = borrow.as_ref() else {
            return;
        };
        match data {
            Some(svg) => {
                store.attempts.borrow_mut().remove(&id);
                // Clone the signal handle out and drop the `signals` borrow BEFORE `set`: under M3's shared runtime a signal write flushes effects synchronously, which re-renders an icon → `svg()` → `signals.borrow_mut()`; holding the borrow across `set` would re-enter and panic.
                let handle = store.signals.borrow().get(&id).cloned();
                if let Some(handle) = handle {
                    handle.set(AssetState::Ready(svg));
                }
            }
            None => {
                let attempts = {
                    let mut map = store.attempts.borrow_mut();
                    let count = map.entry(id.clone()).or_insert(0);
                    *count += 1;
                    *count
                };
                if attempts < MAX_ATTEMPTS {
                    let requests = store.requests.clone();
                    timeout(RETRY_DELAY, move || {
                        let _ = requests.send(id);
                    });
                } else {
                    tracing::warn!(
                        "icon '{}:{}' gave up after {MAX_ATTEMPTS} attempts; check the name and the [icons] provider",
                        id.set,
                        id.name
                    );
                    let handle = store.signals.borrow().get(&id).cloned();
                    if let Some(handle) = handle {
                        handle.set(AssetState::Failed);
                    }
                }
            }
        }
    });
}

/// Blocks on the request channel, resolving each icon from disk cache or the network and shipping the parsed `SvgData` back to the UI thread. Runs on a dedicated thread (via `watch`) and ends when the store — and thus the request sender — is dropped on surface teardown.
fn run_worker(incoming: Receiver<IconId>, fetch: FetchConfig, sender: EventSender<IconResult>) {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(15)))
        .build()
        .into();
    for id in incoming {
        let data = load_icon(&id, &fetch, &agent);
        if !sender.send((id, data)) {
            break;
        }
    }
}

/// The glyph as a previous download left it on disk, or `None` when it was never fetched.
fn cached_icon(id: &IconId, cache_dir: &Path) -> Option<Arc<SvgData>> {
    let text = fs::read_to_string(id.cache_path(cache_dir)).ok()?;
    SvgData::from_str(&text).ok().map(Arc::new)
}

/// The glyph from the disk cache, else downloaded and cached. A cache write that fails goes unreported: the glyph is already parsed and in hand, and all it costs is a download next time.
///
/// Written with [`util::fs::write_atomic`] directly rather than through the writer's queue: the name is the icon's id, so there is no order between writes to protect, only a file that [`cached_icon`] must never read half-written.
fn load_icon(id: &IconId, fetch: &FetchConfig, agent: &ureq::Agent) -> Option<Arc<SvgData>> {
    if let Some(svg) = cached_icon(id, &fetch.cache_dir) {
        return Some(svg);
    }
    let path = id.cache_path(&fetch.cache_dir);

    let body = agent
        .get(&id.url(&fetch.provider))
        .call()
        .ok()?
        .body_mut()
        .read_to_string()
        .ok()?;
    if !body.contains("<svg") {
        return None;
    }
    let svg = SvgData::from_str(&body).ok()?;
    let _ = util::fs::write_atomic(&path, body.as_bytes());
    Some(Arc::new(svg))
}

fn cache_dir() -> PathBuf {
    util::paths::cache_dir().join("icons")
}

/// The state of loading an icon set from Iconify's `/collection` endpoint. `Ready` carries the set's `set:name` ids (ready for [`icon_view`]); `Unavailable` covers a provider that can't list icons (a 404) or a transport error — the picker shows a hint rather than failing. The picker filters `Ready` client-side.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CollectionState {
    Loading,
    Ready(Vec<String>),
    Unavailable,
}

#[derive(Deserialize)]
struct CollectionResponse {
    #[serde(default)]
    uncategorized: Vec<String>,
    #[serde(default)]
    categories: HashMap<String, Vec<String>>,
}

thread_local! {
    // Per-surface cache: a set is fetched once, so reopening the picker reuses the loaded list instead of re-downloading it (and re-registering a worker) every time.
    static COLLECTIONS: RefCell<HashMap<String, ReadSignal<CollectionState>>> =
        RefCell::new(HashMap::new());
}

/// Loads icon set `set`'s full name list from the configured provider's `/collection` endpoint on a worker thread, returning a reactive [`CollectionState`] that advances from `Loading` to `Ready`/`Unavailable`. Cached per surface thread, so the set is fetched at most once.
pub fn icon_collection(set: &str) -> ReadSignal<CollectionState> {
    // A settled result is reused; one still `Loading` is treated as a miss and re-fetched. That entry belongs to a picker that closed mid-download, and its worker died with that surface — caching it would leave the set stuck on its spinner for the rest of the session, however many times the picker is reopened.
    let cached = COLLECTIONS.with(|c| c.borrow().get(set).cloned());
    if let Some(existing) = cached
        && existing.peek() != CollectionState::Loading
    {
        return existing;
    }
    let result = signal(CollectionState::Loading);
    let read = result.read_only();
    COLLECTIONS.with(|c| c.borrow_mut().insert(set.to_string(), read));

    let provider = search_provider();
    let set = set.to_string();
    let setter = result;
    watch(
        move |sender| {
            let _ = sender.send(load_collection(&provider, &set));
        },
        move |state: CollectionState| setter.set(state),
    );
    read
}

fn search_provider() -> String {
    surface_env()
        .map(|e| e.config.icons.provider.clone())
        .unwrap_or_else(|| "https://api.iconify.design".to_string())
}

fn load_collection(provider: &str, set: &str) -> CollectionState {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(15)))
        .build()
        .into();
    let url = format!(
        "{}/collection?prefix={}",
        provider.trim_end_matches('/'),
        set
    );
    let body = match agent.get(&url).call() {
        Ok(mut resp) => match resp.body_mut().read_to_string() {
            Ok(body) => body,
            Err(_) => return CollectionState::Unavailable,
        },
        // A 404 (provider that can't list icons) or any transport error: nothing to show.
        Err(_) => return CollectionState::Unavailable,
    };
    match serde_json::from_str::<CollectionResponse>(&body) {
        Ok(collection) => {
            let ids = collection_ids(set, collection);
            if ids.is_empty() {
                CollectionState::Unavailable
            } else {
                CollectionState::Ready(ids)
            }
        }
        Err(_) => CollectionState::Unavailable,
    }
}

/// Flattens a `/collection` response (uncategorized plus every category) into a sorted, de-duplicated list of `set:name` ids.
fn collection_ids(set: &str, collection: CollectionResponse) -> Vec<String> {
    let mut names = collection.uncategorized;
    for list in collection.categories.into_values() {
        names.extend(list);
    }
    names.sort();
    names.dedup();
    names
        .into_iter()
        .map(|name| format!("{set}:{name}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_parsing_splits_set_and_defaults() {
        let bare = IconId::parse("bell", "lucide");
        assert_eq!((bare.set.as_str(), bare.name.as_str()), ("lucide", "bell"));
        let qualified = IconId::parse("mdi:home", "lucide");
        assert_eq!(
            (qualified.set.as_str(), qualified.name.as_str()),
            ("mdi", "home")
        );
        let empty_set = IconId::parse(":oops", "lucide");
        assert_eq!(
            empty_set.set, "lucide",
            "a leading colon is not a set override"
        );
    }

    #[test]
    fn url_and_cache_path_follow_iconify_layout() {
        let id = IconId::parse("mdi:home", "lucide");
        assert_eq!(
            id.url("https://api.iconify.design/"),
            "https://api.iconify.design/mdi/home.svg",
            "trailing slash on the provider does not double up"
        );
        let root = PathBuf::from("/cache");
        assert_eq!(id.cache_path(&root), PathBuf::from("/cache/mdi/home.svg"));
    }

    #[test]
    fn icon_state_is_loading_without_a_surface_a_network_or_a_cached_copy() {
        assert!(
            matches!(
                icon_state("hogar-shell-test:nothing-was-ever-cached-here"),
                AssetState::Loading
            ),
            "with no event loop and nothing on disk the icon has nothing to resolve from, so it stays on its spinner"
        );
    }

    /// The other half of that: a glyph a previous run already downloaded is readable without the worker, which is what makes a `[preview]` — where `watch` starts none — draw real icons instead of a page of spinners.
    #[test]
    fn a_cached_glyph_resolves_with_no_worker_to_ask() {
        let root = std::env::temp_dir().join(format!("hogar-shell-icon-{}", std::process::id()));
        let id = IconId::parse("mdi:home", "lucide");
        let path = id.cache_path(&root);
        fs::create_dir_all(path.parent().expect("the cache path has a set directory")).unwrap();
        fs::write(
            &path,
            r#"<svg viewBox="0 0 16 16"><path d="M0 0h16v16H0z"/></svg>"#,
        )
        .unwrap();

        assert!(cached_icon(&id, &root).is_some(), "read back off disk");
        assert!(
            cached_icon(&IconId::parse("mdi:absent", "lucide"), &root).is_none(),
            "and a glyph nobody downloaded is still a miss"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn init_store_is_idempotent_and_rebuilds_only_on_a_config_change() {
        use config::IconsConfig;

        let base = IconsConfig::default();
        init_store(&base);
        // Requesting an icon registers its signal. Whether that registration survives is what distinguishes a no-op from a rebuild — a rebuild drops the store, and with it every in-flight download.
        let _ = icon_state("bell");
        assert!(was_requested("bell"), "the icon was registered");

        init_store(&base);
        assert!(
            was_requested("bell"),
            "an unchanged config must not tear the store down — doing so cancels every in-flight download"
        );

        let changed = IconsConfig {
            default_set: "mdi".to_string(),
            ..IconsConfig::default()
        };
        init_store(&changed);
        let rebuilt = STORE.with(|s| s.borrow().as_ref().map(|store| store.default_set.clone()));
        assert_eq!(
            rebuilt.as_deref(),
            Some("mdi"),
            "a changed [icons] config re-resolves icons against the new set"
        );
        assert!(
            !was_requested("bell"),
            "and the old set's cached signals go with it"
        );

        // Leave the thread-local as the rest of the suite expects to find it.
        STORE.with(|s| *s.borrow_mut() = None);
        COLLECTIONS.with(|c| c.borrow_mut().clear());
    }

    #[test]
    fn a_collection_stuck_loading_is_not_served_from_cache() {
        // Regression: a picker closed mid-download takes its worker with it (the `watch` channel is bound to that surface). Serving the still-`Loading` signal back would strand the set on its spinner for the rest of the session, however many times the picker is reopened.
        COLLECTIONS.with(|c| c.borrow_mut().clear());
        let stalled = signal(CollectionState::Loading);
        COLLECTIONS.with(|c| {
            c.borrow_mut()
                .insert("lucide".to_string(), stalled.read_only())
        });

        let handed_out = icon_collection("lucide");
        // Moving the stalled signal proves the caller was handed a different one: a reused entry would follow.
        stalled.set(CollectionState::Unavailable);
        assert_eq!(
            handed_out.peek(),
            CollectionState::Loading,
            "a stalled entry must be replaced by a fresh fetch, not reused"
        );

        // A settled entry, by contrast, is exactly what the cache is for.
        let ready = CollectionState::Ready(vec!["lucide:home".to_string()]);
        let settled = signal(ready.clone());
        COLLECTIONS.with(|c| {
            c.borrow_mut()
                .insert("mdi".to_string(), settled.read_only())
        });
        assert_eq!(
            icon_collection("mdi").peek(),
            ready,
            "a loaded set is reused instead of re-downloaded"
        );

        COLLECTIONS.with(|c| c.borrow_mut().clear());
    }

    #[test]
    fn collection_ids_flattens_prefixes_sorts_and_dedups() {
        let response: CollectionResponse = serde_json::from_str(
            r#"{"prefix":"lucide","uncategorized":["home","bell"],"categories":{"Arrows":["arrow-up","home"]}}"#,
        )
        .unwrap();
        let ids = collection_ids("lucide", response);
        // Uncategorized + every category, prefixed with the set, sorted, with the duplicate `home` removed.
        assert_eq!(
            ids,
            vec![
                "lucide:arrow-up".to_string(),
                "lucide:bell".to_string(),
                "lucide:home".to_string(),
            ]
        );
    }
}
