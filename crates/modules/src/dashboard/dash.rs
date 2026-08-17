//! The Dash page: what time it is, what month it is, and whose machine this is.

use ui::scale::{paint, space};
use std::path::{Path, PathBuf};

use chrono::{Datelike, Days, Local, Months, NaiveDate, Weekday};
use telar::{
    AlignItems, Color, Container, JustifyContent, LayoutError, LayoutItem, LayoutStyle,
    ReactiveList, RectStyle, RwSignal, SizeDimension, StyledContainer, Text, box_item, signal,
};

use super::card;
use config::theme::{FontRole, NordTheme};
use config::{ClockConfig, Config, DashboardConfig};
use services::clock;
use ui::icon::icon_view;
use util::paths;
use util::picture;
use util::reactive::derive;

/// A month grid is at most six rows of seven — February starting on the last column of the week is the case
/// that needs the sixth.
const WEEKS: u32 = 6;
const CELL_HEIGHT: f32 = 30.0;
const AVATAR: f32 = 56.0;

pub fn page(config: &Config, theme: NordTheme) -> Result<Box<dyn LayoutItem>, LayoutError> {
    card::page(vec![
        clock_card(config.clock.clone(), theme)?,
        calendar_card(&config.dashboard, theme)?,
        user_card(&config.dashboard, theme)?,
    ])
}

/// F2. The same `[clock]` config that drives the bar chip, given the room the bar does not have: the time at
/// display size, the date under it whether or not the chip was asked to show one.
fn clock_card(config: ClockConfig, theme: NordTheme) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let for_tick = config.clone();
    let now = signal(Local::now());
    let sink = now.clone();
    platform_wayland::watch(clock::subscribe, move |t| sink.set(t));

    let time = derive(now.clone(), move |t| {
        t.format(for_tick.time_format()).to_string()
    });
    let date = derive(now, move |t| t.format(&config.date_format).to_string());

    let time_text = Text::auto(
        move || time.get(),
        LayoutStyle::new(),
        move || {
            theme
                .text_style(FontRole::Display, theme.text)
                .with_weight(700)
        },
    )?;
    let date_text = Text::auto(
        move || date.get(),
        LayoutStyle::new(),
        move || theme.text_style(FontRole::Body, theme.subtle),
    )?;

    let stack = Container::new(
        LayoutStyle::new()
            .flex_column()
            .align_items(AlignItems::CENTER)
            .gap(space::XS)
            .width(SizeDimension::Percent(1.0)),
        vec![box_item(time_text), box_item(date_text)],
    )?;
    card::frame(vec![Box::new(stack)], theme)
}

/// F3. The month, navigable, with today marked.
///
/// The grid is a keyed list over the anchor rather than a tree rebuilt in place, because that is what makes
/// stepping a month a rebuild of the cells and nothing else — the heading, the weekday row and the card around
/// them are laid out once and stay.
fn calendar_card(
    config: &DashboardConfig,
    theme: NordTheme,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let first_weekday = config.first_weekday();
    let today = Local::now().date_naive();
    let anchor = signal(first_of_month(today));

    let title = derive(anchor.clone(), |month| {
        format!("{} {}", month_label(month.month()), month.year())
    });

    let heading = Container::new(
        LayoutStyle::new()
            .flex_row()
            .align_items(AlignItems::CENTER)
            .gap(space::MD)
            .width(SizeDimension::Percent(1.0)),
        vec![
            step_button("chevron-left", anchor.clone(), -1, theme)?,
            box_item(Text::auto(
                move || title.get(),
                LayoutStyle::new()
                    .flex_grow(1.0)
                    .justify_content(JustifyContent::CENTER),
                move || {
                    theme
                        .text_style(FontRole::Title, theme.text)
                        .with_weight(700)
                },
            )?),
            step_button("chevron-right", anchor.clone(), 1, theme)?,
        ],
    )?;

    let source = anchor.read_only();
    let grid = ReactiveList::new(
        move || vec![source.get()],
        |month: &NaiveDate| month.format("%Y-%m").to_string(),
        move |month: NaiveDate| month_grid(month, today, first_weekday, theme),
        0.0,
    )?;

    card::frame(
        vec![
            Box::new(heading),
            weekday_header(first_weekday, theme)?,
            Box::new(grid),
        ],
        theme,
    )
}

