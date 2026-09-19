use std::cell::RefCell;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use telar::{
    AssetState, Color, LayoutError, LayoutItem, LayoutStyle, ObjectFit, ReactiveList, ReadSignal,
    RectStyle, SpinnerProps, StyledContainer, Svg, SvgData, spinner, use_theme,
};
use util::asset::{Load, Loader, Retry};

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

/// Where the download worker fetches from and caches to; owned on its own thread, so it holds only `Send` data.
#[derive(Clone)]
struct FetchConfig {
    provider: String,
    cache_dir: PathBuf,
}

/// The process-wide icon registry: a glyph or a set's names is a signal that starts `Loading` and advances as its download lands. All surfaces share the UI thread, so one store serves them all, rebuilt when the `[icons]` config it was built from changes.
struct IconStore {
    glyphs: Loader<IconId, Arc<SvgData>>,
    collections: Loader<String, Vec<String>>,
    cache_dir: PathBuf,
    default_set: String,
    config: config::IconsConfig,
}

impl IconStore {
    fn new(icons: &config::IconsConfig) -> Self {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(15)))
            .build()
            .into();
        let fetch = FetchConfig {
            provider: icons.provider.clone(),
            cache_dir: cache_dir(),
        };
        let glyph_agent = agent.clone();
        let glyphs = Loader::retrying(
            Retry {
                attempts: MAX_ATTEMPTS,
                delay: RETRY_DELAY,
                gave_up: |id: &IconId| {
                    tracing::warn!(
                        "icon '{}:{}' gave up after {MAX_ATTEMPTS} attempts; check the name and the [icons] provider",
                        id.set,
                        id.name
                    )
                },
            },
            move |id: &IconId| load_icon(id, &fetch, &glyph_agent),
        );
        let provider = icons.provider.clone();
        let collections = Loader::new(move |set: &String| load_collection(&provider, set, &agent));
        Self {
            glyphs,
            collections,
            cache_dir: cache_dir(),
            default_set: icons.default_set.clone(),
            config: icons.clone(),
        }
    }

    /// `name`, read off the disk cache on the frame it is asked for when a previous download left it there — which is also what lets a `[preview]`, where no worker runs, draw real icons.
    fn svg(&self, name: &str) -> ReadSignal<Load<Arc<SvgData>>> {
        self.glyphs
            .get(IconId::parse(name, &self.default_set), |id| {
                cached_icon(id, &self.cache_dir)
            })
    }

    fn retire(self) {
        self.glyphs.retire();
        self.collections.retire();
    }
}

thread_local! {
    static STORE: RefCell<Option<IconStore>> = const { RefCell::new(None) };
}

fn with_store<R>(read: impl FnOnce(&IconStore) -> R) -> R {
    ensure_store();
    STORE.with(|s| {
        read(
            s.borrow()
                .as_ref()
                .expect("ensure_store initializes the icon store"),
        )
    })
}

/// A reactive icon widget: shows the self-animating [`spinner`] while the glyph downloads, then swaps to the tinted SVG once it lands. `name` and `tint` are reactive closures, so the icon re-resolves when either changes (e.g. battery ↔ charging).
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
            AssetState::Loading => spinner(
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
        s.borrow()
            .as_ref()
            .is_some_and(|store| store.glyphs.has(&IconId::parse(name, &store.default_set)))
    })
}

