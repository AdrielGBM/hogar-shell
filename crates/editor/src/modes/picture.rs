//! What a wallpaper region's popover sets about its picture beyond its fit and transition (T-7.5): the picture itself, picked from the wallpaper library as the launcher and the settings browse it; the point `cover` keeps in view, also dragged as a dot on the region; how the picture dims and blurs under windows; and how far it slides across the workspaces.

use telar::{
    AlignItems, Border, Color, Cursor, Key, LayoutError, LayoutItem, LayoutStyle, NamedKey,
    ReactiveList, Rect, RectStyle, Role, RwSignal, SizeDimension, StyledContainer, Text,
    Transaction, box_item, effect, signal,
};

use config::theme::{FontRole, NordTheme};
use layout::{AreaKind, Fit, Focus, ResolvedArea, ResolvedAreaKind};
use services::wallpaper::Entry;
use ui::descriptor::Built;

use crate::popover::rows::{self, Range, Rows, label};
use crate::popover::{AreaDraft, help, kind_field};

use super::gesture;

/// How wide one picture of the library is in the picker.
const TILE: f32 = 96.0;
/// How many pictures the picker draws at once; the search reaches the rest.
const TILES: usize = 24;
/// How big the focus dot is across.
const DOT: f32 = 18.0;
/// How far one arrow moves the focus, as a fraction of the picture.
pub(crate) const FOCUS_STEP: f32 = 0.05;

/// The region's picture: the library as thumbnails narrowed by a search, a press on one picking it; any path typed; or none of its own, following `[background]`.
pub(crate) fn picture_rows(draft: &AreaDraft, picture: RwSignal<String>) -> Built {
    let query = signal(String::new());
    let library = signal(services::wallpaper::all());
    platform_wayland::watch(services::wallpaper::subscribe_library, move |entries| {
        library.set(entries)
    });
    let theme = telar::use_theme::<NordTheme>();
    let tiles = ReactiveList::with_style(
        LayoutStyle::new()
            .flex_row()
            .flex_wrap()
            .gap(ui::scale::space::sm())
            .width(SizeDimension::Percent(1.0)),
        move || narrowed(&library.get(), &query.get()),
        |entry: &Entry| entry.path.clone(),
        move |entry: Entry| tile(entry, picture, theme),
    )?;
    let empty = rows::note(move || {
        match (
            library.with(Vec::is_empty),
            narrowed(&library.get(), &query.get()).is_empty(),
        ) {
            (true, _) => telar::t!("editor.region.library_empty"),
            (false, true) => telar::t!("editor.region.no_match"),
            (false, false) => String::new(),
        }
    })?;
    draft.marked(
        &["source"],
        rows::together(vec![
            rows::captioned(
                label!("editor.area.source"),
                help("AreaKind::WallpaperRegion", "source"),
                rows::together(vec![
                    rows::text(label!("editor.region.search"), None, query)?,
                    Box::new(tiles),
                    empty,
                ])?,
            )?,
            rows::text(
                label!("editor.region.path"),
                help("AreaKind::WallpaperRegion", "source"),
                picture,
            )?,
            rows::action(
                || telar::t!("editor.region.follow"),
                move || {
                    if picture.peek_with(|now| !now.is_empty()) {
                        picture.set(String::new());
                    }
                },
            )?,
        ])?,
    )
}

/// The library's pictures whose name or folder holds `query`, at most [`TILES`] of them.
pub(crate) fn narrowed(entries: &[Entry], query: &str) -> Vec<Entry> {
    let needle = query.trim().to_lowercase();
    entries
        .iter()
        .filter(|entry| {
            needle.is_empty()
                || entry.name.to_lowercase().contains(&needle)
                || entry.folder.to_lowercase().contains(&needle)
        })
        .take(TILES)
        .cloned()
        .collect()
}

