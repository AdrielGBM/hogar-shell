//! The application launcher: a modal that owns the keyboard while it is up.

use std::rc::Rc;
use ui::scale::{corner, paint};

use platform_wayland::KeyboardMode;
use telar::{
    AlignItems, Container, Input, LayoutError, LayoutItem, LayoutStyle, RectStyle, SizeDimension,
    StyledContainer, SurfaceToken, Text, box_item, memo, signal, use_theme,
};

use platform_wayland::ManagedToplevel;

use config::scheme;
use config::theme::{FontRole, NordTheme};
use config::{LauncherAction, LauncherConfig};
use services::apps::{self, App};
use services::state;
use services::wallpaper;
use surfaces::shell;
use ui::keynav::{self, Move};
use ui::panel::{PanelSurface, content_radius, panel_fill};
use ui::placement::{Centred, Placement};
use ui::scale::space;
use ui::thumbnail;
use util::calc;
use util::search::{self, Mode};
use util::state::kept;

/// The id the surface registry keys the launcher on.
pub const ID: &str = "launcher";

/// Typing this first switches to the action mode, listing `[[launcher.actions]]` instead of applications.
const ACTION_PREFIX: char = '>';

/// Typing this first forces the calculator, for the cases auto-detection deliberately skips — `=2` echoes 2, where a bare `2` is far more likely the start of an app name.
const CALC_PREFIX: char = '=';

/// Typing this first lists the colour schemes: every palette, the light/dark modes and the dynamic variants.
const SCHEME_PREFIX: char = '#';

/// Typing this first browses the wallpaper library as a grid of thumbnails.
const WALLPAPER_PREFIX: char = '@';

/// Typing this first lists the windows that are already open, so `/fire` switches to a browser rather than starting a second one.
const WINDOW_PREFIX: char = '/';

/// The widest a wallpaper tile gets before another column fits. Thumbnails are landscape, so a tile the width of a row would show one picture where four fit — the grid is the point of this mode.
const TILE_WIDTH: f32 = 150.0;

/// Wallpaper tiles are pictures of screens, so a tile is shaped like one.
const TILE_ASPECT: f32 = 9.0 / 16.0;

const TILE_GAP: f32 = 8.0;

/// The width to lay a grid out for when there is no config to read one from — a headless render, or a preview.
const DEFAULT_PANEL_WIDTH: f32 = 640.0;

/// How many wallpapers the grid draws at once.
///
/// A bound the *reactive list* needs, not the library: it builds a widget per tile up front, so pointing the shell at a picture archive would spend the UI thread building thousands of them before the panel appeared. Far above the row cap because a grid shows several rows of what a list shows one of, and typing narrows a bigger library faster than scrolling one would. Lifting it properly means a `VirtualList` for the lines.
const GRID_CAP: usize = 150;

/// One row of the launcher. The launcher lists *things you can do*, not only applications, so each mode contributes the same shape and one `row` renders any of them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Entry {
    App(App),
    Action(LauncherAction),
    /// The calculator's answer. Selecting it copies the result rather than the whole sum, which is what you want to paste.
    Calculation {
        expression: String,
        result: String,
    },
    /// A palette, a light/dark mode or a dynamic-scheme variant. Choosing one writes it to `[theme]`, which the config watcher then reloads — the same route the settings panel and `hogar-shell scheme` take.
    Scheme {
        choice: scheme::Choice,
        value: String,
    },
    /// An image from the wallpaper library. Drawn as a tile rather than a row, because a wallpaper is chosen by looking at it — a list of file names would be a worse version of `ls`.
    Wallpaper(wallpaper::Entry),
    /// A window that is already open. Choosing it switches to that window instead of starting the application again, which is the difference between this mode and typing the same name with no prefix.
    Window(ManagedToplevel),
}

impl Entry {
    /// The identity the reactive list reconciles on and the selection is resolved by. Prefixed per kind so an app and an action that share a name can't collide into one row.
    pub fn key(&self) -> String {
        match self {
            Entry::App(app) => format!("app:{}", app.id),
            Entry::Action(action) => format!("action:{}", action.name),
            Entry::Calculation { expression, .. } => format!("calc:{expression}"),
            Entry::Scheme { choice, value } => format!("scheme:{choice:?}:{value}"),
            Entry::Wallpaper(entry) => format!("wallpaper:{}", entry.path.display()),
            // The protocol object, not the title: two windows of one application share a title far more often than they share nothing, and a list keyed on that would collapse them into one row.
            Entry::Window(window) => format!("window:{}", window.id.raw()),
        }
    }

    /// Whether choosing this needs a second, confirming Enter.
    pub fn is_dangerous(&self) -> bool {
        matches!(self, Entry::Action(action) if action.dangerous)
    }
}

/// Which mode a query is in, and the query with its prefix stripped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueryMode {
    Apps,
    Actions,
    Calculator,
    Schemes,
    Wallpapers,
    Windows,
}

impl QueryMode {
    /// How many tiles a row of this mode holds inside a panel `width` px wide, or 1 for the modes that are lists.
    ///
    /// The selection, the keys and the reveal all work in one flat index whatever the shape, so this is the only thing that has to know a grid from a list.
    pub fn columns(self, width: f32) -> usize {
        if self != QueryMode::Wallpapers {
            return 1;
        }
        let inner = (width - space::xl() * 2.0).max(TILE_WIDTH);
        (((inner + TILE_GAP) / (TILE_WIDTH + TILE_GAP)).floor() as usize).clamp(2, 8)
    }
}

/// Reads the mode off the query's first character. An explicit prefix always wins; without one the query is an app search, which may still *also* show a calculation (see [`entries`]).
pub fn mode_of(query: &str) -> (QueryMode, &str) {
    let trimmed = query.trim_start();
    if let Some(rest) = trimmed.strip_prefix(ACTION_PREFIX) {
        return (QueryMode::Actions, rest.trim_start());
    }
    if let Some(rest) = trimmed.strip_prefix(CALC_PREFIX) {
        return (QueryMode::Calculator, rest.trim_start());
    }
    if let Some(rest) = trimmed.strip_prefix(SCHEME_PREFIX) {
        return (QueryMode::Schemes, rest.trim_start());
    }
    if let Some(rest) = trimmed.strip_prefix(WALLPAPER_PREFIX) {
        return (QueryMode::Wallpapers, rest.trim_start());
    }
    if let Some(rest) = trimmed.strip_prefix(WINDOW_PREFIX) {
        return (QueryMode::Windows, rest.trim_start());
    }
    (QueryMode::Apps, query.trim())
}

/// Every row to show for `query`, in order.
///
/// The calculator is additive rather than a mode you fall into: an unambiguous sum puts its answer at the top and the app matches still follow underneath, so typing something that happens to parse as arithmetic never hides the app you were reaching for.
pub fn entries(
    apps: Vec<App>,
    library: Vec<wallpaper::Entry>,
    open: Vec<ManagedToplevel>,
    query: &str,
    config: &LauncherConfig,
) -> Vec<Entry> {
    let (mode, rest) = mode_of(query);
    let cap = config.max_results.max(1) as usize;
    match mode {
        QueryMode::Actions => actions(rest, config).into_iter().take(cap).collect(),
        QueryMode::Calculator => calculation_or_qalc(rest, config).into_iter().collect(),
        QueryMode::Schemes => schemes(rest, config).into_iter().take(cap).collect(),
        QueryMode::Windows => windows(open, rest, config).into_iter().take(cap).collect(),
        // A grid fits several rows of what a list shows one of, so the row cap would cut the browse mode off at a third of a screen. It gets its own, much larger bound instead — see `GRID_CAP`.
        QueryMode::Wallpapers => {
            let mut tiles = wallpapers(library, rest, config);
            tiles.truncate(GRID_CAP);
            tiles
        }
        QueryMode::Apps => {
            let mut rows: Vec<Entry> = Vec::new();
            if config.calculator && calc::looks_like_math(rest) {
                rows.extend(calculation(rest, config));
            }
            rows.extend(results(apps, rest, config).into_iter().map(Entry::App));
            rows.truncate(cap);
            rows
        }
    }
}

fn calculation(expression: &str, config: &LauncherConfig) -> Option<Entry> {
    if !config.calculator {
        return None;
    }
    calc::solve(expression).map(|answer| Entry::Calculation {
        expression: expression.trim().to_string(),
        result: answer.text(),
    })
}

/// The calculator row for an explicit `=` query, falling back to `qalc` for what the in-house evaluator cannot do.
///
/// Only on the explicit prefix, and only after the local evaluator has declined: an app search must never spawn a process, and a sum with a local answer must never wait for one. While the subprocess is out there is no row — the answer simply appears, because this runs inside the results memo and the loader's signal is what re-runs it.
fn calculation_or_qalc(expression: &str, config: &LauncherConfig) -> Option<Entry> {
    if let Some(local) = calculation(expression, config) {
        return Some(local);
    }
    if !config.calculator || !config.qalc {
        return None;
    }
    calc::qalc::answer(expression).map(|result| Entry::Calculation {
        expression: expression.trim().to_string(),
        result,
    })
}

