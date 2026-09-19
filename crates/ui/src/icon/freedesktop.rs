use std::cell::RefCell;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use telar::{ImageData, SvgData};

use config::surface_env;

/// The desktop icon size a notification card asks the theme for; the card renders it at 36px, so a 48px source downscales cleanly, and scalable (SVG) entries match regardless.
const REQUEST_SIZE: u32 = 48;

/// A resolved application icon, ready to hand to the matching rsx widget: a parsed SVG (rendered untinted, so the app's own colours show) or decoded raster pixels.
#[derive(Clone)]
pub enum AppIcon {
    Vector(Arc<SvgData>),
    Raster(Arc<ImageData>),
}

thread_local! {
    /// Memoizes each reference's resolution per surface thread, so a snapshot-driven card rebuild doesn't re-walk the theme directories (or re-decode the file) on every render.
    static CACHE: RefCell<HashMap<String, Option<AppIcon>>> = RefCell::new(HashMap::new());
}

/// Resolves a freedesktop notification icon `reference` — an absolute path, a `file://` URI, or an icon name per the [Icon Theme Specification](https://specifications.freedesktop.org/icon-theme-spec/latest/) — to a loaded icon, or `None` when it is empty, unresolvable, or of an undecodable format. Memoized per thread.
pub fn resolve_app_icon(reference: &str) -> Option<AppIcon> {
    if reference.is_empty() {
        return None;
    }
    if let Some(hit) = CACHE.with(|c| c.borrow().get(reference).cloned()) {
        return hit;
    }
    let theme = surface_env()
        .map(|env| env.config.icons.app_icon_theme.clone())
        .unwrap_or_default();
    let icon = locate(reference, &theme).and_then(|path| load(&path));
    CACHE.with(|c| c.borrow_mut().insert(reference.to_string(), icon.clone()));
    icon
}

/// A filesystem path for `reference`: the file itself when it is a path or `file://` URI, otherwise the theme lookup for an icon name.
fn locate(reference: &str, theme: &str) -> Option<PathBuf> {
    if let Some(rest) = reference.strip_prefix("file://") {
        let path = PathBuf::from(rest);
        return path.is_file().then_some(path);
    }
    if reference.starts_with('/') {
        let path = PathBuf::from(reference);
        return path.is_file().then_some(path);
    }
    let bases = icon_base_dirs();
    let order = theme_search_order(theme, &bases);
    lookup(reference, REQUEST_SIZE, &bases, &order)
}

fn load(path: &Path) -> Option<AppIcon> {
    match path.extension().and_then(|e| e.to_str()) {
        Some("svg") => {
            let text = fs::read_to_string(path).ok()?;
            SvgData::from_str(&text)
                .ok()
                .map(|svg| AppIcon::Vector(Arc::new(svg)))
        }
        _ => {
            let rgba = image::open(path).ok()?.to_rgba8();
            let (width, height) = rgba.dimensions();
            Some(AppIcon::Raster(Arc::new(ImageData::new(
                rgba.into_raw(),
                width,
                height,
            ))))
        }
    }
}

/// The base directories searched for icon themes, in the spec's precedence: per-user (`$HOME/.icons`, `$XDG_DATA_HOME/icons`) before system (`$XDG_DATA_DIRS/icons`), so a user override wins.
fn icon_base_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(home) = util::paths::home_dir() {
        dirs.push(home.join(".icons"));
    }
    dirs.push(util::paths::xdg_data_home().join("icons"));
    let data_dirs = std::env::var_os("XDG_DATA_DIRS")
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    for entry in std::env::split_paths(&data_dirs) {
        dirs.push(entry.join("icons"));
    }
    dirs
}