/// One picture of the library: its thumbnail over its name, ringed while it is the region's, and picked by a press or by Enter while it has the focus.
fn tile(entry: Entry, picture: RwSignal<String>, theme: NordTheme) -> Built {
    let thumbnail = ui::thumbnail::view(
        entry.path.clone(),
        TILE,
        (TILE * 9.0 / 16.0).round(),
        6.0,
        "image",
        theme,
    )?;
    let name = entry.name.clone();
    let caption = Text::new(
        move || name.clone(),
        LayoutStyle::new().width(SizeDimension::Percent(1.0)),
        move || {
            theme
                .text_style(FontRole::Caption, theme.subtle)
                .with_clamp(1, true)
        },
    )?;
    let path = entry.path.display().to_string();
    let chosen = path.clone();
    let corner = ui::scale::corner::md();
    Ok(Box::new(
        StyledContainer::new(
            LayoutStyle::new()
                .flex_column()
                .gap(ui::scale::space::xs())
                .width(TILE + 8.0)
                .padding_all(ui::scale::space::xs())
                .align_items(AlignItems::CENTER),
            move |_| {
                let ring = match picture.with(|now| *now == chosen) {
                    true => theme.accent,
                    false => Color::TRANSPARENT,
                };
                RectStyle::filled(Color::TRANSPARENT, corner)
                    .with_border(Border::uniform(ring, 2.0))
            },
            vec![thumbnail, box_item(caption)],
        )?
        .control(Role::Button)
        .cursor(Cursor::Pointer)
        .on_press(move || {
            if picture.peek_with(|now| *now != path) {
                picture.set(path.clone());
            }
        }),
    ))
}

/// The point `cover` keeps in view, as two rows and the dot on the region they share ([`focus_handle`]).
pub(crate) fn focus_rows(draft: &AreaDraft, focus: RwSignal<Focus>) -> Built {
    let across = signal(focus.peek().x);
    let down = signal(focus.peek().y);
    follow(across, focus, move |x| Focus {
        x: *x,
        ..focus.peek()
    });
    follow(down, focus, move |y| Focus {
        y: *y,
        ..focus.peek()
    });
    follow(focus, across, |focus| focus.x);
    follow(focus, down, |focus| focus.y);
    let fraction = Range::new(0.0, 1.0, 0.01);
    draft.marked(
        &["focus"],
        rows::together(vec![
            rows::number(
                label!("editor.region.focus_x"),
                help("AreaKind::WallpaperRegion", "focus"),
                across,
                fraction,
            )?,
            rows::number(
                label!("editor.region.focus_y"),
                help("AreaKind::WallpaperRegion", "focus"),
                down,
                fraction,
            )?,
        ])?,
    )
}

pub(crate) fn focus_of(draft: &AreaDraft) -> RwSignal<Focus> {
    draft.setting(
        "focus",
        "focus",
        |area: &ResolvedArea| match area.kind {
            ResolvedAreaKind::WallpaperRegion { focus, .. } => focus,
            _ => Focus::MIDDLE,
        },
        |area, focus: &Focus| {
            kind_field!(
                area,
                "wallpaper_region",
                WallpaperRegion { focus },
                focus.clamped()
            )
        },
    )
}

/// How the picture dims and blurs while a window covers the screen, and how far it slides across the workspaces.
pub(crate) fn under_windows_rows(draft: &AreaDraft) -> Rows {
    let dim = draft.setting(
        "dim",
        "dim",
        |area: &ResolvedArea| match area.kind {
            ResolvedAreaKind::WallpaperRegion { dim, .. } => dim,
            _ => 0.0,
        },
        |area, dim: &f32| kind_field!(area, "wallpaper_region", WallpaperRegion { dim }, *dim),
    );
    let blur = draft.setting(
        "blur",
        "blur",
        |area: &ResolvedArea| match area.kind {
            ResolvedAreaKind::WallpaperRegion { blur, .. } => blur,
            _ => 0.0,
        },
        |area, blur: &f32| kind_field!(area, "wallpaper_region", WallpaperRegion { blur }, *blur),
    );
    let parallax = draft.setting(
        "parallax",
        "parallax",
        |area: &ResolvedArea| match area.kind {
            ResolvedAreaKind::WallpaperRegion { parallax, .. } => parallax,
            _ => 0.0,
        },
        |area, parallax: &f32| {
            kind_field!(
                area,
                "wallpaper_region",
                WallpaperRegion { parallax },
                *parallax
            )
        },
    );
    Ok(vec![
        rows::heading(|| telar::t!("editor.region.under_windows"))?,
        draft.marked(
            &["dim"],
            rows::number(
                label!("editor.region.dim"),
                help("AreaKind::WallpaperRegion", "dim"),
                dim,
                Range::new(0.0, 1.0, 0.05),
            )?,
        )?,
        draft.marked(
            &["blur"],
            rows::number(
                label!("editor.region.blur"),
                help("AreaKind::WallpaperRegion", "blur"),
                blur,
                Range::new(0.0, AreaKind::MOST_BLUR, 1.0),
            )?,
        )?,
        draft.marked(
            &["parallax"],
            rows::number(
                label!("editor.region.parallax"),
                help("AreaKind::WallpaperRegion", "parallax"),
                parallax,
                Range::new(0.0, AreaKind::MOST_PARALLAX, 0.01),
            )?,
        )?,
    ])
}