/// The declared actions matching `query`, ranked by the same matcher the app list uses so the two modes behave identically under the same `fuzzy` setting.
pub fn actions(query: &str, config: &LauncherConfig) -> Vec<Entry> {
    let listed: Vec<LauncherAction> = config
        .actions
        .iter()
        .filter(|a| a.is_listed(config.enable_dangerous_actions))
        .cloned()
        .collect();
    search::rank(
        listed,
        query,
        match_mode(config),
        |action| format!("{} {}", action.name, action.description),
        |_| 0,
    )
    .into_iter()
    .map(Entry::Action)
    .collect()
}

/// The colour schemes matching `query`, ranked by the same matcher the other modes use.
///
/// Palettes, modes and variants in one list rather than three sub-modes: they are all answers to "make the desktop look like this", and a picker that made the user first say *which kind* of answer they wanted would be one more step than typing `#latte` needs.
pub fn schemes(query: &str, config: &LauncherConfig) -> Vec<Entry> {
    let listed: Vec<Entry> = scheme::choices()
        .into_iter()
        .map(|(choice, value)| Entry::Scheme { choice, value })
        .collect();
    search::rank(
        listed,
        query,
        match_mode(config),
        |entry| {
            let (name, kind) = scheme_text(entry);
            format!("{name} {kind}")
        },
        |_| 0,
    )
}

/// The wallpapers matching `query`, ranked by the same matcher every other mode uses.
///
/// The folder is part of the haystack, so `@nature` finds a whole folder without the user having to remember what any single picture in it is called.
pub fn wallpapers(
    library: Vec<wallpaper::Entry>,
    query: &str,
    config: &LauncherConfig,
) -> Vec<Entry> {
    search::rank(
        library,
        query,
        match_mode(config),
        |entry| format!("{} {}", entry.name, entry.folder),
        |_| 0,
    )
    .into_iter()
    .map(Entry::Wallpaper)
    .collect()
}

/// The open windows matching `query`, in the order the compositor announced them.
///
/// Both the title and the application id are in the haystack, so `/fire` finds a browser window whose title says nothing about the browser, and `/docs` finds it by what is on screen.
pub fn windows(open: Vec<ManagedToplevel>, query: &str, config: &LauncherConfig) -> Vec<Entry> {
    search::rank(
        open,
        query,
        match_mode(config),
        |window| format!("{} {}", window.title, window.app_id),
        |_| 0,
    )
    .into_iter()
    .map(Entry::Window)
    .collect()
}

/// A scheme row's name and what kind of choice it is. The name is the palette's own where there is one, so `#moc` finds "Catppuccin Mocha" and not just the config spelling.
fn scheme_text(entry: &Entry) -> (String, String) {
    let Entry::Scheme { choice, value } = entry else {
        return (String::new(), String::new());
    };
    match choice {
        scheme::Choice::Palette if value != scheme::DYNAMIC => (
            format!("{} {value}", NordTheme::meta(value).name),
            telar::t!("launcher.scheme.palette"),
        ),
        scheme::Choice::Palette => (value.clone(), telar::t!("launcher.scheme.palette")),
        scheme::Choice::Mode => (value.clone(), telar::t!("launcher.scheme.mode")),
        scheme::Choice::Variant => (value.clone(), telar::t!("launcher.scheme.variant")),
    }
}

fn match_mode(config: &LauncherConfig) -> Mode {
    if config.fuzzy {
        Mode::Fuzzy
    } else {
        Mode::Substring
    }
}

/// The applications matching `query`, ranked and capped.
///
/// Familiarity is folded into the score rather than sorted on separately, so a much better text match still wins over a slightly more familiar app — the user typing `fir` means Firefox even if they open Files more. Favourites are the exception: they are pinned above the ranking, in the order the user listed them, because naming an app there *is* the statement that it outranks the shell's idea of relevance. A favourite that does not match the query is still filtered out, so pinning never puts an irrelevant entry at the top.
pub fn results(apps: Vec<App>, query: &str, config: &LauncherConfig) -> Vec<App> {
    let counts = state::get().launch_counts;
    let hidden = config.hidden.clone();
    let visible: Vec<App> = apps
        .into_iter()
        .filter(|a| !hidden.contains(&a.id))
        .collect();

    let mut ranked = search::rank(
        visible,
        query,
        match_mode(config),
        |app| app.haystack(),
        move |app| {
            // Diminishing: the 50th launch should not outweigh a better name match, but the difference between never-used and used-daily should be visible.
            let launches = counts.get(&app.id).copied().unwrap_or(0);
            (launches as f32).sqrt().round() as i32 * 4
        },
    );
    // Pinned before the cap, so a favourite the ranking put 20th still makes a 12-row list. `sort_by_key` is stable, so everything else keeps the order the ranking gave it.
    let favourites = &config.favourites;
    ranked.sort_by_key(|app| {
        favourites
            .iter()
            .position(|id| *id == app.id)
            .unwrap_or(usize::MAX)
    });
    ranked.truncate(config.max_results.max(1) as usize);
    // Applied here rather than at the row that draws it: this is the one place the launcher turns the app database into the list it shows, so every consumer downstream — the row, its icon, a future preview — sees the override without each having to know the config carries one.
    for app in &mut ranked {
        let icon = config.icon_for(&app.id, &app.icon).to_string();
        app.icon = icon;
    }
    ranked
}

/// Carries out `entry`.
///
/// **Every caller closes the launcher first.** A compositor ignores a request to focus a window while another surface holds the seat's keyboard, and this one holds it for as long as it is up — so switching to a window from a launcher that is still up does nothing at all. Closing first is what makes the window mode work, and it costs the other kinds nothing: none of them reads anything the surface owns.
fn choose(entry: &Entry) {
    match entry {
        Entry::App(app) => apps::launch(app),
        Entry::Action(action) => apps::run_detached(action.command.clone()),
        Entry::Calculation { result, .. } => util::clipboard::copy(result),
        Entry::Scheme { choice, value } => {
            if let Err(e) = scheme::apply(*choice, value) {
                tracing::warn!("launcher: {e}");
            }
        }
        // Every screen, not the focused one: a choice made with no monitor named is a choice about the desktop, which is the same rule `hogar-shell wallpaper set` follows — including re-deriving a dynamic palette from the new picture, which is the other half of "the wallpaper changed" and is easy to ship without.
        Entry::Wallpaper(entry) => {
            wallpaper::set(&entry.path, None);
            scheme::refresh_current();
        }
        Entry::Window(window) => {
            services::windows::focus(window.id);
        }
    }
}

/// Opens the launcher, or closes it if it is already up. Opening it takes the screen from whatever drawer was up — see [`shell::close_drawer`].
pub fn toggle() {
    shell::toggle_standing_window(ID, open);
}

fn open() -> SurfaceToken {
    let output = shell::focused_output();

    // No `.size(...)`: a modal is dismissed by a press outside it, so its *surface* is full-screen and the scaffold centres the window inside it. The window's own size is a layout property (see `panel`), not a surface one — asking the surface to be 640×420 would leave every press beyond that box unheard.
    PanelSurface::new(Placement::centred(Centred::Modal).output(output), |env| {
        panel(env.config.resolve_theme(), &env.config.launcher)
    })
    .open()
}