fn step_button(
    glyph: &'static str,
    anchor: RwSignal<NaiveDate>,
    months: i32,
    theme: NordTheme,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let icon = icon_view(move || glyph.to_string(), move || theme.subtle, 16.0)?;
    Ok(Box::new(
        StyledContainer::new(
            LayoutStyle::new()
                .padding_all(space::SM)
                .flex_shrink(0.0)
                .align_items(AlignItems::CENTER),
            paint::xs(Color::TRANSPARENT),
            vec![icon],
        )?
        .hover_style(paint::xs(theme.overlay))
        .on_press(move || anchor.set(shift_months(anchor.peek(), months))),
    ))
}

fn weekday_header(first: Weekday, theme: NordTheme) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let mut cells: Vec<Box<dyn LayoutItem>> = Vec::with_capacity(7);
    for offset in 0..7 {
        let weekday = shift_weekday(first, offset);
        cells.push(box_item(Text::auto(
            move || weekday_label(weekday),
            LayoutStyle::new()
                .flex_grow(1.0)
                .flex_basis(0.0)
                .justify_content(JustifyContent::CENTER),
            move || {
                theme
                    .text_style(FontRole::Caption, theme.muted)
                    .with_weight(700)
            },
        )?));
    }
    Ok(Box::new(Container::new(
        LayoutStyle::new()
            .flex_row()
            .width(SizeDimension::Percent(1.0)),
        cells,
    )?))
}

fn month_grid(
    month: NaiveDate,
    today: NaiveDate,
    first: Weekday,
    theme: NordTheme,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let start = grid_start(month, first);
    let mut rows: Vec<Box<dyn LayoutItem>> = Vec::with_capacity(WEEKS as usize);
    for week in 0..WEEKS {
        let mut cells: Vec<Box<dyn LayoutItem>> = Vec::with_capacity(7);
        for column in 0..7 {
            let Some(date) = start.checked_add_days(Days::new((week * 7 + column) as u64)) else {
                continue;
            };
            cells.push(day_cell(date, month, today, theme)?);
        }
        rows.push(Box::new(Container::new(
            LayoutStyle::new()
                .flex_row()
                .width(SizeDimension::Percent(1.0)),
            cells,
        )?));
    }
    Ok(Box::new(Container::new(
        LayoutStyle::new()
            .flex_column()
            .gap(space::XS)
            .width(SizeDimension::Percent(1.0)),
        rows,
    )?))
}

/// One day. A date outside the month being shown is drawn muted rather than blanked: the leading and trailing
/// days are what make the week rows line up, and a reader looking at "the 1st is a Wednesday" needs to see the
/// Monday and Tuesday it follows.
fn day_cell(
    date: NaiveDate,
    month: NaiveDate,
    today: NaiveDate,
    theme: NordTheme,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let is_today = date == today;
    let in_month = date.month() == month.month() && date.year() == month.year();
    let ink = if is_today {
        theme.accent.most_readable(&[theme.text, theme.base])
    } else if in_month {
        theme.text
    } else {
        theme.muted
    };
    let label = date.day().to_string();
    let text = Text::auto(
        move || label.clone(),
        LayoutStyle::new(),
        move || theme.text_style(FontRole::Caption, ink),
    )?;
    let fill = if is_today {
        theme.accent
    } else {
        Color::TRANSPARENT
    };
    Ok(Box::new(StyledContainer::new(
        LayoutStyle::new()
            .flex_grow(1.0)
            .flex_basis(0.0)
            .height(CELL_HEIGHT)
            .align_items(AlignItems::CENTER)
            .justify_content(JustifyContent::CENTER),
        move |_r| RectStyle::filled(fill, CELL_HEIGHT / 2.0),
        vec![box_item(text)],
    )?))
}

