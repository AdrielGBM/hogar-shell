//! The wallpaper, the library it is picked from, and what is drawn over it.
//!
//! What is left here is the forms this area cannot say in `.rsx`: the ones whose rows are a list the machine decides the length of. The static-shape forms are `.rsx` components beside this file.

use std::path::{Path, PathBuf};
use ui::scale::{corner, paint, space};

use telar::{
    AlignItems, Container, LayoutError, LayoutItem, LayoutStyle, ReactiveList, RectStyle, RwSignal,
    SizeDimension, StyledContainer, Text, box_item, signal,
};

use crate::form::*;
use config::theme::{FontRole, NordTheme};
use config::{BackgroundConfig, WallpaperTransition};

/// How wide one wallpaper tile is, and the shape of its picture — landscape, because a wallpaper is a picture of a screen and a square crop of one is unrecognisable.
const WALL_TILE: f32 = 132.0;
const WALL_ASPECT: f32 = 9.0 / 16.0;

/// The same bound, and the same reason, as the launcher's wallpaper grid: `ReactiveList` builds a widget per tile up front, so a library of two thousand would spend the UI thread before the page appeared. The search box is what reaches past it.
const WALL_TILES: usize = 150;

/// K9: the wallpaper library, grouped by the folder each image was found in.
///
/// `[background] image` names one file and `[background.monitors]` names one per screen — both of which the forms below already edit. What neither of them is, is a way to *see* the library, and choosing a picture from a list of paths is choosing it by its file name. Pressing a tile sets it on every screen, which is the rule the wallpaper commands already follow: a mutation with no target named means all of them.
pub(crate) fn wallpaper_browser_section() -> Result<Box<dyn LayoutItem>, LayoutError> {
    let (config, _path) = crate::form::source();
    let theme = telar::use_theme::<NordTheme>();
    let query = signal(String::new());

    let library = signal(services::wallpaper::all());
    let sink = library;
    platform_wayland::watch(services::wallpaper::subscribe_library, move |entries| {
        sink.set(entries)
    });

    // Which tile reads as the current one. The runtime choice first, then whatever `[background]` resolves to, so a fresh session with nothing chosen at runtime still marks the picture actually on screen.
    let configured = services::wallpaper::current_image(&config, None);
    let current = signal(services::wallpaper::assignment().global.or(configured));
    let current_sink = current;
    platform_wayland::watch(
        services::wallpaper::subscribe,
        move |assignment: services::wallpaper::Assignment| current_sink.set(assignment.global),
    );

    let search = text_field(
        || telar::t!("settings.field.search"),
        query,
        "sunset",
        theme,
    )?;

    let source_library = library.read_only();
    let source_query = query.read_only();
    let source_current = current.read_only();
    let groups = ReactiveList::new(
        move || {
            let entries = source_library.get();
            let query = source_query.get();
            let current = source_current.get();
            folders(&entries, &query)
                .into_iter()
                .map(|(folder, entries)| WallGroup {
                    key: group_key(&folder, &entries, current.as_deref()),
                    folder,
                    entries,
                })
                .collect()
        },
        |group: &WallGroup| group.key.clone(),
        move |group: WallGroup| wallpaper_group(group, current.read_only(), theme),
        14.0,
    )?;

    let empty_library = library.read_only();
    let empty_query = query.read_only();
    let empty = Text::new(
        move || {
            let entries = empty_library.get();
            let query = empty_query.get();
            if entries.is_empty() {
                telar::t!("settings.wallpaper.empty")
            } else if folders(&entries, &query).is_empty() {
                telar::t!("settings.wallpaper.no_match")
            } else {
                String::new()
            }
        },
        LayoutStyle::new(),
        move || theme.text_style(FontRole::Caption, theme.muted),
    )?;

    let clear = save_button(
        SaveButtonProps::props()
            .label(telar::Reactive::of(|| {
                telar::t!("settings.wallpaper.clear")
            }))
            .on_press(std::rc::Rc::new(|| services::wallpaper::clear(None)))
            .build(),
        telar::Children::default(),
    )?;

    Ok(Box::new(Container::new(
        LayoutStyle::new()
            .flex_column()
            .gap(space::md())
            .width(SizeDimension::Percent(1.0)),
        vec![
            section_label(|| telar::t!("settings.section.library"), theme)?,
            search,
            box_item(empty),
            Box::new(groups),
            clear,
        ],
    )?))
}

/// One folder's worth of the library, as the browser draws it.
#[derive(Clone, Debug, PartialEq)]
struct WallGroup {
    /// Keyed on the pictures *and* on which of them is current, so choosing one repaints the ring without the whole library rebuilding under the pointer.
    key: String,
    folder: String,
    entries: Vec<services::wallpaper::Entry>,
}