/// The current load state of `name`, subscribing the caller so it re-renders as the icon resolves. `name` is a bare glyph (`bell`) or a `set:name` for another Iconify set (`mdi:home`).
pub(crate) fn icon_state(name: &str) -> AssetState<Arc<SvgData>> {
    match with_store(|store| store.svg(name)).get() {
        Load::Loading => AssetState::Loading,
        Load::Ready(svg) => AssetState::Ready(svg),
        Load::Missing => AssetState::Failed,
    }
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

/// Builds the process-wide icon store. Idempotent: a call with the same `[icons]` config is a no-op, so the reload path can call it unconditionally, and a changed one replaces the store — freeing every signal the old one handed out and stopping its workers — which is how editing the provider or default set takes effect.
pub fn init_store(icons: &config::IconsConfig) {
    let unchanged = STORE.with(|s| {
        s.borrow()
            .as_ref()
            .is_some_and(|store| store.config == *icons)
    });
    if unchanged {
        return;
    }
    let replaced = STORE.with(|s| s.borrow_mut().replace(IconStore::new(icons)));
    if let Some(old) = replaced {
        old.retire();
    }
}

/// Lazy fallback for call sites the shell's startup doesn't reach — a headless render, a unit test. The running shell builds the store up front via [`init_store`]; this only fills in when nothing has.
fn ensure_store() {
    if STORE.with(|s| s.borrow().is_none()) {
        init_store(&icons_config());
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

#[derive(Deserialize)]
struct CollectionResponse {
    #[serde(default)]
    uncategorized: Vec<String>,
    #[serde(default)]
    categories: HashMap<String, Vec<String>>,
}

/// Icon set `set`'s names from the configured provider's `/collection` endpoint, as `set:name` ids ready for [`icon_view`] — or `Missing` for a provider that cannot list icons. Fetched once per set for as long as the `[icons]` config holds, so reopening the picker reuses the list, and owned by the store rather than by the picker that first asked.
pub fn icon_collection(set: &str) -> ReadSignal<Load<Vec<String>>> {
    with_store(|store| store.collections.get(set.to_string(), |_| None))
}

fn load_collection(provider: &str, set: &str, agent: &ureq::Agent) -> Option<Vec<String>> {
    let url = format!(
        "{}/collection?prefix={}",
        provider.trim_end_matches('/'),
        set
    );
    let body = agent
        .get(&url)
        .call()
        .ok()?
        .body_mut()
        .read_to_string()
        .ok()?;
    let collection = serde_json::from_str::<CollectionResponse>(&body).ok()?;
    let ids = collection_ids(set, collection);
    (!ids.is_empty()).then_some(ids)
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
    }

    /// A changed `[icons]` config frees every signal the old store handed out — one per glyph, one per loaded set — rather than leaving them in the runtime's arena for as long as the shell runs.
    #[test]
    fn replacing_the_store_frees_every_signal_it_held() {
        use config::IconsConfig;

        STORE.with(|s| *s.borrow_mut() = None);

        let base = IconsConfig::default();
        init_store(&base);
        let icon_handle = STORE.with(|s| s.borrow().as_ref().unwrap().svg("bell"));
        let collection_handle = icon_collection("lucide");
        assert!(icon_handle.is_alive(), "the store just made it");
        assert!(collection_handle.is_alive(), "the collection just made it");

        let changed = IconsConfig {
            default_set: "mdi".to_string(),
            ..IconsConfig::default()
        };
        init_store(&changed);

        assert!(
            !icon_handle.is_alive(),
            "a replaced store must free the signals it handed out"
        );
        assert!(
            !collection_handle.is_alive(),
            "and the collection cache's signals along with it"
        );

        STORE.with(|s| *s.borrow_mut() = None);
    }

    #[test]
    fn a_collection_outlives_the_picker_that_first_asked_for_it() {
        STORE.with(|s| *s.borrow_mut() = None);
        let picker = telar::owner_scope();
        let owner = picker.id();
        let first = icon_collection("lucide");
        drop(picker);
        telar::dispose_owner(owner);

        let again = icon_collection("lucide");
        assert_eq!(
            again.get(),
            Load::Loading,
            "the next picker reads the same live entry rather than a handle its predecessor's teardown freed"
        );
        assert_eq!(
            first.peek(),
            again.peek(),
            "one entry per set, not a download per picker"
        );
        STORE.with(|s| *s.borrow_mut() = None);
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