/// The ordered theme names to search: the preferred theme, then every theme it `Inherits` (transitively), and finally `hicolor`, the spec-mandated fallback that every theme implicitly inherits.
fn theme_search_order(preferred: &str, bases: &[PathBuf]) -> Vec<String> {
    let start = if preferred.is_empty() {
        detected_icon_theme()
    } else {
        preferred.to_string()
    };
    let mut order = Vec::new();
    let mut stack = vec![start];
    while let Some(theme) = stack.pop() {
        if theme.is_empty() || order.contains(&theme) {
            continue;
        }
        let inherits = theme_index(bases, &theme)
            .and_then(|index| {
                index
                    .get("Icon Theme")
                    .and_then(|s| s.get("Inherits"))
                    .cloned()
            })
            .unwrap_or_default();
        order.push(theme);
        for parent in inherits.split(',').rev() {
            let parent = parent.trim();
            if !parent.is_empty() {
                stack.push(parent.to_string());
            }
        }
    }
    if !order.iter().any(|t| t == "hicolor") {
        order.push("hicolor".to_string());
    }
    order
}

/// The user's configured icon theme, read from the GTK settings file (the de-facto source most desktops honour), or empty when none is set — the caller then falls back to `hicolor`.
fn detected_icon_theme() -> String {
    let config_home = util::paths::xdg_config_home();
    for settings in ["gtk-4.0/settings.ini", "gtk-3.0/settings.ini"] {
        let Ok(text) = fs::read_to_string(config_home.join(settings)) else {
            continue;
        };
        if let Some(name) = parse_ini(&text)
            .get("Settings")
            .and_then(|s| s.get("gtk-icon-theme-name"))
        {
            return name.clone();
        }
    }
    String::new()
}

/// Finds `name` for `size` across `bases`/`themes` per the spec's lookup: theme priority dominates (a match in an earlier theme wins over any parent), and a bare fallback in the base directories (and `/usr/share/pixmaps`) covers themeless icons.
fn lookup(name: &str, size: u32, bases: &[PathBuf], themes: &[String]) -> Option<PathBuf> {
    themes
        .iter()
        .find_map(|theme| lookup_in_theme(name, size, bases, theme))
        .or_else(|| fallback_icon(name, bases))
}

/// The best `name` for `size` within a single theme: a size-exact SVG (crispest), else a size-exact raster, else the nearest-sized icon. `None` when the theme has no `index.theme` or holds the icon at no size.
fn lookup_in_theme(name: &str, size: u32, bases: &[PathBuf], theme: &str) -> Option<PathBuf> {
    let index = theme_index(bases, theme)?;
    let dirs = index.get("Icon Theme").and_then(|s| s.get("Directories"))?;
    let mut exact_raster: Option<PathBuf> = None;
    let mut closest: Option<(u32, PathBuf)> = None;
    for subdir in dirs.split(',').map(str::trim).filter(|d| !d.is_empty()) {
        let Some(props) = index.get(subdir) else {
            continue;
        };
        if !directory_matches_size(props, size) {
            continue;
        }
        let distance = directory_size_distance(props, size);
        for base in bases {
            let Some(hit) = icon_in_dir(&base.join(theme).join(subdir), name) else {
                continue;
            };
            if distance == 0 {
                if hit.extension().and_then(|e| e.to_str()) == Some("svg") {
                    return Some(hit);
                }
                exact_raster.get_or_insert(hit);
            } else if closest.as_ref().is_none_or(|(best, _)| distance < *best) {
                closest = Some((distance, hit));
            }
        }
    }
    exact_raster.or(closest.map(|(_, path)| path))
}

/// The best icon file for `name` in a single directory: a scalable SVG if present, else the first decodable raster. `None` when the directory holds neither.
fn icon_in_dir(dir: &Path, name: &str) -> Option<PathBuf> {
    let svg = dir.join(format!("{name}.svg"));
    if svg.is_file() {
        return Some(svg);
    }
    for ext in ["png", "webp", "jpg", "jpeg"] {
        let raster = dir.join(format!("{name}.{ext}"));
        if raster.is_file() {
            return Some(raster);
        }
    }
    None
}