/// F4. Who is logged in, on what machine, and for how long.
fn user_card(
    config: &DashboardConfig,
    theme: NordTheme,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let name = username();
    let host = hostname();

    let picking = signal(false);
    let start = avatar_path(config)
        .and_then(|path| path.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| paths::user_dir("XDG_PICTURES_DIR", "Pictures"));

    let face = match avatar_path(config).and_then(|path| picture::circle(&path, AVATAR)) {
        Some(picture) => picture,
        None => icon_view(
            || "circle-user-round".to_string(),
            move || theme.subtle,
            AVATAR,
        )?,
    };
    // F4a: the picture is the control. A press opens the browser below it, which is the only affordance the
    // card has room for and the only one a user would look for — nothing else on this card is pressable.
    let open = picking.clone();
    let avatar: Box<dyn LayoutItem> = Box::new(
        StyledContainer::new(
            LayoutStyle::new()
                .flex_shrink(0.0)
                .align_items(AlignItems::CENTER)
                .justify_content(JustifyContent::CENTER),
            move |_r| RectStyle::filled(Color::TRANSPARENT, AVATAR / 2.0),
            vec![face],
        )?
        .hover_style(move |_r| RectStyle::filled(theme.overlay, AVATAR / 2.0))
        .on_press(move || open.set(!open.peek())),
    );

    let name_text = Text::auto(
        move || name.clone(),
        LayoutStyle::new(),
        move || {
            theme
                .text_style(FontRole::Title, theme.text)
                .with_weight(700)
        },
    )?;
    let host_text = Text::auto(
        move || host.clone(),
        LayoutStyle::new(),
        move || theme.text_style(FontRole::Caption, theme.subtle),
    )?;

    // Uptime rides the shared clock rather than arming a ticker of its own; it changes once a minute, and the
    // second boundary is already being published to every surface.
    let now = signal(Local::now());
    let sink = now.clone();
    platform_wayland::watch(clock::subscribe, move |t| sink.set(t));
    let uptime = derive(now, |_| match read_uptime() {
        Some(seconds) => telar::t!("dashboard.uptime", time = duration_label(seconds)),
        None => telar::t!("sysinfo.no_reading"),
    });
    let uptime_text = Text::auto(
        move || uptime.get(),
        LayoutStyle::new(),
        move || theme.text_style(FontRole::Caption, theme.muted),
    )?;

    let labels = Container::new(
        LayoutStyle::new().flex_column().flex_grow(1.0).gap(space::XS),
        vec![
            box_item(name_text),
            box_item(host_text),
            box_item(uptime_text),
        ],
    )?;

    let row = Container::new(
        LayoutStyle::new()
            .flex_row()
            .align_items(AlignItems::CENTER)
            .gap(space::XL)
            .width(SizeDimension::Percent(1.0)),
        vec![avatar, Box::new(labels)],
    )?;
    card::frame(
        vec![Box::new(row), avatar_picker(start, picking, theme)?],
        theme,
    )
}

/// How many entries the avatar browser draws at once, and how large each thumbnail is. The same bound, and the
/// same reason, as the launcher's wallpaper grid: `ReactiveList` builds a widget per tile up front.
const PICKER_ENTRIES: usize = 120;
const PICKER_TILE: f32 = 64.0;

/// One row of the avatar browser: a folder to go into, or a picture to become the avatar.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Choice {
    path: PathBuf,
    name: String,
    folder: bool,
}

/// F4a: the file picker the shell did not have.
///
/// Deliberately not a portal dialog — that is another process and another toolkit on screen — and deliberately
/// not a second surface: a dashboard page already *is* the surface, and a picker that opened its own would have
/// to be anchored, dismissed and kept on the right monitor for a job that is one press long. It browses instead
/// of asking for a path, because a user who could type the path already has `[dashboard] avatar`.
fn avatar_picker(
    start: PathBuf,
    picking: RwSignal<bool>,
    theme: NordTheme,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let folder = signal(start);
    // A one-item keyed list over the open flag, which is how this shell expresses a reactive `if`: the browser
    // is *built* when it opens and torn down when it closes, so a card that is not picking an avatar costs
    // exactly what it did before this landed rather than carrying a hidden subtree.
    let open = picking.read_only();
    Ok(Box::new(ReactiveList::new(
        move || vec![open.get()],
        |open: &bool| open.to_string(),
        move |open: bool| {
            if open {
                browser(folder.clone(), picking.clone(), theme)
            } else {
                Ok(Box::new(Container::new(LayoutStyle::new(), vec![])?) as Box<dyn LayoutItem>)
            }
        },
        0.0,
    )?))
}