/// Where the dot for `focus` sits on a region drawn at `rect`: as far across and down the region as the focus is across and down the picture.
pub(crate) fn dot_at(rect: Rect, focus: Focus) -> (f32, f32) {
    let focus = focus.clamped();
    (
        rect.x + focus.x * rect.width,
        rect.y + focus.y * rect.height,
    )
}

/// The focus a dot dragged to `point` over a region drawn at `rect` sets.
pub(crate) fn focus_at(rect: Rect, (x, y): (f32, f32)) -> Focus {
    Focus {
        x: (x - rect.x) / rect.width.max(1.0),
        y: (y - rect.y) / rect.height.max(1.0),
    }
    .clamped()
}

/// The focus moved one arrow press `key` says, a step scaled by the modifiers held; `None` for any other key.
pub(crate) fn stepped(focus: Focus, key: &Key) -> Option<Focus> {
    let step = FOCUS_STEP * telar::step_factor(telar::modifiers());
    let (x, y) = match key {
        Key::Named(NamedKey::ArrowLeft) => (-step, 0.0),
        Key::Named(NamedKey::ArrowRight) => (step, 0.0),
        Key::Named(NamedKey::ArrowUp) => (0.0, -step),
        Key::Named(NamedKey::ArrowDown) => (0.0, step),
        _ => return None,
    };
    Some(
        Focus {
            x: focus.x + x,
            y: focus.y + y,
        }
        .clamped(),
    )
}

/// The dot on the region that sets the focus, laid over it while the picture is cropped by `cover`: dragged, it follows the pointer, and focused, the arrows move it a step. A drag let go keeps where it is and Esc puts it back.
pub(crate) fn focus_handle(
    draft: &AreaDraft,
    focus: RwSignal<Focus>,
    fit: Option<RwSignal<String>>,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let covers = move || {
        fit.is_none_or(|fit| {
            fit.with(|now| crate::popover::area::parsed::<Fit>(now) == Some(Fit::Cover))
        })
    };
    let draft = draft.clone();
    crate::host::see_through(ReactiveList::with_style(
        crate::host::whole(),
        move || match covers() {
            true => vec![()],
            false => Vec::new(),
        },
        |_: &()| (),
        move |_| dot(&draft, focus),
    )?)
}

fn dot(draft: &AreaDraft, focus: RwSignal<Focus>) -> Built {
    let theme = telar::use_theme::<NordTheme>();
    let (placing, dragging) = (draft.clone(), draft.clone());
    let where_now = move || placing.rect().map(|rect| dot_at(rect, focus.get()));
    let grip = draft.grip();
    let (keeping, dropping, holding) = (grip.clone(), grip.clone(), grip);
    let transaction = Transaction::new(focus)
        .on_commit(move |_, _| keeping.release())
        .on_revert(move |_| dropping.put_back());
    let face = StyledContainer::new(
        LayoutStyle::new()
            .absolute()
            .inset_start(0.0)
            .inset_top(0.0)
            .width(DOT)
            .height(DOT),
        move |_| {
            RectStyle::filled(theme.accent, DOT / 2.0)
                .with_border(Border::uniform(Color::WHITE, 2.0))
        },
        Vec::new(),
    )?;
    let placed = where_now.clone();
    let handle = crate::host::centred(face, DOT, placed)
        .control(Role::Slider)
        .cursor(Cursor::Grab)
        .on_focused_key(move |key: &Key| -> bool {
            let Some(next) = stepped(focus.peek(), key) else {
                return false;
            };
            if focus.peek() != next {
                focus.set(next);
            }
            true
        });
    Ok(Box::new(gesture::drag(
        handle,
        transaction,
        move |_| where_now().map(|(x, y)| (x - DOT / 2.0, y - DOT / 2.0)),
        move |origin: &(f32, f32), (x, y)| {
            holding.hold();
            let Some(rect) = dragging.rect() else {
                return;
            };
            let moved = focus_at(rect, (origin.0 + x, origin.1 + y));
            if focus.peek() != moved {
                let _ = transaction.preview(|now| *now = moved);
            }
        },
        |_, _| {},
    )))
}

fn follow<S: Clone + PartialEq + 'static, T: Clone + PartialEq + 'static>(
    source: RwSignal<S>,
    to: RwSignal<T>,
    from: impl Fn(&S) -> T + 'static,
) {
    effect(move || {
        let next = from(&source.get());
        if to.peek_with(|now| *now != next) {
            to.set(next);
        }
    });
}