/// A last-resort lookup for icons that live outside any theme: directly under a base directory, or in the legacy `/usr/share/pixmaps`.
fn fallback_icon(name: &str, bases: &[PathBuf]) -> Option<PathBuf> {
    bases
        .iter()
        .cloned()
        .chain(std::iter::once(PathBuf::from("/usr/share/pixmaps")))
        .find_map(|dir| icon_in_dir(&dir, name))
}

/// Whether a theme directory holds icons usable at `size`, per its `Type` (`Fixed`/`Scalable`/`Threshold`). The spec's `Scale` key is intentionally not gated: a notification icon renders at one fixed logical size, so a `@2x` directory's higher-density source is fine (and, for themes that keep an app icon only under `@2x`, it is the difference between resolving the icon and falling back to the dot).
fn directory_matches_size(props: &HashMap<String, String>, size: u32) -> bool {
    let dir_size = prop(props, "Size", 0);
    match props.get("Type").map(String::as_str).unwrap_or("Threshold") {
        "Fixed" => dir_size == size,
        "Scalable" => {
            let min = prop(props, "MinSize", dir_size);
            let max = prop(props, "MaxSize", dir_size);
            (min..=max).contains(&size)
        }
        _ => {
            let threshold = prop(props, "Threshold", 2);
            dir_size.abs_diff(size) <= threshold
        }
    }
}

/// How far a directory's icons are from `size`, for picking the nearest when no directory matches exactly.
fn directory_size_distance(props: &HashMap<String, String>, size: u32) -> u32 {
    let dir_size = prop(props, "Size", 0);
    match props.get("Type").map(String::as_str).unwrap_or("Threshold") {
        "Scalable" => {
            let min = prop(props, "MinSize", dir_size);
            let max = prop(props, "MaxSize", dir_size);
            min.saturating_sub(size).max(size.saturating_sub(max))
        }
        _ => dir_size.abs_diff(size),
    }
}