fn browser(
    folder: RwSignal<PathBuf>,
    picking: RwSignal<bool>,
    theme: NordTheme,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let source_folder = folder.read_only();

    let path_label = Text::auto(
        move || source_folder.get().display().to_string(),
        LayoutStyle::new().flex_grow(1.0),
        move || {
            theme
                .text_style(FontRole::Caption, theme.muted)
                .with_max_lines(1)
                .with_ellipsis(true)
        },
    )?;

    let up_folder = folder.clone();
    let up = StyledContainer::new(
        LayoutStyle::new()
            .flex_shrink(0.0)
            .padding_horizontal(space::MD)
            .padding_vertical(space::SM),
        paint::md(theme.base),
        vec![icon_view(
            || "corner-left-up".to_string(),
            move || theme.text,
            16.0,
        )?],
    )?
    .hover_style(paint::md(theme.overlay))
    .on_press(move || {
        let here = up_folder.peek();
        if let Some(parent) = here.parent() {
            up_folder.set(parent.to_path_buf());
        }
    });

    let list_folder = folder.read_only();
    let into = folder.clone();
    let tiles = ReactiveList::with_style(
        LayoutStyle::new()
            .flex_row()
            .flex_wrap()
            .gap(space::MD)
            .width(SizeDimension::Percent(1.0)),
        move || entries_in(&list_folder.get()),
        |choice: &Choice| choice.path.display().to_string(),
        move |choice: Choice| choice_tile(choice, into.clone(), picking.clone(), theme),
    )?;

    let header = Container::new(
        LayoutStyle::new()
            .flex_row()
            .align_items(AlignItems::CENTER)
            .gap(space::MD)
            .width(SizeDimension::Percent(1.0)),
        vec![Box::new(up), box_item(path_label)],
    )?;

    Ok(Box::new(Container::new(
        LayoutStyle::new()
            .flex_column()
            .gap(space::MD)
            .width(SizeDimension::Percent(1.0)),
        vec![Box::new(header), Box::new(tiles)],
    )?))
}

/// What a folder offers: its sub-folders, then the pictures in it, each alphabetical. Hidden entries are left
/// out — a picture someone chose to hide is not one they are looking for here.
fn entries_in(folder: &Path) -> Vec<Choice> {
    let Ok(read) = std::fs::read_dir(folder) else {
        return Vec::new();
    };
    let (mut folders, mut images): (Vec<Choice>, Vec<Choice>) = (Vec::new(), Vec::new());
    for entry in read.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().map(|n| n.to_string_lossy().into_owned()) else {
            continue;
        };
        if name.starts_with('.') {
            continue;
        }
        if path.is_dir() {
            folders.push(Choice {
                path,
                name,
                folder: true,
            });
        } else if is_picture(&path) {
            images.push(Choice {
                path,
                name,
                folder: false,
            });
        }
    }
    folders.sort_by_key(|choice| choice.name.to_lowercase());
    images.sort_by_key(|choice| choice.name.to_lowercase());
    folders.extend(images);
    folders.truncate(PICKER_ENTRIES);
    folders
}

/// The formats `shared::picture` can decode. Stated here rather than read from `[wallpaper] extensions`,
/// which is about what counts as a wallpaper — a user who narrows their wallpaper library to `png` has not
/// said anything about which pictures may be their face.
fn is_picture(path: &Path) -> bool {
    let Some(extension) = path.extension().map(|e| e.to_string_lossy().to_lowercase()) else {
        return false;
    };
    matches!(extension.as_str(), "png" | "jpg" | "jpeg" | "webp")
}