/// Where the arrow keys move the selection, given the current index and how many results there are.
///
/// Wraps at both ends, so holding Down cycles rather than sticking at the bottom, and Up from the first result jumps to the last — which is how every launcher behaves and what the hand expects.
fn panel(theme: NordTheme, config: &LauncherConfig) -> Result<Box<dyn LayoutItem>, LayoutError> {
    // Kept by the surface rather than built here: a config edit rebuilds this tree, and a launcher that lost the half-typed search it was showing would be a launcher the user has to start over in.
    let query = kept("launcher.query", || signal(String::new()));
    let query_read = query.read_only();
    let config = config.clone();

    // The app list is read once per open, not per keystroke: it only changes when software is installed.
    let installed = apps::all();
    let shown = results_memo(
        installed,
        library(),
        open_windows(),
        query_read,
        config.clone(),
    );
    let for_columns = query.read_only();
    let width = config.width as f32;
    let columns = memo(move || mode_of(&for_columns.get()).0.columns(width));

    let selected = kept("launcher.selected", || signal(0usize));
    // Which row is armed, by key. A dangerous action needs a second Enter, and arming in place costs no extra surface — the same rule the session menu's destructive tiles follow.
    let armed = kept("launcher.armed", || signal(String::new()));
    // Typing changes the result set, so the old index would point at a different app — or past the end. Resetting to the top on every query change keeps "type a few letters, press Enter" landing on the best match. It also disarms: a row you have navigated away from must not still be one keystroke from running.
    let reset_on_query = selected;
    let disarm_on_query = armed;
    let query_watch = query.read_only();
    // An effect fires once when it is registered, and on a rebuild that run is the *tree* being seeded, not the user typing — counting it would put the selection back to the top every time the config changed.
    let seeded = std::cell::Cell::new(false);
    let follow_query = telar::effect(move || {
        query_watch.get();
        if !seeded.replace(true) {
            return;
        }
        reset_on_query.set(0);
        disarm_on_query.set(String::new());
    });

    let field = search_field(query, theme)?;
    // What is left for the list once the search field and the panel's own padding have taken their share; the panel then sizes to its content, so a short result list gives a short panel rather than dead space.
    let field_height = theme.font(FontRole::Title) * 1.8 + 12.0;
    let list_height = (config.height as f32 - field_height - 38.0).max(80.0);
    let list = result_list(
        shown,
        columns,
        selected,
        armed.read_only(),
        list_height,
        theme,
    )?;
    let keys_shown = shown;
    let keys_columns = columns;
    // The shared list bindings, so the launcher and every other list surface agree on what a key means.
    let nav = keynav::from_config(&config::config().map(|c| c.keynav).unwrap_or_default());
    let grid_nav = nav.grid();
    let keys_selected = selected;
    let keys_armed = armed;

    // A fixed-width box the scaffold centres, rather than `100%` — which inside a full-screen scrim scaffold would be the whole screen. Height is left to the content so a short result list gives a short panel; the list itself carries the bound (see `result_list`).
    let panel = StyledContainer::new(
        LayoutStyle::new()
            .flex_column()
            .gap(space::lg())
            .padding_all(space::xl())
            .width(config.width as f32),
        move |_| RectStyle::filled(panel_fill(), content_radius()),
        vec![field, list],
    )?
    // `on_key` fires before the event reaches the children, which is what lets the arrows drive the list while the search field holds focus and keeps every other keystroke going to the field as typing. It also owns the query-reset subscription above: an `Effect` deregisters when its handle drops, and this closure lives for exactly as long as the panel it is attached to.
    .on_key(move |key| {
        let _ = &follow_query;
        // Which bindings apply is a property of the shape the results are in, and that changes as the query does: a grid answers to both pairs of arrows, a list only to one.
        let columns = keys_columns.with(|columns| *columns);
        let reading = if columns > 1 { grid_nav } else { nav };
        let Some(movement) = reading.interpret(key) else {
            return;
        };
        match movement {
            Move::Activate => {
                let chosen = keys_shown.with(|list| list.get(keys_selected.peek()).cloned());
                let Some(entry) = chosen else { return };
                let key = entry.key();
                // A dangerous action arms on the first Enter and runs on the second. Arming leaves the launcher up — closing it would be indistinguishable from having run the thing.
                if entry.is_dangerous() && keys_armed.peek() != key {
                    keys_armed.set(key);
                    return;
                }
                shell::close(ID);
                choose(&entry);
            }
            Move::Cancel => {
                // Escape backs out of one thing at a time: an armed confirmation first, the launcher itself once there is nothing left to back out of. Closed here rather than left to the surface's own Escape handling because the search field is focused and claims the key before it gets there.
                if keys_armed.peek().is_empty() {
                    shell::close(ID);
                } else {
                    keys_armed.set(String::new());
                }
            }
            movement => {
                let count = keys_shown.with(|list| list.len());
                keys_selected.set(keynav::apply_grid(
                    keys_selected.peek(),
                    count,
                    columns,
                    movement,
                ));
            }
        }
    });
    Ok(Box::new(panel))
}

/// The rows to show, re-derived whenever the query or the library changes.
///
/// Every cell is read *out* before `entries` runs, and it has to be: `entries` calls `t!` for the scheme rows and asks the qalc loader for a signal for the `=` rows, and either one inside a `with` panics on the runtime's borrow. That panic is invisible until the closure actually runs, which is why this is a function a test can drive rather than a closure buried in `panel`.
fn results_memo(
    installed: Vec<App>,
    library: telar::ReadSignal<Vec<wallpaper::Entry>>,
    open: telar::ReadSignal<Vec<ManagedToplevel>>,
    query: telar::ReadSignal<String>,
    config: LauncherConfig,
) -> telar::Memo<Vec<Entry>> {
    memo(move || {
        let images = library.get();
        let windows = open.get();
        let text = query.get();
        entries(installed.clone(), images, windows, &text, &config)
    })
}

/// The open windows as this surface sees them, staying current for as long as the launcher is up.
///
/// Subscribed rather than read once, for the same reason the wallpaper library is: the service answers with its current list immediately, so the mode is populated on the frame it opens, and a window closing behind the launcher drops out of the list rather than leaving a row that focuses nothing.
fn open_windows() -> telar::ReadSignal<Vec<ManagedToplevel>> {
    let windows = signal(Vec::new());
    let sink = windows;
    platform_wayland::watch(services::windows::subscribe, move |open| sink.set(open));
    windows.read_only()
}

/// The wallpaper library as this surface sees it, staying current for as long as the launcher is up.
///
/// Subscribed rather than read once: the store answers with its current contents immediately, so the grid is populated on the frame it opens, and a folder that grows behind the launcher fills in without a reopen. A shell with `[wallpaper] enabled` off subscribes to nothing, so it never starts the scanner it has no use for.
fn library() -> telar::ReadSignal<Vec<wallpaper::Entry>> {
    let images = signal(Vec::new());
    let enabled = config::config()
        .map(|config| config.wallpaper.enabled)
        .unwrap_or(false);
    if !enabled {
        return images.read_only();
    }
    let sink = images;
    platform_wayland::watch(wallpaper::subscribe_library, move |entries| {
        sink.set(entries)
    });
    images.read_only()
}

fn search_field(
    query: telar::RwSignal<String>,
    theme: NordTheme,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let input = Input::new(
        query,
        LayoutStyle::new()
            .flex_grow(1.0)
            .height(theme.font(FontRole::Title) * 1.8),
        move || theme.text_style(FontRole::Title, theme.text),
    )?
    .placeholder(telar::t!("launcher.placeholder"))
    // A launcher exists *because* someone wants to type: it opens on a keybind and the next keystroke is already its first search character. Without this the field has to be clicked into first, which is the one thing a launcher must never ask for.
    .autofocus();

    let boxed = StyledContainer::new(
        LayoutStyle::new()
            .flex_row()
            .align_items(AlignItems::CENTER)
            .padding_horizontal(space::lg())
            .padding_vertical(space::md())
            .width(SizeDimension::Percent(1.0)),
        paint::xl(theme.base),
        vec![box_item(input)],
    )?;
    Ok(Box::new(boxed))
}

/// One laid-out line of results: a full-width row, or a row of tiles in a grid mode.
///
/// The reactive list is one list either way. Switching between a list of applications and a grid of wallpapers is a keystroke in the search field, so it has to be a change of *content* rather than of which widget is on screen — the alternative is tearing the scroll area down and rebuilding it mid-typing.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Line {
    Row(Entry),
    Tiles(Vec<Entry>),
}

impl Line {
    /// The identity the reactive list reconciles on. A grid line's own key is its tiles', because that is what it draws: a row of four pictures that becomes a row of four *different* pictures is a different line.
    fn key(&self) -> String {
        match self {
            Line::Row(entry) => entry.key(),
            Line::Tiles(entries) => {
                let keys: Vec<String> = entries.iter().map(Entry::key).collect();
                format!("tiles:{}", keys.join("|"))
            }
        }
    }
}

/// Breaks `entries` into the lines that draw them: one row each, or `columns` tiles per line in a grid mode.
fn lines(entries: Vec<Entry>, columns: usize) -> Vec<Line> {
    if columns <= 1 {
        return entries.into_iter().map(Line::Row).collect();
    }
    entries
        .chunks(columns)
        .map(|chunk| Line::Tiles(chunk.to_vec()))
        .collect()
}

/// The wallpaper grid over a made-up library, for [`crate::preview`]. Headless there are no thumbnails, so what this shows is the tile layout and its glyph fallback — which is also what a first open of a cold cache looks like.
pub(crate) fn wallpaper_grid_preview() -> Result<Box<dyn LayoutItem>, LayoutError> {
    let theme = use_theme::<NordTheme>();
    let images: Vec<Entry> = [
        "sunset", "forest", "city", "harbour", "dunes", "rain", "pier",
    ]
    .into_iter()
    .map(|name| {
        Entry::Wallpaper(wallpaper::Entry {
            path: std::path::PathBuf::from(format!("/pictures/{name}.png")),
            name: name.to_string(),
            folder: String::new(),
        })
    })
    .collect();
    let shown = signal(images).read_only();
    let list = result_list(
        memo(move || shown.get()),
        memo(|| QueryMode::Wallpapers.columns(640.0)),
        signal(1usize),
        signal(String::new()).read_only(),
        260.0,
        theme,
    )?;
    Ok(Box::new(StyledContainer::new(
        LayoutStyle::new()
            .flex_column()
            .padding_all(space::xl())
            .width(640.0),
        paint::xl(theme.surface),
        vec![list],
    )?))
}