fn prop(props: &HashMap<String, String>, key: &str, default: u32) -> u32 {
    props
        .get(key)
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

thread_local! {
    /// Parsed `index.theme` per theme name, so a lookup that touches many subdirectories parses the file once.
    static INDEX_CACHE: RefCell<HashMap<String, Option<Arc<ThemeIndex>>>> =
        RefCell::new(HashMap::new());
}

type ThemeIndex = HashMap<String, HashMap<String, String>>;

/// The parsed `index.theme` for `theme`, from the first base directory that has one. Cached per thread.
fn theme_index(bases: &[PathBuf], theme: &str) -> Option<Arc<ThemeIndex>> {
    if let Some(hit) = INDEX_CACHE.with(|c| c.borrow().get(theme).cloned()) {
        return hit;
    }
    let parsed = bases
        .iter()
        .find_map(|base| fs::read_to_string(base.join(theme).join("index.theme")).ok())
        .map(|text| Arc::new(parse_ini(&text)));
    INDEX_CACHE.with(|c| c.borrow_mut().insert(theme.to_string(), parsed.clone()));
    parsed
}

/// A minimal desktop-INI parser: `[section]` headers and `key=value` lines into a section→key→value map. Enough for `index.theme` and GTK settings; comments (`#`) and blank lines are ignored.
fn parse_ini(text: &str) -> ThemeIndex {
    let mut sections: ThemeIndex = HashMap::new();
    let mut current = String::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(section) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            current = section.to_string();
        } else if let Some((key, value)) = line.split_once('=') {
            sections
                .entry(current.clone())
                .or_default()
                .insert(key.trim().to_string(), value.trim().to_string());
        }
    }
    sections
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn directory_matches_size_honors_type() {
        assert!(directory_matches_size(
            &map(&[("Size", "48"), ("Type", "Fixed")]),
            48
        ));
        assert!(!directory_matches_size(
            &map(&[("Size", "48"), ("Type", "Fixed")]),
            32
        ));

        let scalable = map(&[
            ("Size", "48"),
            ("MinSize", "16"),
            ("MaxSize", "256"),
            ("Type", "Scalable"),
        ]);
        assert!(directory_matches_size(&scalable, 48));
        assert!(directory_matches_size(&scalable, 16));
        assert!(!directory_matches_size(&scalable, 512));

        // Threshold is the default type, defaulting to ±2.
        let threshold = map(&[("Size", "48")]);
        assert!(directory_matches_size(&threshold, 50));
        assert!(!directory_matches_size(&threshold, 53));
    }

    #[test]
    fn parse_ini_reads_sections_keys_and_skips_noise() {
        let ini = "# comment\n[Icon Theme]\nName = Hicolor\nDirectories=48x48/apps,scalable/apps\n\n[48x48/apps]\nSize=48\nType=Fixed\n";
        let parsed = parse_ini(ini);
        assert_eq!(parsed["Icon Theme"]["Name"], "Hicolor");
        assert_eq!(
            parsed["Icon Theme"]["Directories"],
            "48x48/apps,scalable/apps"
        );
        assert_eq!(parsed["48x48/apps"]["Type"], "Fixed");
    }

    #[test]
    fn theme_search_order_follows_inherits_and_appends_hicolor() {
        let root = std::env::temp_dir().join(format!("hogar-shell-icons-{}", std::process::id()));
        let theme_dir = root.join("Papirus");
        fs::create_dir_all(&theme_dir).unwrap();
        fs::write(
            theme_dir.join("index.theme"),
            "[Icon Theme]\nInherits=Adwaita,gnome\n",
        )
        .unwrap();

        let order = theme_search_order("Papirus", std::slice::from_ref(&root));
        assert_eq!(order, vec!["Papirus", "Adwaita", "gnome", "hicolor"]);
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn lookup_prefers_size_match_and_scalable_then_falls_back() {
        let root = std::env::temp_dir().join(format!("hogar-shell-lookup-{}", std::process::id()));
        let theme = root.join("Test");
        let fixed = theme.join("48x48/apps");
        let scalable = theme.join("scalable/apps");
        fs::create_dir_all(&fixed).unwrap();
        fs::create_dir_all(&scalable).unwrap();
        fs::write(
            theme.join("index.theme"),
            "[Icon Theme]\nDirectories=48x48/apps,scalable/apps\n\n[48x48/apps]\nSize=48\nType=Fixed\n\n[scalable/apps]\nSize=48\nMinSize=8\nMaxSize=512\nType=Scalable\n",
        )
        .unwrap();
        fs::write(fixed.join("firefox.png"), b"x").unwrap();
        fs::write(scalable.join("firefox.svg"), b"<svg/>").unwrap();

        // Both directories match size 48; the scalable SVG is preferred over the fixed PNG.
        let hit = lookup(
            "firefox",
            48,
            std::slice::from_ref(&root),
            &["Test".to_string()],
        )
        .unwrap();
        assert_eq!(hit, scalable.join("firefox.svg"));

        // A themeless icon resolves via the base-directory fallback.
        fs::write(root.join("loose.png"), b"x").unwrap();
        let loose = lookup(
            "loose",
            48,
            std::slice::from_ref(&root),
            &["Test".to_string()],
        )
        .unwrap();
        assert_eq!(loose, root.join("loose.png"));

        assert!(
            lookup(
                "absent",
                48,
                std::slice::from_ref(&root),
                &["Test".to_string()]
            )
            .is_none()
        );
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn locate_resolves_paths_and_file_uris() {
        let root = std::env::temp_dir().join(format!("hogar-shell-locate-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let file = root.join("icon.png");
        fs::write(&file, b"x").unwrap();

        assert_eq!(locate(file.to_str().unwrap(), ""), Some(file.clone()));
        assert_eq!(
            locate(&format!("file://{}", file.display()), ""),
            Some(file.clone())
        );
        assert!(locate("/no/such/icon.png", "").is_none());
        fs::remove_dir_all(&root).ok();
    }
}