fn choice_tile(
    choice: Choice,
    folder: RwSignal<PathBuf>,
    picking: RwSignal<bool>,
    theme: NordTheme,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let picture: Box<dyn LayoutItem> = if choice.folder {
        icon_view(|| "folder".to_string(), move || theme.yellow, 28.0)?
    } else {
        ui::thumbnail::view(
            choice.path.clone(),
            PICKER_TILE,
            PICKER_TILE,
            6.0,
            "image",
            theme,
        )?
    };
    let name = choice.name.clone();
    let label = Text::auto(
        move || name.clone(),
        LayoutStyle::new().width(SizeDimension::Percent(1.0)),
        move || {
            theme
                .text_style(FontRole::Caption, theme.subtle)
                .with_max_lines(1)
                .with_ellipsis(true)
        },
    )?;

    let tile = StyledContainer::new(
        LayoutStyle::new()
            .flex_column()
            .gap(space::SM)
            .width(PICKER_TILE + 12.0)
            .padding_all(space::SM)
            .align_items(AlignItems::CENTER)
            .justify_content(JustifyContent::CENTER),
        paint::md(theme.base),
        vec![picture, box_item(label)],
    )?
    .hover_style(paint::md(theme.overlay))
    .on_press(move || {
        if choice.folder {
            folder.set(choice.path.clone());
            return;
        }
        set_avatar(&choice.path);
        picking.set(false);
    });
    Ok(Box::new(tile))
}

/// Writes the chosen picture to `[dashboard] avatar`, which the config watcher then applies — the same route
/// the settings panel's Save takes, so one path leads to a reload and one look for the change.
fn set_avatar(path: &Path) {
    let Some(config) = config::config() else {
        return;
    };
    let value = DashboardConfig {
        avatar: path.display().to_string(),
        ..config.dashboard.clone()
    };
    if let Err(e) = Config::save_section(&Config::default_path(), "dashboard", &value) {
        tracing::warn!("dashboard: could not save the avatar: {e}");
    }
}

/// The first image that exists, in the order a desktop conventionally writes them: the user's own `~/.face`
/// first, then whatever their display manager put in AccountsService.
/// Where the user's picture is: the `[dashboard] avatar` override, else the conventional places a desktop
/// keeps one. Shared with the lock screen so the two never disagree about whose face this is.
pub fn avatar_path(config: &DashboardConfig) -> Option<PathBuf> {
    let configured = config.avatar.trim();
    if !configured.is_empty() {
        let path = paths::expand_tilde(&PathBuf::from(configured));
        return path.exists().then_some(path);
    }
    let home = paths::home_dir()?;
    let candidates = [
        home.join(".face"),
        home.join(".face.icon"),
        PathBuf::from("/var/lib/AccountsService/icons").join(username()),
    ];
    candidates.into_iter().find(|path| path.exists())
}

fn username() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("LOGNAME"))
        .unwrap_or_else(|_| telar::t!("sysinfo.no_reading"))
}

fn hostname() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .ok()
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| telar::t!("sysinfo.no_reading"))
}

/// Seconds since boot, from `/proc/uptime`'s first field.
fn read_uptime() -> Option<u64> {
    let text = std::fs::read_to_string("/proc/uptime").ok()?;
    let seconds: f64 = text.split_whitespace().next()?.parse().ok()?;
    Some(seconds.max(0.0) as u64)
}

/// The coarsest two units that still say something: days and hours once a machine has been up a day, hours and
/// minutes below that. A seconds field on an uptime is noise that changes while you read it.
fn duration_label(seconds: u64) -> String {
    let (days, hours, minutes) = (
        seconds / 86_400,
        (seconds % 86_400) / 3_600,
        (seconds % 3_600) / 60,
    );
    if days > 0 {
        format!("{days}d {hours}h")
    } else if hours > 0 {
        format!("{hours}h {minutes}m")
    } else {
        format!("{minutes}m")
    }
}

fn first_of_month(date: NaiveDate) -> NaiveDate {
    date.with_day(1).unwrap_or(date)
}