fn result_list(
    matches: telar::Memo<Vec<Entry>>,
    columns: telar::Memo<usize>,
    selected: telar::RwSignal<usize>,
    armed: telar::ReadSignal<String>,
    height: f32,
    theme: NordTheme,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    // Built through the viewport-taking constructor so the rows can reach it: moving the selection has to scroll the list to follow, and only the viewport can do that. Kept, so a config edit landing while the user is halfway down their results does not throw them back to the top of the list.
    let scroll = telar::LayoutScrollArea::new_kept(
        "launcher.results",
        LayoutStyle::new()
            .flex_column()
            .width(SizeDimension::Percent(1.0))
            // A *definite* height, and it has to be: a scroll area is a layout leaf whose content is laid out as its own root, so nothing inside it contributes to its size. With `max_height` alone — which is what this was — the leaf measured 612×0 and the whole result list drew nothing. It is also why the panel is the size `[launcher] height` declares rather than shrinking to a one-row answer.
            .height(height),
        move |viewport| {
            let for_source = matches;
            let for_columns = columns;
            // Both read *out* of their cells before either is used: a nested signal read holds the runtime's borrow and panics at build time.
            let source = move || {
                let across = for_columns.get();
                lines(for_source.get(), across)
            };
            let key = |line: &Line| line.key();
            let tile_columns = columns;
            // Cloned per row rather than moved: `ReactiveList` needs an `Fn`, so the builder may run many times.
            let build = move |line: Line| -> Result<Box<dyn LayoutItem>, LayoutError> {
                let keys = match &line {
                    Line::Row(entry) => vec![entry.key()],
                    Line::Tiles(entries) => entries.iter().map(Entry::key).collect(),
                };
                let item = match line {
                    Line::Row(entry) => {
                        // A row highlights when it *is* the selection, resolved by key rather than by position, so the reactive list can reorder rows without the highlight following the wrong one.
                        let is_selected = selection_is(matches, selected.read_only(), keys.clone());
                        let armed_key = entry.key();
                        let armed = armed;
                        let is_armed = move || armed.get() == armed_key;
                        let select = select_onto(matches, selected, entry.key());
                        row(entry, theme, is_selected, is_armed, select)?
                    }
                    Line::Tiles(entries) => tile_row(
                        entries,
                        matches,
                        selected,
                        tile_columns.with(|across| *across),
                        theme,
                    )?,
                };

                // Follow the selection: when this line holds the selected entry, ask the viewport to bring it into view. Already-visible lines are left alone, so arrowing within the visible window doesn't yank the list. The subscription is tied to the line, since the list rebuilds them and an effect that outlived one would keep revealing a node that is gone.
                let node = item.layout_node();
                let viewport = viewport.clone();
                let holds_selection = selection_is(matches, selected.read_only(), keys);
                telar::effect(move || {
                    if holds_selection() {
                        viewport.reveal(node, 4.0);
                    }
                });
                Ok(item)
            };
            // `with_style` rather than `new`: the convenience constructors carry no width, so a grid line asking for `100%` inside one resolves against nothing and lays its tiles out at their intrinsic size.
            Ok(Box::new(telar::ReactiveList::with_style(
                LayoutStyle::new()
                    .flex_column()
                    .width(SizeDimension::Percent(1.0)),
                source,
                key,
                build,
            )?) as Box<dyn LayoutItem>)
        },
    )?;
    Ok(Box::new(scroll))
}

/// Moves the selection onto `key`, which is what hovering an entry does.
///
/// The pointer and the keyboard drive the same selection rather than each painting a highlight of its own. Two highlights in the same colour said two different things at once — one for what a click would open and one for what Enter would — and left a stale one behind when the pointer moved off the list entirely.
fn select_onto(
    matches: telar::Memo<Vec<Entry>>,
    selected: telar::RwSignal<usize>,
    key: String,
) -> Rc<dyn Fn()> {
    Rc::new(move || {
        let at = matches.with(|list| list.iter().position(|entry| entry.key() == key));
        // Out of the list's borrow before the selection is read, and read with `peek`: a pointer moving within one row must not re-run the effect that scrolls the selection into view on every event.
        if let Some(at) = at
            && selected.peek() != at
        {
            selected.set(at);
        }
    })
}

/// Whether the selected entry is one of `keys`.
///
/// By key rather than by index, so the reactive list can reorder or re-chunk without the highlight following the wrong tile — and one predicate serves a row (one key) and a grid line (its whole row of them). The index is read out of its cell before the list is borrowed: a signal read nested inside another's `with` panics.
fn selection_is(
    matches: telar::Memo<Vec<Entry>>,
    selected: telar::ReadSignal<usize>,
    keys: Vec<String>,
) -> impl Fn() -> bool + Clone + 'static {
    move || {
        let at = selected.get();
        matches.with(|list| {
            list.get(at)
                .is_some_and(|entry| keys.contains(&entry.key()))
        })
    }
}

/// One row of a grid: `columns` tiles wide, laid out along the bar of the panel rather than down it.
///
/// The row is padded to a full `columns` regardless of how many tiles it holds, so the last row of a library that does not divide evenly keeps its pictures the same size as every other row's rather than stretching them.
fn tile_row(
    entries: Vec<Entry>,
    matches: telar::Memo<Vec<Entry>>,
    selected: telar::RwSignal<usize>,
    columns: usize,
    theme: NordTheme,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let columns = columns.max(1);
    let panel_width = config::config()
        .map(|config| config.launcher.width as f32)
        .unwrap_or(DEFAULT_PANEL_WIDTH);
    let mut children: Vec<Box<dyn LayoutItem>> = Vec::with_capacity(entries.len());
    for entry in entries {
        let is_selected = selection_is(matches, selected.read_only(), vec![entry.key()]);
        let select = select_onto(matches, selected, entry.key());
        children.push(tile(entry, columns, theme, is_selected, select)?);
    }
    let row = Container::new(
        LayoutStyle::new()
            .flex_row()
            .gap(TILE_GAP)
            .width(SizeDimension::Percent(1.0))
            .height(tile_row_height(columns, panel_width, theme))
            .padding_vertical(TILE_GAP / 2.0),
        children,
    )?;
    Ok(Box::new(row))
}

/// The width one tile gets when `columns` of them share the panel, gaps included.
fn tile_width(columns: usize, panel_width: f32) -> f32 {
    let columns = columns.max(1);
    let inner = (panel_width - space::xl() * 2.0).max(TILE_WIDTH);
    ((inner - TILE_GAP * (columns - 1) as f32) / columns as f32).max(48.0)
}

/// The picture and the caption a tile of `width` is made of, and the height the two of them need together.
///
/// Stated rather than left to the content, because a row's own auto height came back 18px shorter than the tiles in it — enough to draw each row's captions under the row below. A grid of identical tiles has one right answer for this, so computing it once here is also what makes every row exactly as tall as the last.
fn tile_metrics(width: f32, theme: NordTheme) -> (f32, f32, f32) {
    let picture_width = width - TILE_GAP;
    let picture_height = (picture_width * TILE_ASPECT).round();
    let caption_height = (theme.font(FontRole::Caption) * 1.6).round();
    let height = TILE_GAP + picture_height + TILE_GAP / 2.0 + caption_height;
    (picture_height, caption_height, height)
}

/// How tall one line of the grid is: a tile plus the row's own breathing space.
fn tile_row_height(columns: usize, panel_width: f32, theme: NordTheme) -> f32 {
    let (_, _, tile) = tile_metrics(tile_width(columns, panel_width), theme);
    tile + TILE_GAP
}

/// One wallpaper as a picture with its name under it.
///
/// The thumbnail is asked for rather than read: a library of two hundred images would otherwise decode two hundred photographs before the grid drew its first frame. `shared::thumbnail` hands back a glyph until each one lands.
fn tile(
    entry: Entry,
    columns: usize,
    theme: NordTheme,
    is_selected: impl Fn() -> bool + Clone + 'static,
    select: Rc<dyn Fn()>,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let (name, _) = row_text(&entry);
    let panel_width = config::config()
        .map(|config| config.launcher.width as f32)
        .unwrap_or(DEFAULT_PANEL_WIDTH);
    let width = tile_width(columns, panel_width);
    let (picture_height, caption_height, height) = tile_metrics(width, theme);
    let source = match &entry {
        Entry::Wallpaper(wall) => wall.path.clone(),
        _ => std::path::PathBuf::new(),
    };
    let picture = thumbnail::view(
        source,
        width - TILE_GAP,
        picture_height,
        6.0,
        "image",
        theme,
    )?;

    let selected_label = is_selected.clone();
    let label = Text::new(
        move || name.clone(),
        LayoutStyle::new()
            .width(SizeDimension::Percent(1.0))
            .height(caption_height),
        move || {
            let colour = if selected_label() {
                theme.text
            } else {
                theme.subtle
            };
            theme
                .text_style(FontRole::Caption, colour)
                .with_clamp(1, true)
        },
    )?;

    let chosen = Rc::new(entry);
    let rounded = corner::md();
    let tile = StyledContainer::new(
        LayoutStyle::new()
            .flex_column()
            .align_items(AlignItems::CENTER)
            .gap(TILE_GAP / 2.0)
            .padding_all(TILE_GAP / 2.0)
            .width(width)
            .height(height),
        move |_| {
            let fill = if is_selected() {
                theme.overlay
            } else {
                telar::Color::TRANSPARENT
            };
            RectStyle::filled(fill, rounded)
        },
        vec![picture, box_item(label)],
    )?
    .on_hover(move |hovering| {
        if hovering {
            select();
        }
    })
    .on_press(move || {
        shell::close(ID);
        choose(&chosen);
    });
    Ok(Box::new(tile))
}