fn group_key(
    folder: &str,
    entries: &[services::wallpaper::Entry],
    current: Option<&Path>,
) -> String {
    let chosen = entries
        .iter()
        .position(|entry| Some(entry.path.as_path()) == current);
    format!("{folder}|{}|{chosen:?}", entries.len())
}

/// The library narrowed by `query` and grouped by folder, top-level images first and the rest alphabetical — which is the order a file manager would show them in, and the only one a user can predict.
fn folders(
    entries: &[services::wallpaper::Entry],
    query: &str,
) -> Vec<(String, Vec<services::wallpaper::Entry>)> {
    let needle = query.trim().to_lowercase();
    let mut grouped: std::collections::BTreeMap<String, Vec<services::wallpaper::Entry>> =
        std::collections::BTreeMap::new();
    let mut shown = 0usize;
    for entry in entries {
        if shown >= WALL_TILES {
            break;
        }
        if !needle.is_empty()
            && !entry.name.to_lowercase().contains(&needle)
            && !entry.folder.to_lowercase().contains(&needle)
        {
            continue;
        }
        grouped
            .entry(entry.folder.clone())
            .or_default()
            .push(entry.clone());
        shown += 1;
    }
    grouped.into_iter().collect()
}

fn wallpaper_group(
    group: WallGroup,
    current: telar::ReadSignal<Option<PathBuf>>,
    theme: NordTheme,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let mut tiles: Vec<Box<dyn LayoutItem>> = Vec::with_capacity(group.entries.len());
    for entry in group.entries {
        tiles.push(wallpaper_tile(entry, current, theme)?);
    }
    let grid = Container::new(
        LayoutStyle::new()
            .flex_row()
            .flex_wrap()
            .gap(space::md())
            .width(SizeDimension::Percent(1.0)),
        tiles,
    )?;
    // A folder name at the top level is empty, and a heading reading nothing is a gap the user has to guess at.
    let folder = group.folder.clone();
    let heading = subheader(
        move || {
            if folder.is_empty() {
                telar::t!("settings.wallpaper.top_level")
            } else {
                folder.clone()
            }
        },
        theme,
    )?;
    Ok(Box::new(Container::new(
        LayoutStyle::new()
            .flex_column()
            .gap(space::md())
            .width(SizeDimension::Percent(1.0)),
        vec![heading, Box::new(grid)],
    )?))
}

fn wallpaper_tile(
    entry: services::wallpaper::Entry,
    current: telar::ReadSignal<Option<PathBuf>>,
    theme: NordTheme,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let picture_height = (WALL_TILE * WALL_ASPECT).round();
    let picture = ui::thumbnail::view(
        entry.path.clone(),
        WALL_TILE,
        picture_height,
        6.0,
        "image",
        theme,
    )?;

    let name = entry.name.clone();
    let label = Text::new(
        move || name.clone(),
        LayoutStyle::new().width(SizeDimension::Percent(1.0)),
        move || {
            theme
                .text_style(FontRole::Caption, theme.subtle)
                .with_clamp(1, true)
        },
    )?;

    let path = entry.path.clone();
    let chosen = entry.path.clone();
    let rounded = corner::md();
    let tile = StyledContainer::new(
        LayoutStyle::new()
            .flex_column()
            .gap(space::sm())
            .width(WALL_TILE + 8.0)
            .padding_all(space::sm())
            .align_items(AlignItems::CENTER),
        move |_r| {
            let is_current = current.get().as_deref() == Some(path.as_path());
            let fill = if is_current { theme.accent } else { theme.base };
            RectStyle::filled(fill, rounded)
        },
        vec![picture, box_item(label)],
    )?
    .hover_style(paint::md(theme.overlay))
    .on_press(move || services::wallpaper::set(&chosen, None));
    Ok(Box::new(tile))
}

/// Every screen a `[background.monitors]` row should exist for: the ones plugged in now, plus any the config already names.
///
/// Both halves matter. Only listing the connected screens would silently drop the override a user wrote for the monitor they left at the office the moment they saved anything; only listing the configured ones would mean a screen can never get its first override from the UI, which is the whole of J9.
fn monitor_keys(configured: &std::collections::HashMap<String, PathBuf>) -> Vec<String> {
    let mut names: Vec<String> = platform_wayland::outputs()
        .into_iter()
        .filter_map(|output| output.name)
        .collect();
    for name in configured.keys() {
        if !names.contains(name) {
            names.push(name.clone());
        }
    }
    names.sort();
    names
}