/// The month `months` away, saturating at chrono's representable range rather than wrapping to another era.
fn shift_months(anchor: NaiveDate, months: i32) -> NaiveDate {
    let step = Months::new(months.unsigned_abs());
    let moved = if months < 0 {
        anchor.checked_sub_months(step)
    } else {
        anchor.checked_add_months(step)
    };
    moved.unwrap_or(anchor)
}

/// The date the grid's first cell shows: the configured first day of the week on or before the 1st.
fn grid_start(month: NaiveDate, first: Weekday) -> NaiveDate {
    // `+ 7` before the modulo: these indices are unsigned, so subtracting a later weekday from an earlier one
    // wraps to four billion and the grid starts on the wrong day rather than obviously breaking.
    let offset = (month.weekday().num_days_from_monday() + 7 - first.num_days_from_monday()) % 7;
    month
        .checked_sub_days(Days::new(offset as u64))
        .unwrap_or(month)
}

fn shift_weekday(first: Weekday, offset: u32) -> Weekday {
    let index = (first.num_days_from_monday() + offset) % 7;
    Weekday::try_from(index as u8).unwrap_or(Weekday::Mon)
}

/// One `t!` per weekday rather than a key built from the name: the macro checks its key against the catalogs at
/// compile time, and a computed key would opt out of that.
pub(super) fn weekday_label(day: Weekday) -> String {
    match day {
        Weekday::Mon => telar::t!("dashboard.weekday.mon"),
        Weekday::Tue => telar::t!("dashboard.weekday.tue"),
        Weekday::Wed => telar::t!("dashboard.weekday.wed"),
        Weekday::Thu => telar::t!("dashboard.weekday.thu"),
        Weekday::Fri => telar::t!("dashboard.weekday.fri"),
        Weekday::Sat => telar::t!("dashboard.weekday.sat"),
        Weekday::Sun => telar::t!("dashboard.weekday.sun"),
    }
}