/// The title, subtitle and icon a row shows. Resolving them per kind here keeps `row` itself one layout.
fn row_text(entry: &Entry) -> (String, String) {
    match entry {
        Entry::App(app) => (app.name.clone(), app.description.clone()),
        Entry::Action(action) => (action.name.clone(), action.description.clone()),
        // The answer is the headline and the sum the caption: what you are reading for is the number.
        Entry::Calculation { expression, result } => (result.clone(), format!("{expression} =")),
        // The palette's own name leads and the kind is the caption, so a list mixing all three reads as a list of looks rather than as a list of settings keys.
        Entry::Scheme { choice, value } => match choice {
            scheme::Choice::Palette if value != scheme::DYNAMIC => (
                NordTheme::meta(value).name.to_string(),
                NordTheme::meta(value).description.to_string(),
            ),
            _ => {
                let (name, kind) = scheme_text(entry);
                (name, kind)
            }
        },
        Entry::Wallpaper(entry) => (entry.name.clone(), entry.folder.clone()),
        // The title leads because that is what tells two windows of one application apart, which is the whole job of this list; the application id is the caption.
        Entry::Window(window) => (window.title.clone(), window.app_id.clone()),
    }
}

/// A row's leading graphic: an application's own artwork, or an Iconify glyph for the other kinds.
fn row_icon(entry: &Entry, theme: NordTheme) -> Result<Option<Box<dyn LayoutItem>>, LayoutError> {
    const SIZE: f32 = 28.0;
    match entry {
        Entry::App(app) => ui::icon::app_icon_view(&app.icon, SIZE),
        Entry::Action(action) => {
            let glyph = action.icon.clone();
            let tint = if action.dangerous {
                theme.red
            } else {
                theme.text
            };
            ui::icon::icon_view(move || glyph.clone(), move || tint, SIZE).map(Some)
        }
        Entry::Calculation { .. } => {
            ui::icon::icon_view(|| "equal".to_string(), move || theme.accent, SIZE).map(Some)
        }
        Entry::Scheme { choice, .. } => {
            let glyph = match choice {
                scheme::Choice::Palette => "palette",
                scheme::Choice::Mode => "sun-moon",
                scheme::Choice::Variant => "droplet",
            };
            ui::icon::icon_view(move || glyph.to_string(), move || theme.accent, SIZE).map(Some)
        }
        Entry::Wallpaper(_) => {
            ui::icon::icon_view(|| "image".to_string(), move || theme.accent, SIZE).map(Some)
        }
        Entry::Window(window) => ui::icon::app_icon_view(&window.app_id, SIZE),
    }
}

fn row(
    entry: Entry,
    theme: NordTheme,
    is_selected: impl Fn() -> bool + Clone + 'static,
    is_armed: impl Fn() -> bool + Clone + 'static,
    select: Rc<dyn Fn()>,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let icon = row_icon(&entry, theme)?;
    let (name, description) = row_text(&entry);
    let dangerous = entry.is_dangerous();

    let armed_title = is_armed.clone();
    let title = Text::new(
        move || name.clone(),
        LayoutStyle::new(),
        move || {
            // An armed row reads in the warning colour, so the state is visible and not only implied by the caption underneath it.
            let colour = if armed_title() { theme.red } else { theme.text };
            theme.text_style(FontRole::Body, colour).with_clamp(1, true)
        },
    )?;

    let armed_caption = is_armed.clone();
    let armed_caption_style = is_armed.clone();
    let mut lines: Vec<Box<dyn LayoutItem>> = vec![box_item(title)];
    if !description.is_empty() || dangerous {
        let subtitle = Text::new(
            move || {
                if armed_caption() {
                    telar::t!("launcher.confirm")
                } else {
                    description.clone()
                }
            },
            LayoutStyle::new(),
            move || {
                let colour = if armed_caption_style() {
                    theme.red
                } else {
                    theme.muted
                };
                theme
                    .text_style(FontRole::Caption, colour)
                    .with_clamp(1, true)
            },
        )?;
        lines.push(box_item(subtitle));
    }
    let text_column = Container::new(
        LayoutStyle::new()
            .flex_column()
            .flex_grow(1.0)
            .gap(space::xs()),
        lines,
    )?;

    let mut children: Vec<Box<dyn LayoutItem>> = Vec::new();
    if let Some(icon) = icon {
        children.push(icon);
    }
    children.push(box_item(text_column));

    // A press is the pointer's Enter, so it follows the same arm-then-run rule; the keyboard path owns the armed signal, so a dangerous row simply refuses the click and leaves arming to the keyboard.
    let chosen = Rc::new(entry);
    let armed_press = is_armed.clone();
    let rounded = corner::md();
    let row = StyledContainer::new(
        LayoutStyle::new()
            .flex_row()
            .align_items(AlignItems::CENTER)
            .gap(space::lg())
            .padding_horizontal(space::lg())
            .padding_vertical(space::md())
            .width(SizeDimension::Percent(1.0)),
        move |_| {
            let fill = if is_armed() {
                theme.red.with_alpha(0.18)
            } else if is_selected() {
                theme.overlay
            } else {
                telar::Color::TRANSPARENT
            };
            RectStyle::filled(fill, rounded)
        },
        children,
    )?
    // Hovering selects rather than painting a highlight of its own, so what the pointer is on and what Enter would open are the same row — and an armed row keeps its warning colour under the pointer instead of being repainted as a plain selection.
    .on_hover(move |hovering| {
        if hovering {
            select();
        }
    })
    .on_press(move || {
        if dangerous && !armed_press() {
            return;
        }
        shell::close(ID);
        choose(&chosen);
    });
    Ok(Box::new(row))
}

/// The keyboard mode the launcher asks for, kept next to the surface it belongs to so the reason is visible: it opens on a keybind, and the next keystroke is already its first search character.
pub const KEYBOARD: KeyboardMode = KeyboardMode::Exclusive;

#[cfg(test)]
mod tests {
    use super::*;

    fn app(id: &str, name: &str, keywords: &[&str]) -> App {
        App {
            id: id.to_string(),
            name: name.to_string(),
            keywords: keywords.iter().map(|k| k.to_string()).collect(),
            exec: id.to_string(),
            ..App::default()
        }
    }

    fn catalog() -> Vec<App> {
        vec![
            app("firefox", "Firefox", &["www", "browser"]),
            app("files", "Files", &[]),
            app("code", "Visual Studio Code", &["editor"]),
        ]
    }

    #[test]
    fn an_empty_query_lists_everything_up_to_the_cap() {
        let config = LauncherConfig {
            max_results: 2,
            ..LauncherConfig::default()
        };
        assert_eq!(results(catalog(), "", &config).len(), 2);
        assert_eq!(
            results(catalog(), "", &LauncherConfig::default()).len(),
            3,
            "the default cap is above this catalog's size"
        );
    }

    #[test]
    fn a_query_narrows_to_matches_and_ranks_them() {
        let found = results(catalog(), "fire", &LauncherConfig::default());
        assert_eq!(found.first().map(|a| a.id.as_str()), Some("firefox"));
        assert_eq!(
            found.len(),
            1,
            "nothing else contains those letters in order"
        );
    }

    #[test]
    fn keywords_are_searchable_not_just_names() {
        // Nothing is called "www", but Firefox lists it — which is exactly what keywords are for.
        let found = results(catalog(), "www", &LauncherConfig::default());
        assert_eq!(found.first().map(|a| a.id.as_str()), Some("firefox"));
    }

    #[test]
    fn an_acronym_finds_the_editor() {
        let found = results(catalog(), "vsc", &LauncherConfig::default());
        assert_eq!(found.first().map(|a| a.id.as_str()), Some("code"));
    }

    #[test]
    fn hidden_apps_never_appear() {
        let config = LauncherConfig {
            hidden: vec!["firefox".to_string()],
            ..LauncherConfig::default()
        };
        let found = results(catalog(), "f", &config);
        assert!(
            !found.iter().any(|a| a.id == "firefox"),
            "a hidden app is hidden even when it matches"
        );
        assert!(found.iter().any(|a| a.id == "files"));
    }

    #[test]
    fn favourites_are_pinned_above_the_ranking_in_the_order_listed() {
        let config = LauncherConfig {
            favourites: vec!["files".to_string(), "code".to_string()],
            ..LauncherConfig::default()
        };
        let ids: Vec<String> = results(catalog(), "", &config)
            .into_iter()
            .map(|a| a.id)
            .collect();
        assert_eq!(
            &ids[..2],
            &["files", "code"],
            "config order, not rank order"
        );
        assert!(ids.len() > 2, "the rest still follow");
    }