pub(crate) fn background_section() -> Result<Box<dyn LayoutItem>, LayoutError> {
    let (config, path) = crate::form::source();
    let theme = telar::use_theme::<NordTheme>();
    let b = &config.background;
    let enabled = signal(b.enabled);
    let image = signal(
        b.image
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_default(),
    );
    let transition = signal(b.transition.id().to_string());
    let transition_ms = signal(b.transition_ms.to_string());

    let mut rows = vec![
        toggle_field(|| telar::t!("settings.field.enabled"), enabled, theme)?,
        text_field(
            || telar::t!("settings.field.image"),
            image,
            "~/wall.png",
            theme,
        )?,
        enum_field(
            || telar::t!("settings.field.transition"),
            transition,
            TRANSITIONS,
            theme,
        )?,
        text_field(
            || telar::t!("settings.field.transition_ms"),
            transition_ms,
            "600",
            theme,
        )?,
    ];

    // One row per screen, which is what makes a map-valued section editable without K13's generic key/value machinery: the keys are not free text here, they are the monitors that exist.
    let names = monitor_keys(&b.monitors);
    let mut monitors: Vec<(String, RwSignal<String>)> = Vec::new();
    if !names.is_empty() {
        rows.push(subheader(
            || telar::t!("settings.subheader.monitors"),
            theme,
        )?);
    }
    for name in names {
        let value = signal(
            b.monitors
                .get(&name)
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
        );
        let label = name.clone();
        rows.push(text_field(
            move || label.clone(),
            value,
            "(global image)",
            theme,
        )?);
        monitors.push((name, value));
    }

    let base = b.clone();
    let path = path.to_path_buf();
    let save = save_button(
        SaveButtonProps::props()
            .label(telar::Reactive::of(|| {
                telar::t!("settings.save.background")
            }))
            .on_press(std::rc::Rc::new(move || {
                let monitors = monitors
                    .iter()
                    .filter_map(|(name, value)| {
                        opt_string(&value.peek()).map(|path| (name.clone(), PathBuf::from(path)))
                    })
                    .collect();
                let value = BackgroundConfig {
                    enabled: enabled.peek(),
                    image: opt_string(&image.peek()).map(PathBuf::from),
                    monitors,
                    transition: WallpaperTransition::from_id(&transition.peek())
                        .unwrap_or_default(),
                    transition_ms: parse_u64(&transition_ms.peek(), base.transition_ms),
                };
                persist(&path, "background", &value);
            }))
            .build(),
        telar::Children::default(),
    )?;
    section(
        || telar::t!("settings.section.background"),
        rows,
        save,
        theme,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn the_wallpaper_browser_groups_by_folder_and_narrows_by_name() {
        use services::wallpaper::Entry;
        let entry = |name: &str, folder: &str| Entry {
            path: PathBuf::from(format!("/w/{folder}/{name}.jpg")),
            name: name.to_string(),
            folder: folder.to_string(),
        };
        let library = vec![
            entry("dune", "deserts"),
            entry("fjord", ""),
            entry("erg", "deserts"),
        ];

        let grouped = folders(&library, "");
        assert_eq!(
            grouped.iter().map(|(f, _)| f.as_str()).collect::<Vec<_>>(),
            vec!["", "deserts"],
            "the library root sorts above its sub-folders"
        );
        assert_eq!(grouped[1].1.len(), 2);

        // A folder name matches as well as an image name: "show me the deserts" is the question a grouped browser exists to answer, and typing one image's name to reach its neighbours is not an answer.
        assert_eq!(folders(&library, "deserts")[0].1.len(), 2);
        assert_eq!(folders(&library, "fjord").len(), 1);
        assert!(folders(&library, "tundra").is_empty());
    }

    #[test]
    fn a_group_is_keyed_on_which_of_its_tiles_is_current() {
        use services::wallpaper::Entry;
        let entries: Vec<Entry> = ["a", "b"]
            .iter()
            .map(|name| Entry {
                path: PathBuf::from(format!("/w/{name}.jpg")),
                name: name.to_string(),
                folder: String::new(),
            })
            .collect();
        // Choosing a picture has to move the ring, and nothing else about the group changed when it did.
        let none = group_key("", &entries, None);
        let first = group_key("", &entries, Some(Path::new("/w/a.jpg")));
        let second = group_key("", &entries, Some(Path::new("/w/b.jpg")));
        assert_ne!(none, first);
        assert_ne!(first, second);
        assert_eq!(first, group_key("", &entries, Some(Path::new("/w/a.jpg"))));
    }
}