fn month_label(month: u32) -> String {
    match month {
        1 => telar::t!("dashboard.month.january"),
        2 => telar::t!("dashboard.month.february"),
        3 => telar::t!("dashboard.month.march"),
        4 => telar::t!("dashboard.month.april"),
        5 => telar::t!("dashboard.month.may"),
        6 => telar::t!("dashboard.month.june"),
        7 => telar::t!("dashboard.month.july"),
        8 => telar::t!("dashboard.month.august"),
        9 => telar::t!("dashboard.month.september"),
        10 => telar::t!("dashboard.month.october"),
        11 => telar::t!("dashboard.month.november"),
        _ => telar::t!("dashboard.month.december"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).expect("a real date")
    }

    /// The avatar is a bitmap in a pressable box, which is two things that position themselves — so the one
    /// question worth measuring is whether it ends up inside the card at all.
    #[test]
    fn the_avatar_sits_inside_its_card() {
        use telar::{AvailableSpace, compute_layout, new_container, track_layout};

        telar::reset_layout_runtime();
        telar::set_theme(NordTheme::new());
        let theme = NordTheme::new();
        let card = user_card(&DashboardConfig::default(), theme).expect("the user card builds");
        let card_rect = track_layout(card.layout_node()).expect("the card registers its rect");

        let root = new_container(
            LayoutStyle::new()
                .flex_column()
                .padding_all(space::XL)
                .width(380.0)
                .height(900.0),
            &[card.layout_node()],
        )
        .expect("a drawer-shaped root");
        compute_layout(
            root,
            AvailableSpace::Definite(380.0),
            AvailableSpace::Definite(900.0),
        )
        .expect("layout");

        let card_rect = card_rect.get();
        assert!(
            card_rect.width > 0.0 && card_rect.height > 0.0,
            "the card measured {}x{}",
            card_rect.width,
            card_rect.height
        );
        assert!(
            card_rect.height < 900.0,
            "the collapsed picker must cost the card no height: {}",
            card_rect.height
        );
    }

    #[test]
    fn the_avatar_browser_lists_folders_first_and_only_pictures() {
        let dir = std::env::temp_dir().join(format!("hyprshell-avatar-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("zz-folder")).expect("a sub-folder");
        std::fs::create_dir_all(dir.join(".hidden-folder")).expect("a hidden sub-folder");
        for name in ["b.png", "a.JPG", "notes.txt", ".secret.png"] {
            std::fs::write(dir.join(name), b"x").expect("a file");
        }

        let listed = entries_in(&dir);
        assert_eq!(
            listed
                .iter()
                .map(|c| (c.name.as_str(), c.folder))
                .collect::<Vec<_>>(),
            vec![("zz-folder", true), ("a.JPG", false), ("b.png", false)],
            "folders first, then pictures, each alphabetical and case-insensitively so"
        );
        assert!(
            !listed.iter().any(|c| c.name.starts_with('.')),
            "a picture someone hid is not one they are looking for here"
        );
        assert!(
            !listed.iter().any(|c| c.name.ends_with(".txt")),
            "only what `shared::picture` can decode"
        );
        assert!(
            entries_in(&dir.join("no-such-folder")).is_empty(),
            "a folder that is not there lists nothing rather than failing the page"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_grid_starts_on_the_configured_first_day_on_or_before_the_first() {
        // 1 Jan 2026 is a Thursday.
        let january = date(2026, 1, 1);
        assert_eq!(
            grid_start(january, Weekday::Mon),
            date(2025, 12, 29),
            "a Monday-first grid backs up to the Monday of that week"
        );
        assert_eq!(
            grid_start(january, Weekday::Sun),
            date(2025, 12, 28),
            "a Sunday-first grid backs up one day further"
        );
        // A month that already starts on the configured day must not back up a whole week.
        let june = date(2025, 6, 1);
        assert_eq!(june.weekday(), Weekday::Sun);
        assert_eq!(grid_start(june, Weekday::Sun), june);
    }

    #[test]
    fn six_weeks_always_cover_the_month() {
        // February 2026 starts on a Sunday, which is the case a five-row grid truncates.
        for (year, month) in [(2026, 2), (2026, 1), (2024, 2), (2025, 8)] {
            let anchor = date(year, month, 1);
            let start = grid_start(anchor, Weekday::Mon);
            let last_cell = start
                .checked_add_days(Days::new((WEEKS * 7 - 1) as u64))
                .expect("in range");
            let last_of_month = shift_months(anchor, 1)
                .checked_sub_days(Days::new(1))
                .expect("in range");
            assert!(
                last_cell >= last_of_month,
                "{year}-{month:02} runs past the grid: {last_cell} < {last_of_month}"
            );
        }
    }

    #[test]
    fn the_weekday_header_starts_where_the_grid_does() {
        assert_eq!(shift_weekday(Weekday::Mon, 0), Weekday::Mon);
        assert_eq!(shift_weekday(Weekday::Mon, 6), Weekday::Sun);
        assert_eq!(shift_weekday(Weekday::Sun, 0), Weekday::Sun);
        assert_eq!(shift_weekday(Weekday::Sun, 1), Weekday::Mon);
        assert_eq!(
            shift_weekday(Weekday::Sat, 1),
            Weekday::Sun,
            "wraps the week"
        );
    }

    #[test]
    fn stepping_a_month_lands_on_the_first_and_survives_a_year_boundary() {
        assert_eq!(shift_months(date(2026, 1, 1), -1), date(2025, 12, 1));
        assert_eq!(shift_months(date(2025, 12, 1), 1), date(2026, 1, 1));
        // The case a naive month += 1 gets wrong.
        assert_eq!(shift_months(date(2026, 1, 31), 1), date(2026, 2, 28));
    }

    #[test]
    fn uptime_reads_in_the_two_units_that_matter() {
        assert_eq!(duration_label(0), "0m");
        assert_eq!(duration_label(90), "1m");
        assert_eq!(duration_label(3 * 3600 + 25 * 60), "3h 25m");
        assert_eq!(duration_label(2 * 86_400 + 5 * 3600), "2d 5h");
    }
}