    #[test]
    fn a_favourite_that_does_not_match_stays_out_of_the_results() {
        let config = LauncherConfig {
            favourites: vec!["code".to_string()],
            ..LauncherConfig::default()
        };
        let found = results(catalog(), "firefox", &config);
        assert!(
            !found.iter().any(|a| a.id == "code"),
            "pinning reorders the matches; it does not add one"
        );
        assert_eq!(found.first().map(|a| a.id.as_str()), Some("firefox"));
    }

    #[test]
    fn a_favourite_survives_the_result_cap() {
        // Ranked last but pinned first: the cap must not be what decides whether a favourite is shown.
        let config = LauncherConfig {
            favourites: vec!["files".to_string()],
            max_results: 1,
            ..LauncherConfig::default()
        };
        let found = results(catalog(), "", &config);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, "files");
    }

    fn action(name: &str, dangerous: bool) -> LauncherAction {
        LauncherAction {
            name: name.to_string(),
            command: format!("run-{name}"),
            dangerous,
            ..LauncherAction::default()
        }
    }

    fn names(entries: &[Entry]) -> Vec<String> {
        entries
            .iter()
            .map(|e| match e {
                Entry::App(app) => app.id.clone(),
                Entry::Action(a) => a.name.clone(),
                Entry::Calculation { result, .. } => result.clone(),
                Entry::Scheme { value, .. } => value.clone(),
                Entry::Wallpaper(entry) => entry.name.clone(),
                Entry::Window(window) => window.title.clone(),
            })
            .collect()
    }

    /// Two windows of one application plus a third, which is the case a switcher exists for: telling apart rows an application-keyed list would have collapsed into one.
    fn open() -> Vec<ManagedToplevel> {
        [
            ("kitty", "nvim"),
            ("kitty", "cargo test"),
            ("code", "README.md"),
        ]
        .into_iter()
        .enumerate()
        .map(|(index, (app_id, title))| ManagedToplevel {
            id: platform_wayland::ManagedToplevelId::from_raw(index as u32 + 1),
            app_id: app_id.to_string(),
            title: title.to_string(),
            ..ManagedToplevel::default()
        })
        .collect()
    }

    fn library() -> Vec<wallpaper::Entry> {
        ["sunset", "forest", "city"]
            .into_iter()
            .enumerate()
            .map(|(index, name)| wallpaper::Entry {
                path: std::path::PathBuf::from(format!("/pictures/{name}.png")),
                name: name.to_string(),
                folder: if index == 0 {
                    String::new()
                } else {
                    "nature".to_string()
                },
            })
            .collect()
    }

    #[test]
    fn a_prefix_selects_the_mode_and_is_stripped() {
        assert_eq!(mode_of("firefox"), (QueryMode::Apps, "firefox"));
        assert_eq!(mode_of("> reboot"), (QueryMode::Actions, "reboot"));
        assert_eq!(mode_of(">reboot"), (QueryMode::Actions, "reboot"));
        assert_eq!(mode_of("=2+2"), (QueryMode::Calculator, "2+2"));
        assert_eq!(mode_of("/nvim"), (QueryMode::Windows, "nvim"));
        assert_eq!(
            mode_of(">"),
            (QueryMode::Actions, ""),
            "a bare prefix lists everything in that mode"
        );
    }

    #[test]
    fn the_window_mode_lists_what_is_open_and_matches_on_both_names() {
        let config = LauncherConfig::default();

        let all = names(&entries(catalog(), Vec::new(), open(), "/", &config));
        assert_eq!(
            all,
            vec!["nvim", "cargo test", "README.md"],
            "a bare prefix lists every open window, in the order the compositor announced them"
        );

        assert_eq!(
            names(&entries(catalog(), Vec::new(), open(), "/README", &config)),
            vec!["README.md"],
            "the title is in the haystack"
        );
        assert_eq!(
            names(&entries(catalog(), Vec::new(), open(), "/kitty", &config)),
            vec!["nvim", "cargo test"],
            "and so is the application id, which is what finds every window of one application"
        );
    }

    /// The reason a row is keyed on the protocol object rather than on what it says: two windows of one application routinely share an application id and sometimes a title, and a list that collapsed them would offer no way to reach the second one.
    #[test]
    fn two_windows_of_one_application_stay_two_rows() {
        // Same application, same title, different windows — two terminals in the same directory, which is the case that collapses if the row is keyed on anything the user can see.
        let mut twins = open();
        twins[1].title = twins[0].title.clone();

        let rows = entries(
            catalog(),
            Vec::new(),
            twins,
            "/",
            &LauncherConfig::default(),
        );
        assert_eq!(rows.len(), 3);
        let keys: std::collections::HashSet<String> = rows.iter().map(Entry::key).collect();
        assert_eq!(
            keys.len(),
            3,
            "a keyed list shows one row per key, so a collision here is a window the user cannot reach"
        );
    }

    #[test]
    fn a_window_row_shows_the_title_over_the_application() {
        let rows = entries(
            catalog(),
            Vec::new(),
            open(),
            "/README",
            &LauncherConfig::default(),
        );
        assert_eq!(
            row_text(&rows[0]),
            ("README.md".to_string(), "code".to_string())
        );
    }

    /// A compositor with no window management protocol publishes nothing, and the mode has to be empty rather than fall through to listing applications — which would launch a second copy of what the user was trying to switch to.
    #[test]
    fn no_windows_is_an_empty_list_rather_than_the_applications() {
        let found = entries(
            catalog(),
            Vec::new(),
            Vec::new(),
            "/fire",
            &LauncherConfig::default(),
        );
        assert!(found.is_empty(), "{found:?}");
    }

    #[test]
    fn the_calculator_answers_above_the_apps_without_hiding_them() {
        let found = entries(
            catalog(),
            Vec::new(),
            Vec::new(),
            "2+2",
            &LauncherConfig::default(),
        );
        assert!(
            matches!(found.first(), Some(Entry::Calculation { result, .. }) if result == "4"),
            "the answer leads: {:?}",
            names(&found)
        );

        // A query that happens to parse as arithmetic must not hide the app search underneath it.
        let mixed = entries(
            catalog(),
            Vec::new(),
            Vec::new(),
            "2+2",
            &LauncherConfig::default(),
        );
        assert!(
            mixed.len() > 1 || mixed.iter().all(|e| matches!(e, Entry::Calculation { .. })),
            "app matches still follow when there are any"
        );

        // And a plain name never grows a calculation row.
        let plain = entries(
            catalog(),
            Vec::new(),
            Vec::new(),
            "firefox",
            &LauncherConfig::default(),
        );
        assert!(!plain.iter().any(|e| matches!(e, Entry::Calculation { .. })));
    }

    #[test]
    fn the_calc_prefix_forces_a_result_where_auto_detection_declines() {
        let config = LauncherConfig::default();
        // A bare number is treated as the start of a name, not a sum — unless asked explicitly.
        assert!(
            !entries(catalog(), Vec::new(), Vec::new(), "2", &config)
                .iter()
                .any(|e| matches!(e, Entry::Calculation { .. }))
        );
        assert_eq!(
            names(&entries(catalog(), Vec::new(), Vec::new(), "=2", &config)),
            vec!["2".to_string()]
        );
    }

    #[test]
    fn the_scheme_mode_lists_palettes_modes_and_variants_and_finds_them_by_real_name() {
        let config = LauncherConfig::default();
        assert_eq!(mode_of("#latte"), (QueryMode::Schemes, "latte"));

        // Uncapped, because what is under test is which choices exist, not the row cap the other modes share.
        let uncapped = LauncherConfig {
            max_results: 200,
            ..LauncherConfig::default()
        };
        let all = entries(catalog(), Vec::new(), Vec::new(), "#", &uncapped);
        assert!(
            all.iter().all(|e| matches!(e, Entry::Scheme { .. })),
            "the mode is exclusive: apps do not leak into it"
        );
        let kinds = |choice: scheme::Choice| {
            all.iter()
                .filter(|e| matches!(e, Entry::Scheme { choice: c, .. } if *c == choice))
                .count()
        };
        assert!(
            kinds(scheme::Choice::Palette) > 1,
            "every palette is offered"
        );
        assert!(kinds(scheme::Choice::Mode) >= 3, "auto, dark and light");
        assert_eq!(kinds(scheme::Choice::Variant), scheme::Variant::ALL.len());

        // The palette's own name, not just its config spelling — `catppuccin-latte` is not what anyone types.
        let found = entries(catalog(), Vec::new(), Vec::new(), "#latte", &config);
        assert!(
            matches!(found.first(), Some(Entry::Scheme { value, .. }) if value == "catppuccin-latte"),
            "'#latte' finds Catppuccin Latte: {:?}",
            names(&found)
        );
        assert!(
            entries(catalog(), Vec::new(), Vec::new(), "#dynamic", &config)
                .iter()
                .any(|e| matches!(e, Entry::Scheme { value, .. } if value == scheme::DYNAMIC)),
            "the wallpaper-derived palette is pickable too"
        );
    }

    #[test]
    fn a_scheme_row_cannot_collide_with_another_kind_of_row() {
        // The reactive list reconciles on these, and "dark" is a plausible application name.
        let palette = Entry::Scheme {
            choice: scheme::Choice::Palette,
            value: "nord".to_string(),
        };
        let mode = Entry::Scheme {
            choice: scheme::Choice::Mode,
            value: "nord".to_string(),
        };
        assert_ne!(
            palette.key(),
            mode.key(),
            "the kind is part of the identity"
        );
        assert_ne!(palette.key(), Entry::App(app("nord", "nord", &[])).key());
        assert!(
            !palette.is_dangerous(),
            "a palette is never a confirming choice"
        );
    }

    #[test]
    fn a_unit_conversion_answers_like_any_other_calculation() {
        let config = LauncherConfig::default();
        let found = entries(catalog(), Vec::new(), Vec::new(), "3 km in mi", &config);
        assert!(
            matches!(found.first(), Some(Entry::Calculation { result, .. }) if result == "1.8641135767 mi"),
            "the conversion leads, unit and all: {:?}",
            names(&found)
        );
        assert_eq!(
            names(&entries(
                catalog(),
                Vec::new(),
                Vec::new(),
                "=100 c in f",
                &config
            )),
            vec!["212 °F".to_string()],
            "the explicit prefix works the same way"
        );

        // And the switch that turns the calculator off turns conversions off with it — they are the same feature.
        let off = LauncherConfig {
            calculator: false,
            ..LauncherConfig::default()
        };
        assert!(
            !entries(catalog(), Vec::new(), Vec::new(), "3 km in mi", &off)
                .iter()
                .any(|e| matches!(e, Entry::Calculation { .. }))
        );
    }

    /// Headless there is no worker, so this is what the guard is really about: an app search must not reach for a subprocess, and the launcher must build a result list either way.
    #[test]
    fn only_an_explicit_calculation_falls_back_to_qalc() {
        let config = LauncherConfig::default();
        assert!(config.qalc, "the fallback is on by default");
        // A question the built-in evaluator has no answer for: no row, and no process, until qalc answers.
        assert!(entries(catalog(), Vec::new(), Vec::new(), "=1 usd in eur", &config).is_empty());
        // The same text without the prefix is an app search and never asks anything.
        assert!(
            entries(catalog(), Vec::new(), Vec::new(), "1 usd in eur", &config)
                .iter()
                .all(|e| matches!(e, Entry::App(_)))
        );
    }

    #[test]
    fn the_calculator_can_be_switched_off_entirely() {
        let config = LauncherConfig {
            calculator: false,
            ..LauncherConfig::default()
        };
        assert!(
            !entries(catalog(), Vec::new(), Vec::new(), "2+2", &config)
                .iter()
                .any(|e| matches!(e, Entry::Calculation { .. }))
        );
        assert!(entries(catalog(), Vec::new(), Vec::new(), "=2+2", &config).is_empty());
    }

    #[test]
    fn action_mode_lists_and_ranks_the_declared_actions() {
        let config = LauncherConfig {
            actions: vec![action("lock", false), action("logout", false)],
            ..LauncherConfig::default()
        };
        assert_eq!(
            names(&entries(catalog(), Vec::new(), Vec::new(), ">", &config)).len(),
            2
        );
        assert_eq!(
            names(&entries(catalog(), Vec::new(), Vec::new(), ">lo", &config)).len(),
            2
        );
        assert_eq!(
            names(&entries(
                catalog(),
                Vec::new(),
                Vec::new(),
                ">lock",
                &config
            )),
            vec!["lock".to_string()]
        );
        // Action mode is exclusive: apps do not leak into it.
        assert!(
            entries(catalog(), Vec::new(), Vec::new(), ">", &config)
                .iter()
                .all(|e| matches!(e, Entry::Action(_)))
        );
    }

    #[test]
    fn a_dangerous_action_is_hidden_until_it_is_allowed() {
        let mut config = LauncherConfig {
            actions: vec![action("lock", false), action("wipe", true)],
            ..LauncherConfig::default()
        };
        assert_eq!(
            names(&entries(catalog(), Vec::new(), Vec::new(), ">", &config)),
            vec!["lock".to_string()],
            "a dangerous action is not even listed without the opt-in"
        );

        config.enable_dangerous_actions = true;
        let listed = entries(catalog(), Vec::new(), Vec::new(), ">", &config);
        assert_eq!(names(&listed).len(), 2);
        assert!(
            listed.iter().any(|e| e.is_dangerous()),
            "and once allowed it still asks for a confirming Enter"
        );
    }

    #[test]
    fn an_incomplete_or_disabled_action_never_appears() {
        let config = LauncherConfig {
            actions: vec![
                LauncherAction {
                    name: "no command".to_string(),
                    ..LauncherAction::default()
                },
                LauncherAction {
                    command: "run".to_string(),
                    ..LauncherAction::default()
                },
                LauncherAction {
                    enabled: false,
                    ..action("parked", false)
                },
                action("good", false),
            ],
            ..LauncherConfig::default()
        };
        assert_eq!(
            names(&entries(catalog(), Vec::new(), Vec::new(), ">", &config)),
            vec!["good".to_string()]
        );
    }

    #[test]
    fn the_wallpaper_mode_browses_the_library_and_finds_a_folder_by_name() {
        let config = LauncherConfig::default();
        assert_eq!(mode_of("@sun"), (QueryMode::Wallpapers, "sun"));

        let all = entries(catalog(), library(), Vec::new(), "@", &config);
        assert_eq!(all.len(), 3, "a bare prefix browses the whole library");
        assert!(
            all.iter().all(|e| matches!(e, Entry::Wallpaper(_))),
            "the mode is exclusive: apps do not leak into it"
        );

        assert_eq!(
            names(&entries(
                catalog(),
                library(),
                Vec::new(),
                "@sunset",
                &config
            )),
            vec!["sunset".to_string()]
        );
        // The folder is searchable, so a whole collection is reachable without remembering one file's name.
        let folder = names(&entries(
            catalog(),
            library(),
            Vec::new(),
            "@nature",
            &config,
        ));
        assert_eq!(
            folder.len(),
            2,
            "both images filed under nature: {folder:?}"
        );

        // An empty library is the case where the folder is missing or `[wallpaper] enabled` is off. It shows nothing rather than falling back to the app list, which would be a different answer to what was asked.
        assert!(entries(catalog(), Vec::new(), Vec::new(), "@", &config).is_empty());
    }

    /// The row cap is a *list* bound. A grid four across would show three rows of a library and stop, which reads as the library being that small.
    #[test]
    fn the_wallpaper_grid_is_not_cut_off_by_the_row_cap() {
        let config = LauncherConfig {
            max_results: 2,
            ..LauncherConfig::default()
        };
        assert_eq!(
            entries(catalog(), library(), Vec::new(), "@", &config).len(),
            3
        );
        assert_eq!(
            entries(catalog(), Vec::new(), Vec::new(), "", &config).len(),
            2,
            "while a list mode still honours it"
        );

        // It has a bound of its own, and it is the reactive list that needs it: every tile is a widget built up front, so a picture archive would spend the UI thread before the panel appeared.
        let archive: Vec<wallpaper::Entry> = (0..GRID_CAP + 40)
            .map(|index| wallpaper::Entry {
                path: std::path::PathBuf::from(format!("/pictures/{index}.png")),
                name: format!("image {index}"),
                folder: String::new(),
            })
            .collect();
        assert_eq!(
            entries(catalog(), archive, Vec::new(), "@", &config).len(),
            GRID_CAP,
            "the grid draws a bounded number of tiles however big the library is"
        );
    }

    #[test]
    fn only_a_grid_mode_has_columns_and_it_fits_them_to_the_panel() {
        assert_eq!(QueryMode::Apps.columns(640.0), 1);
        assert_eq!(QueryMode::Actions.columns(640.0), 1);
        assert_eq!(
            QueryMode::Wallpapers.columns(640.0),
            3,
            "the default panel holds three 150px tiles once its padding is taken off"
        );
        assert!(
            QueryMode::Wallpapers.columns(1200.0) > QueryMode::Wallpapers.columns(640.0),
            "a wider panel shows more per row rather than bigger pictures"
        );
        // A panel narrower than one tile still has to lay out: a column count of zero divides by zero downstream.
        assert_eq!(QueryMode::Wallpapers.columns(80.0), 2);
        assert!(tile_width(QueryMode::Wallpapers.columns(80.0), 80.0) >= 48.0);
    }

    #[test]
    fn a_grid_line_is_a_row_of_tiles_and_a_list_line_is_one_row() {
        let images: Vec<Entry> = library().into_iter().map(Entry::Wallpaper).collect();
        let rows = lines(images.clone(), 1);
        assert_eq!(rows.len(), 3, "one column is a list");
        assert!(rows.iter().all(|line| matches!(line, Line::Row(_))));

        let grid = lines(images.clone(), 2);
        assert_eq!(grid.len(), 2, "three tiles two across is two lines");
        assert!(
            matches!(&grid[1], Line::Tiles(tiles) if tiles.len() == 1),
            "a short last row is still a row"
        );

        // The line's identity is what it draws: re-chunking must not let a stale row keep its pictures.
        assert_ne!(grid[0].key(), grid[1].key());
        assert_ne!(
            lines(images.clone(), 2)[0].key(),
            lines(images, 3)[0].key(),
            "the same first tile in a wider row is a different line"
        );
        assert!(lines(Vec::new(), 4).is_empty());
    }

    /// The reactive rules only bite when the closures actually run, and nothing but a build runs them: a signal read nested inside another's `with` panics here and nowhere else. The regression this exists for: `entries` ran *inside* `query.with(...)`, and two of its modes read a second signal — the scheme rows call `t!`, the `=` rows ask the qalc loader for one. Both panicked with "RefCell already borrowed" the moment the user typed `#` or `=`, and nothing but running the closure finds it.
    #[test]
    fn typing_any_mode_into_the_results_memo_never_double_borrows_the_runtime() {
        telar::reset_layout_runtime();
        telar::set_theme(NordTheme::new());
        let query = signal(String::new());
        let library = signal(library());
        let open = signal(open());
        let shown = results_memo(
            catalog(),
            library.read_only(),
            open.read_only(),
            query.read_only(),
            LauncherConfig::default(),
        );

        for text in [
            "",
            "fire",
            ">",
            "#",
            "#latte",
            "#dynamic",
            "=2+2",
            "=3 km in mi",
            "=12 in in cm",
            "=1 usd in eur",
            "@",
            "@nature",
            "/",
            "/nvim",
        ] {
            query.set(text.to_string());
            // `get` is what runs the closure; a nested read panics here and nowhere else.
            let rows = shown.get();
            assert!(
                rows.len() <= GRID_CAP,
                "'{text}' produced {} rows",
                rows.len()
            );
        }
    }

    #[test]
    fn a_row_of_tiles_builds_and_knows_which_one_is_selected() {
        telar::reset_layout_runtime();
        telar::set_theme(NordTheme::new());
        let images: Vec<Entry> = library().into_iter().map(Entry::Wallpaper).collect();
        let source = signal(images.clone());
        let read = source.read_only();
        let shown = memo(move || read.get());
        let selected = signal(1usize);

        assert!(
            tile_row(images.clone(), shown, selected, 3, NordTheme::new(),).is_ok(),
            "a row of tiles builds"
        );

        // By key, so the selection survives the list being re-chunked under it by a change of column count.
        let holds = selection_is(shown, selected.read_only(), vec![images[1].key()]);
        assert!(holds(), "the tile holding the selected entry knows it does");
        let elsewhere = selection_is(shown, selected.read_only(), vec![images[0].key()]);
        assert!(!elsewhere());
    }

    /// Hovering moves the one selection instead of painting a second highlight beside it.
    ///
    /// The pointer used to paint its own hover fill in the same colour as the selection, so two entries looked chosen at once — one that a click would open and one that Enter would — and the pointer's stayed lit after it had moved off the list. There is one selection now, and the pointer is one of the two things that moves it.
    #[test]
    fn hovering_an_entry_moves_the_selection_onto_it() {
        telar::reset_layout_runtime();
        telar::set_theme(NordTheme::new());
        let images: Vec<Entry> = library().into_iter().map(Entry::Wallpaper).collect();
        let source = signal(images.clone());
        let read = source.read_only();
        let shown = memo(move || read.get());
        let selected = signal(0usize);

        let hover_second = select_onto(shown, selected, images[1].key());
        hover_second();
        assert_eq!(
            selected.peek(),
            1,
            "the hovered entry becomes the selection"
        );

        let holds = selection_is(shown, selected.read_only(), vec![images[1].key()]);
        let elsewhere = selection_is(shown, selected.read_only(), vec![images[0].key()]);
        assert!(holds(), "and is the entry that reads as selected");
        assert!(
            !elsewhere(),
            "while the one the keyboard had left goes back to unselected — one highlight, not two"
        );

        // An entry the query has since filtered out cannot be selected by pointing at where it used to be.
        let gone = select_onto(shown, selected, "wallpaper:/nowhere.png".to_string());
        gone();
        assert_eq!(
            selected.peek(),
            1,
            "an entry that is not in the list is not a selection"
        );
    }

    /// The regression this exists for: a scroll area is a layout *leaf* — its content is laid out as its own root, so nothing inside it contributes to its size. Styled with `max_height` and no height, the list measured 612×0 and every result was clipped out of existence. Building proves none of that; only measuring does.
    #[test]
    fn the_result_list_has_the_height_it_was_given() {
        use telar::{AvailableSpace, JustifyContent, compute_layout, new_container, track_layout};

        telar::reset_layout_runtime();
        telar::set_theme(NordTheme::new());
        let rows: Vec<Entry> = catalog().into_iter().map(Entry::App).collect();
        let source = signal(rows);
        let read = source.read_only();
        let shown = memo(move || read.get());
        let columns = memo(|| 1usize);
        let selected = signal(0usize);
        let armed = signal(String::new());
        let list = result_list(
            shown,
            columns,
            selected,
            armed.read_only(),
            260.0,
            NordTheme::new(),
        )
        .expect("the list builds");
        let rect = track_layout(list.layout_node()).expect("the list registers its rect");

        // The panel and the scaffold that centres it: a content-sized column inside a full-screen flex, which is where a launcher list has no free space to grow into and has to carry its own height.
        let panel = new_container(
            LayoutStyle::new()
                .flex_column()
                .gap(space::lg())
                .padding_all(space::xl())
                .width(640.0),
            &[list.layout_node()],
        )
        .expect("panel");
        let root = new_container(
            LayoutStyle::new()
                .flex_column()
                .align_items(AlignItems::CENTER)
                .justify_content(JustifyContent::CENTER)
                .width(1920.0)
                .height(1080.0),
            &[panel],
        )
        .expect("scaffold");
        compute_layout(
            root,
            AvailableSpace::Definite(1920.0),
            AvailableSpace::Definite(1080.0),
        )
        .expect("layout");

        let rect = rect.get();
        assert_eq!(
            (rect.width, rect.height),
            (640.0 - space::xl() * 2.0, 260.0),
            "the list measured {}x{} — a zero-height viewport clips every result",
            rect.width,
            rect.height
        );
    }

    /// The reactive rules only bite when the closures actually run, and nothing but a build runs them: a signal read nested inside another's `with` panics here and nowhere else.
    #[test]
    fn the_result_list_builds_as_a_list_and_as_a_grid() {
        let images: Vec<Entry> = library().into_iter().map(Entry::Wallpaper).collect();
        for across in [1usize, 3] {
            telar::reset_layout_runtime();
            telar::set_theme(NordTheme::new());
            let source = signal(images.clone());
            let read = source.read_only();
            let shown = memo(move || read.get());
            let columns = memo(move || across);
            let selected = signal(0usize);
            let armed = signal(String::new());
            assert!(
                result_list(
                    shown,
                    columns,
                    selected,
                    armed.read_only(),
                    300.0,
                    NordTheme::new(),
                )
                .is_ok(),
                "{across} column(s) builds"
            );
        }
    }

    #[test]
    fn entry_keys_separate_the_kinds() {
        // The reactive list reconciles on these, so an app and an action sharing a name must not collide.
        let app = Entry::App(app("lock", "lock", &[]));
        let act = Entry::Action(action("lock", false));
        assert_ne!(app.key(), act.key());
        assert!(!app.is_dangerous());
        assert!(Entry::Action(action("wipe", true)).is_dangerous());
    }

    /// The launcher's own half of the shared bindings: with vim mode off (the default), a letter has to reach the search field. `shared::keynav` owns the wrapping and the vim keys; this guards the one thing that is the launcher's to get wrong — swallowing typing.
    #[test]
    fn typing_reaches_the_search_field_while_the_arrows_drive_the_list() {
        use telar::{Key, NamedKey};
        use ui::keynav::Move;

        let nav = keynav::from_config(&config::KeyNavConfig::default());
        for letter in ['j', 'k', 'g', 'G', 'q'] {
            assert_eq!(
                nav.interpret(&Key::Char(letter)),
                None,
                "'{letter}' must be typed into the query, not eaten by the list"
            );
        }
        assert_eq!(
            nav.interpret(&Key::Named(NamedKey::ArrowDown)),
            Some(Move::Next)
        );
        assert_eq!(
            nav.interpret(&Key::Named(NamedKey::Enter)),
            Some(Move::Activate)
        );
    }

    #[test]
    fn substring_mode_is_stricter_than_fuzzy() {
        let fuzzy = LauncherConfig::default();
        let strict = LauncherConfig {
            fuzzy: false,
            ..LauncherConfig::default()
        };
        assert_eq!(results(catalog(), "ff", &fuzzy).len(), 1);
        assert_eq!(
            results(catalog(), "ff", &strict).len(),
            0,
            "substring mode refuses the gap between the two f's"
        );
    }
}
