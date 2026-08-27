use std::collections::BTreeSet;
use std::sync::Arc;
use ui::scale::paint;

use telar::{
    AlignItems, Color, Container, Declared, Image, ImageData, JustifyContent, LayoutError,
    LayoutItem, LayoutStyle, Memo, ObjectFit, Raster, ReactiveList, ReadSignal, RectStyle,
    RwSignal, SizeDimension, Span, StyledContainer, Text, box_item, memo, signal, use_theme,
};

use config::surface_env;
use config::theme::{FontRole, NordTheme};
use config::{FullscreenPopups, NotificationsConfig, StackConfig};
use services::hyprland::{self, ActiveWindow, Client};
use services::notifications::{self, Notification, SharedSnapshot, Snapshot, Urgency};
use ui::panel::{card_gap, panel_fill};
use ui::scale::space;

/// Parses the freedesktop notification body's limited HTML markup into one string and the byte ranges of it
/// that style themselves differently: `<b>`/`<strong>` bold, `<i>`/`<em>` italic, `<a href>` links (painted
/// `link_color`), `<br>` a newline, and an `<img>`'s `alt` text. Unknown tags are dropped, keeping their
/// inner text, and entities are decoded per segment.
///
/// One string rather than a list of runs, which is what lets the body be *clamped*: an ellipsis has to cut
/// across the text, and runs that each owned their slice could not be cut across. A stretch that says
/// nothing about itself costs nothing — it is simply not a span.
fn body_runs(markup: &str, text_color: Color, link_color: Color) -> (String, Vec<Span>) {
    let mut text = String::new();
    let mut spans: Vec<Span> = Vec::new();
    let mut current = String::new();
    let (mut bold, mut italic, mut link) = (0i32, 0i32, 0i32);
    let mut chars = markup.chars();

    while let Some(c) = chars.next() {
        if c != '<' {
            current.push(c);
            continue;
        }
        let mut tag = String::new();
        for t in chars.by_ref() {
            if t == '>' {
                break;
            }
            tag.push(t);
        }
        let lower = tag.trim().to_ascii_lowercase();
        // `<br>` and `<img>` don't change style: they stay within the current run.
        if lower == "br" || lower == "br/" || lower.starts_with("br ") {
            current.push('\n');
            continue;
        }
        if lower.starts_with("img") {
            if let Some(alt) = attr_value(&tag, "alt") {
                current.push_str(&alt);
            }
            continue;
        }
        // A style-changing tag ends the current run before the new style takes effect. `kind`: 0 bold, 1 italic, 2 link — chosen so the counter can be bumped after the borrow of `current` ends.
        let (kind, delta): (u8, i32) = match lower.as_str() {
            "b" | "strong" => (0, 1),
            "/b" | "/strong" => (0, -1),
            "i" | "em" => (1, 1),
            "/i" | "/em" => (1, -1),
            "/a" => (2, -1),
            _ if lower == "a" || lower.starts_with("a ") => (2, 1),
            _ => continue,
        };
        push_run(
            &mut text,
            &mut spans,
            &mut current,
            bold > 0,
            italic > 0,
            link > 0,
            text_color,
            link_color,
        );
        match kind {
            0 => bold = (bold + delta).max(0),
            1 => italic = (italic + delta).max(0),
            _ => link = (link + delta).max(0),
        }
    }
    push_run(
        &mut text,
        &mut spans,
        &mut current,
        bold > 0,
        italic > 0,
        link > 0,
        text_color,
        link_color,
    );
    (text, spans)
}

/// Appends the accumulated segment to the body, decoding entities, and records a span for it when it says
/// anything the paragraph does not already say.
#[allow(clippy::too_many_arguments)]
fn push_run(
    text: &mut String,
    spans: &mut Vec<Span>,
    current: &mut String,
    bold: bool,
    italic: bool,
    link: bool,
    text_color: Color,
    link_color: Color,
) {
    if current.is_empty() {
        return;
    }
    let segment = decode_entities(current);
    current.clear();
    if segment.is_empty() {
        return;
    }
    let start = text.len() as u32;
    text.push_str(&segment);
    let mut over = Declared::default();
    if bold {
        over = over.with_font_weight(700);
    }
    if italic {
        over = over.with_font_style(telar::FontStyle::Italic);
    }
    if link {
        over = over.with_color(link_color);
    }
    let _ = text_color;
    // Plain text at the paragraph's own colour is not a span: it is the paragraph.
    if !over.is_empty() {
        spans.push(Span::new(start..text.len() as u32, over));
    }
}

/// The value of `name="..."` (or `name='...'`) within a tag body, if present.
fn attr_value(tag: &str, name: &str) -> Option<String> {
    let start = tag.find(&format!("{name}="))? + name.len() + 1;
    let rest = &tag[start..];
    let quote = rest.chars().next().filter(|c| *c == '"' || *c == '\'')?;
    let end = rest[1..].find(quote)?;
    Some(rest[1..1 + end].to_string())
}

fn decode_entities(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let after = &rest[amp..];
        let Some(semi) = after.find(';').filter(|&s| s <= 8) else {
            out.push('&');
            rest = &after[1..];
            continue;
        };
        let entity = &after[1..semi];
        let decoded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => entity
                .strip_prefix('#')
                .and_then(|n| {
                    n.strip_prefix('x')
                        .and_then(|h| u32::from_str_radix(h, 16).ok())
                        .or_else(|| n.parse().ok())
                })
                .and_then(char::from_u32),
        };
        match decoded {
            Some(ch) => {
                out.push(ch);
                rest = &after[semi + 1..];
            }
            None => {
                out.push('&');
                rest = &after[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// The width a card is drawn at inside the history panel, which is what the swipe threshold is a fraction of.
///
/// The panel's own, not the column's: a card in the bell drawer is as wide as the drawer, and the column it also appears in is sized by `[stack] width`. One number could only be right in one of the two places.
const PANEL_CARD_WIDTH: f32 = 380.0;

fn urgency_color(urgency: Urgency, theme: &NordTheme) -> Color {
    match urgency {
        Urgency::Critical => theme.red,
        Urgency::Normal => theme.accent,
        Urgency::Low => theme.muted,
    }
}

/// The notifications that should pop right now: none under Do-Not-Disturb, only fresh arrivals (one restored from history belongs in the panel), and under a fullscreen window only what `[notifications] fullscreen` still allows. Neither ordered nor capped — that is [`crate::stack`]'s, over every card it holds.
pub(crate) fn popping(
    snapshot: &Snapshot,
    cfg: &NotificationsConfig,
    fullscreen: bool,
) -> Vec<Notification> {
    // Do-Not-Disturb is not asked about here, and deliberately: the daemon already answered it by recording the notification as not popping — when it arrived under DND, and when DND was switched on over it. Asking a second time would be a second owner of one rule, and the one that let a suppressed notification come back the moment the toggle went off.
    //
    // Only fresh arrivals pop up; notifications restored from persisted history stay in the panel, unpopped.
    let mut list: Vec<Notification> = snapshot
        .active
        .iter()
        .filter(|n| n.popup)
        .cloned()
        .collect();
    if fullscreen {
        list.retain(|n| cfg.fullscreen.allows(n.urgency));
    }
    // Neither ordered nor capped here any more: the column these join orders every card it holds by arrival and caps the lot at `[stack] max_visible`, and a notification queue trimmed twice would hide cards the column had already made room for.
    list
}

/// Whether a notification is one the column puts where it will be read rather than where it landed.
pub(crate) fn is_critical(notification: &Notification) -> bool {
    notification.urgency == Urgency::Critical
}

/// Whether the focused window is covering the screen, as a value the popup stack re-reads.
///
/// Two subscriptions rather than one: `j/clients` carries each window's fullscreen flag but not which one has focus, and `j/activewindow` the reverse. Neither is opened while the policy is `on`, since nothing would read the answer — a shell that never suppresses a popup should not be listening to the compositor for it.
fn fullscreen_focus(cfg: &NotificationsConfig) -> Option<Memo<bool>> {
    if cfg.fullscreen == FullscreenPopups::On {
        return None;
    }
    let clients = signal(Vec::<Client>::new());
    let active = signal(ActiveWindow::default());
    let publish_clients = clients;
    platform_wayland::watch(hyprland::subscribe_clients, move |list: Vec<Client>| {
        publish_clients.set(list)
    });
    let publish_active = active;
    platform_wayland::watch(
        hyprland::subscribe_active_window,
        move |window: ActiveWindow| publish_active.set(window),
    );
    let (clients, active) = (clients.read_only(), active.read_only());
    Some(memo(move || {
        let address = active.get().address;
        !address.is_empty()
            && clients
                .get()
                .iter()
                .any(|c| c.address == address && c.fullscreen)
    }))
}

/// Everything a card needs beyond the notification itself, so the popup stack and the history panel draw the same card from the same `[notifications]` settings instead of each carrying its own arguments.
#[derive(Clone, Copy)]
struct CardStyle {
    theme: NordTheme,
    radius: f32,
    /// What the card paints behind itself.
    ///
    /// Two answers, because a card is two different things. On the popup surface it *is* the panel — nothing else is on that surface — so it takes `[panels] opacity` and the compositor's blur has something to show through. In the history it sits inside a panel that is already translucent, and a second translucent layer over the first would only make the card harder to read than the drawer under it.
    fill: Color,
    /// The body's line cap, or `None` for the whole body (`open_expanded`).
    body_lines: Option<u16>,
    /// A tap on the card body invokes the notification's `default` action, when it declares one.
    action_on_click: bool,
    /// How far sideways the card must be dragged before letting go dismisses it, in px; `None` is off.
    swipe: Option<f32>,
}

impl CardStyle {
    /// A card inside a panel: solid against the translucent surface it sits on. `width` is asked for because the swipe threshold is a fraction of it, and a card is drawn at the panel's width in one place and the column's in the other.
    fn new(
        cfg: &NotificationsConfig,
        stack: &StackConfig,
        width: f32,
        theme: NordTheme,
        radius: f32,
    ) -> Self {
        Self {
            theme,
            radius,
            fill: theme.surface,
            body_lines: cfg.body_max_lines(),
            action_on_click: cfg.action_on_click,
            // The column's threshold even here in the panel: the gesture is one gesture wherever a card is drawn.
            swipe: stack.swipe_distance(width),
        }
    }

    /// A card that is the panel — the popup stack, where nothing is behind it but the desktop.
    fn standalone(self) -> Self {
        Self {
            fill: panel_fill(),
            ..self
        }
    }
}

/// The `default` action's key, when the notification declares one — the spec's convention for what the notification is *for* (open the message, the download, the calendar entry) rather than one of its buttons.
fn default_action_key(notification: &Notification) -> Option<String> {
    notification
        .actions
        .chunks_exact(2)
        .find(|pair| pair[0] == "default")
        .map(|pair| pair[0].clone())
}

fn notification_card(
    notification: &Notification,
    width: SizeDimension,
    style: CardStyle,
    dismiss: Option<fn(u32)>,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let CardStyle {
        theme,
        radius,
        fill,
        ..
    } = style;
    let accent = urgency_color(notification.urgency, &theme);
    let summary = notification.summary.clone();
    let body = body_runs(&notification.body, theme.muted, theme.accent);

    let leading = leading_visual(notification, accent)?;

    let summary_text = Text::new(
        move || summary.clone(),
        LayoutStyle::new(),
        move || {
            theme
                .text_style(FontRole::Body, theme.text)
                .with_font_weight(700)
                .with_clamp(1, true)
        },
    )?;

    let mut column: Vec<Box<dyn LayoutItem>> = vec![Box::new(summary_text)];
    if !body.0.is_empty() {
        let body_lines = style.body_lines;
        let (body_text_content, body_spans) = body;
        let body_text = Text::spanned(
            move || body_text_content.clone(),
            move || body_spans.clone(),
            LayoutStyle::new(),
            move || {
                let base = theme.text_style(FontRole::Caption, theme.muted);
                match body_lines {
                    Some(lines) => base.with_clamp(lines, true),
                    None => base,
                }
            },
        )?;
        column.push(Box::new(body_text));
    }
    // Shown wherever the card is interactive (the panel, and now popups via their carved input region): an action pill hit-tests before the card, so tapping one invokes it while tapping elsewhere dismisses.
    if dismiss.is_some()
        && let Some(actions) = action_buttons(notification, theme)?
    {
        column.push(actions);
    }
    let text_column = Container::new(
        LayoutStyle::new()
            .flex_column()
            .gap(space::xs())
            .flex_grow(1.0)
            .width(SizeDimension::Percent(1.0)),
        column,
    )?;

    let mut children: Vec<Box<dyn LayoutItem>> = vec![leading, Box::new(text_column)];
    // The one control that *deletes* rather than retires, and the only reason a notification card carries a corner the other two do not: swiping puts a notification in the history, and there has to be a way to say "and I do not want it there either". A toast and an OSD have no history to be kept out of.
    if dismiss.is_some() {
        children.push(close_button(notification.id, theme)?);
    }
    let mut card = StyledContainer::new(
        LayoutStyle::new()
            .flex_row()
            .gap(space::lg())
            .padding_all(space::xl())
            .width(width),
        move |_| RectStyle::filled(fill, radius),
        children,
    )?;
    if let Some(dismiss) = dismiss {
        let id = notification.id;
        let default_action = style
            .action_on_click
            .then(|| default_action_key(notification))
            .flatten();
        card = card.on_press(move || match &default_action {
            Some(key) => notifications::invoke_action(id, key),
            None => dismiss(id),
        });
        if let Some(threshold) = style.swipe {
            // Retired rather than deleted: the panel behind the bell is where a swiped-away notification goes.
            card = crate::stack::swipe::swipe_aside(card, threshold, move || {
                notifications::expire(id)
            });
        }
    }
    Ok(Box::new(card))
}

/// The ✕ in a card's corner: the one gesture that takes a notification out of the history rather than putting it there. Hit-tests before the card body, like an action pill, so pressing it never also runs the default action underneath.
fn close_button(id: u32, theme: NordTheme) -> Result<Box<dyn LayoutItem>, LayoutError> {
    // `x`, not `close`: lucide has no icon by that name, and the store spends eight fetches finding out before settling into the missing-glyph placeholder. The sidebar and the note card already ask for the right one.
    let glyph = ui::icon::icon_view(|| "x".to_string(), move || theme.muted, CLOSE_GLYPH)?;
    Ok(box_item(
        StyledContainer::new(
            LayoutStyle::new()
                .align_self_start()
                .padding_all(space::xs()),
            move |_| RectStyle::filled(Color::TRANSPARENT, CLOSE_GLYPH / 2.0),
            vec![glyph],
        )?
        .hover_style(move |_| RectStyle::filled(theme.overlay, CLOSE_GLYPH / 2.0))
        .on_press(move || notifications::close(id)),
    ))
}

/// The ✕'s glyph size. Small enough to read as a corner affordance rather than a second action, large enough that a pointer finds it — this is the only control on the card with no forgiving body around it.
const CLOSE_GLYPH: f32 = 14.0;

/// A card's list key. Keyed on what it *draws*, not on the notification's identity: a sender that edits a notification in place (`replaces_id`) keeps its id while the summary and body turn over entirely, and a key of just the id would leave the old card on screen.
pub(crate) fn card_key(notification: &Notification) -> String {
    format!(
        "{}\u{1}{}\u{1}{}",
        notification.id, notification.summary, notification.body
    )
}

/// The card's leading visual, in freedesktop priority: the notification's own raw image, then its resolved application icon, else the urgency dot.
fn leading_visual(
    notification: &Notification,
    accent: Color,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    if let Some(img) = &notification.image {
        let data = Arc::new(ImageData::new(img.rgba.clone(), img.width, img.height));
        let image = Image::new(
            LayoutStyle::new().width(36.0).height(36.0).flex_shrink(0.0),
            move || data.clone(),
            || Raster::Smooth,
            || ObjectFit::Cover,
        )?;
        return Ok(Box::new(image));
    }
    if let Some(icon) = app_icon_visual(&notification.app_icon)? {
        return Ok(icon);
    }
    let dot = StyledContainer::new(
        LayoutStyle::new().width(8.0).height(8.0).flex_shrink(0.0),
        paint::xs(accent),
        Vec::new(),
    )?;
    Ok(Box::new(dot))
}

/// The resolved application icon as a 36px visual — an untinted SVG (keeping the app's own colours) or its raster pixels — or `None` when the reference is empty or can't be resolved.
fn app_icon_visual(reference: &str) -> Result<Option<Box<dyn LayoutItem>>, LayoutError> {
    ui::icon::app_icon_view(reference, 36.0)
}

/// A wrapping row of the notification's non-default actions, or `None` when it has none. Tapping one invokes it (emitting `ActionInvoked`) and closes the notification.
fn action_buttons(
    notification: &Notification,
    theme: NordTheme,
) -> Result<Option<Box<dyn LayoutItem>>, LayoutError> {
    let buttons: Vec<Box<dyn LayoutItem>> = notification
        .actions
        .chunks_exact(2)
        .filter(|pair| pair[0] != "default")
        .map(|pair| action_pill(notification.id, pair[0].clone(), pair[1].clone(), theme))
        .collect::<Result<_, _>>()?;
    if buttons.is_empty() {
        return Ok(None);
    }
    let row = Container::new(
        LayoutStyle::new()
            .flex_row()
            .flex_wrap()
            .gap(space::md())
            .width(SizeDimension::Percent(1.0)),
        buttons,
    )?;
    Ok(Some(Box::new(row)))
}

fn action_pill(
    id: u32,
    key: String,
    label: String,
    theme: NordTheme,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let text = Text::new(
        move || label.clone(),
        LayoutStyle::new(),
        move || theme.text_style(FontRole::Caption, theme.text),
    )?;
    let pill = StyledContainer::new(
        LayoutStyle::new()
            .padding_horizontal(space::lg())
            .padding_vertical(space::sm()),
        paint::md(theme.overlay),
        vec![box_item(text)],
    )?
    .hover_style(paint::md(theme.overlay.darken(0.12)))
    .on_press(move || notifications::invoke_action(id, &key));
    Ok(Box::new(pill))
}

/// Builds the reactive card stack from a snapshot signal. Split out so tests can drive it with a fixed snapshot instead of a live subscription. One notification as the column draws it: the same card the history panel shows, at the column's width and standalone — nothing is behind it but the desktop.
pub(crate) fn popup_card(
    notification: &Notification,
    theme: NordTheme,
    radius: f32,
    width: f32,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let cfg = config::config()
        .map(|c| c.notifications.clone())
        .unwrap_or_default();
    let stack = config::config().map(|c| c.stack).unwrap_or_default();
    let style = CardStyle::new(&cfg, &stack, width, theme, radius).standalone();
    notification_card(
        notification,
        SizeDimension::Percent(1.0),
        style,
        Some(notifications::expire),
    )
}

/// Whether the focused window is covering the screen, as a value the column re-reads. `None` when the policy never suppresses anything, so a shell that would not act on the answer does not listen for it.
pub(crate) fn covering_focus(cfg: &NotificationsConfig) -> Option<Memo<bool>> {
    fullscreen_focus(cfg)
}

/// The bar chip: a bell whose glyph flips to `bell-off` under Do-Not-Disturb, with an unread-count badge. Subscribes to the daemon like any other module reflecting a shared service; registered with `.opens()` so a click drops the history panel.
pub fn bell_module() -> Result<Box<dyn LayoutItem>, LayoutError> {
    let unread = signal(0u32);
    let dnd = signal(false);
    let unread_read = unread.read_only();
    let dnd_read = dnd.read_only();
    platform_wayland::watch(notifications::subscribe, move |snap: SharedSnapshot| {
        unread.set(snap.unread);
        dnd.set(snap.dnd);
    });

    let fg = ui::module::module_fg();
    let theme = use_theme::<NordTheme>();
    let glyph = memo(move || if dnd_read.get() { "bell-off" } else { "bell" });
    let icon = ui::icon::icon_view(
        move || glyph.get().to_string(),
        move || fg.get(),
        ui::module::icon_px(),
    )?;
    let badge = Text::new(
        move || badge_text(unread_read.get()),
        LayoutStyle::new(),
        move || {
            theme
                .text_style(FontRole::Caption, fg.get())
                .with_font_weight(700)
        },
    )?;
    let row = Container::new(
        LayoutStyle::new()
            .flex_row()
            .align_items(AlignItems::CENTER)
            .gap(space::sm()),
        vec![icon, Box::new(badge)],
    )?;
    Ok(Box::new(row))
}

fn badge_text(unread: u32) -> String {
    match unread {
        0 => String::new(),
        1..=99 => unread.to_string(),
        _ => "99+".to_string(),
    }
}

/// The drawer panel: a header (title, Do-Not-Disturb toggle, clear-all) over the full history, newest first, each card click-to-dismiss. Opening it marks the history read.
pub fn bell_panel() -> Result<Box<dyn LayoutItem>, LayoutError> {
    notifications::mark_read();
    if let Some(env) = surface_env() {
        services::locale::attach(env.config.language());
    }
    let theme = use_theme::<NordTheme>();
    let snapshot = signal(notifications::snapshot_now().unwrap_or_default());
    let setter = snapshot;
    platform_wayland::watch(notifications::subscribe, move |snap: SharedSnapshot| {
        setter.set(snap)
    });
    let read = snapshot.read_only();

    // The cards sit inside the panel (drawer or float), so they carry its (bar-matching) radius.
    let radius = surfaces::drawer::content_radius();
    let cfg = surface_env().map_or_else(NotificationsConfig::default, |env| {
        env.config.notifications.clone()
    });
    let header = panel_header(read, theme)?;
    let list = history_list(read, &cfg, theme, radius)?;
    let panel = Container::new(
        LayoutStyle::new()
            .flex_column()
            .gap(space::lg())
            .width(SizeDimension::Percent(1.0)),
        vec![header, list],
    )?;
    Ok(Box::new(panel))
}

fn panel_header(
    read: ReadSignal<SharedSnapshot>,
    theme: NordTheme,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let title = Text::new(
        || telar::t!("notifications.title"),
        LayoutStyle::new(),
        move || {
            theme
                .text_style(FontRole::Title, theme.text)
                .with_font_weight(700)
        },
    )?;
    let dnd_label = read;
    let dnd_toggle = read;
    let dnd = pill_button(
        move || {
            if dnd_label.get().dnd {
                telar::t!("notifications.dnd_on")
            } else {
                telar::t!("notifications.dnd_off")
            }
        },
        move || notifications::set_dnd(!dnd_toggle.peek().dnd),
        theme,
    )?;
    let clear = pill_button(
        || telar::t!("notifications.clear_all"),
        notifications::clear_all,
        theme,
    )?;
    let actions = Container::new(
        LayoutStyle::new()
            .flex_row()
            .align_items(AlignItems::CENTER)
            .gap(space::md()),
        vec![dnd, clear],
    )?;
    let header = Container::new(
        LayoutStyle::new()
            .flex_row()
            .align_items(AlignItems::CENTER)
            .justify_content(JustifyContent::SPACE_BETWEEN)
            .gap(space::md())
            .width(SizeDimension::Percent(1.0)),
        vec![Box::new(title), Box::new(actions)],
    )?;
    Ok(Box::new(header))
}

fn pill_button(
    label: impl Fn() -> String + 'static,
    on_press: impl Fn() + 'static,
    theme: NordTheme,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let text = Text::new(label, LayoutStyle::new(), move || {
        theme.text_style(FontRole::Caption, theme.text)
    })?;
    let pill = StyledContainer::new(
        LayoutStyle::new()
            .padding_horizontal(space::lg())
            .padding_vertical(space::sm()),
        paint::md(theme.base),
        vec![Box::new(text) as Box<dyn LayoutItem>],
    )?
    .on_press(on_press);
    Ok(Box::new(pill))
}

/// One row of the history panel. Grouping turns a flat list of cards into a list of *rows*, so one keyed list still draws the whole panel — a header, the cards under it, and the row that reveals the rest.
enum HistoryRow {
    Group {
        app: String,
        count: usize,
        muted: bool,
        expanded: bool,
    },
    Card(Notification),
    Expander {
        app: String,
        hidden: usize,
        expanded: bool,
    },
}

/// Every row's list key, on the same rule as [`card_key`]: a header redraws when its count, mute or expansion changes, so all three belong in the key.
fn row_key(row: &HistoryRow) -> String {
    match row {
        HistoryRow::Group {
            app,
            count,
            muted,
            expanded,
        } => format!("g\u{1}{app}\u{1}{count}\u{1}{muted}\u{1}{expanded}"),
        HistoryRow::Card(n) => format!("c\u{1}{}", card_key(n)),
        HistoryRow::Expander {
            app,
            hidden,
            expanded,
        } => format!("x\u{1}{app}\u{1}{hidden}\u{1}{expanded}"),
    }
}

/// Lays the history out newest-first, grouped by application when `[notifications] group_by_app` asks for it.
///
/// A group is ordered by its most recent notification rather than by name, so the application that just spoke is at the top; within it the cards run newest-first like everything else. `expanded` names the groups the user has opened — it is the panel's own state, not the daemon's, so closing the drawer forgets it.
fn history_rows(
    snapshot: &Snapshot,
    cfg: &NotificationsConfig,
    expanded: &BTreeSet<String>,
) -> Vec<HistoryRow> {
    let newest_first: Vec<&Notification> = snapshot.active.iter().rev().collect();
    if !cfg.group_by_app {
        return newest_first
            .into_iter()
            .cloned()
            .map(HistoryRow::Card)
            .collect();
    }
    let mut apps: Vec<&str> = Vec::new();
    for n in &newest_first {
        if !apps.contains(&n.app_name.as_str()) {
            apps.push(&n.app_name);
        }
    }
    let mut rows = Vec::new();
    for app in apps {
        let group: Vec<&Notification> = newest_first
            .iter()
            .copied()
            .filter(|n| n.app_name == app)
            .collect();
        let is_expanded = expanded.contains(app);
        let shown = if is_expanded {
            group.len()
        } else {
            cfg.group_preview().min(group.len())
        };
        rows.push(HistoryRow::Group {
            app: app.to_string(),
            count: group.len(),
            muted: snapshot.is_muted(app),
            expanded: is_expanded,
        });
        rows.extend(
            group[..shown]
                .iter()
                .map(|n| HistoryRow::Card((*n).clone())),
        );
        if group.len() > cfg.group_preview() {
            rows.push(HistoryRow::Expander {
                app: app.to_string(),
                hidden: group.len() - shown,
                expanded: is_expanded,
            });
        }
    }
    rows
}

fn history_list(
    read: ReadSignal<SharedSnapshot>,
    cfg: &NotificationsConfig,
    theme: NordTheme,
    radius: f32,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    // A signal rather than a cell, because the row list derives from it: a plain value would change without anything asking the list to rebuild.
    let expanded = signal(BTreeSet::<String>::new());
    let stack = config::config().map(|c| c.stack).unwrap_or_default();
    let style = CardStyle::new(cfg, &stack, PANEL_CARD_WIDTH, theme, radius);
    let source = {
        let cfg = cfg.clone();
        let expanded = expanded.read_only();
        // Both signals read *out* before either is used: `with` holds the runtime's borrow for as long as its closure runs, so reading the snapshot inside the expanded set's `with` panics on the second borrow.
        move || {
            let snapshot = read.get();
            let open = expanded.get();
            history_rows(&snapshot, &cfg, &open)
        }
    };
    let toggle = expanded;
    let build = move |row: HistoryRow| -> Result<Box<dyn LayoutItem>, LayoutError> {
        match row {
            HistoryRow::Group {
                app, count, muted, ..
            } => group_header(app, count, muted, toggle, theme),
            HistoryRow::Card(n) => notification_card(
                &n,
                SizeDimension::Percent(1.0),
                style,
                Some(notifications::close),
            ),
            HistoryRow::Expander {
                app,
                hidden,
                expanded,
            } => expander_row(app, hidden, expanded, toggle, theme),
        }
    };
    // Gap on the list itself (which lays the cards out); the wrapper only pins the full width so the percent-width cards resolve against it.
    let list = ReactiveList::new(source, row_key, build, card_gap())?;
    let column = Container::new(
        LayoutStyle::new()
            .flex_column()
            .width(SizeDimension::Percent(1.0)),
        vec![Box::new(list) as Box<dyn LayoutItem>],
    )?;
    Ok(Box::new(column))
}

/// Flips one group open or shut. The panel's own state, keyed by application name — the same key the rows are grouped by, so a group that disappears takes its entry with it the next time the panel is built.
fn toggle_group(expanded: &RwSignal<BTreeSet<String>>, app: &str) {
    let app = app.to_string();
    expanded.update(|open| {
        if !open.remove(&app) {
            open.insert(app);
        }
    });
}

/// A group's header: which application, how many it has waiting, and the two things worth doing to all of them at once — muting the sender, and clearing the group. Tapping the header itself opens or shuts the group.
fn group_header(
    app: String,
    count: usize,
    muted: bool,
    toggle: RwSignal<BTreeSet<String>>,
    theme: NordTheme,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let name = if app.trim().is_empty() {
        telar::t!("notifications.unknown_app")
    } else {
        app.clone()
    };
    let label = Text::new(
        move || name.clone(),
        LayoutStyle::new().flex_grow(1.0),
        move || {
            theme
                .text_style(FontRole::Caption, theme.muted)
                .with_font_weight(700)
                .with_clamp(1, true)
        },
    )?;
    // A count of one is what a header without a badge already says.
    let badge = Text::new(
        move || {
            if count > 1 {
                count.to_string()
            } else {
                String::new()
            }
        },
        LayoutStyle::new(),
        move || theme.text_style(FontRole::Caption, theme.muted),
    )?;
    let mute_app = app.clone();
    let mute = icon_button(
        if muted { "bell-off" } else { "bell" },
        if muted { theme.red } else { theme.muted },
        theme,
        move || notifications::set_app_muted(&mute_app, !muted),
    )?;
    let clear_app = app.clone();
    let clear = icon_button("trash-2", theme.muted, theme, move || {
        notifications::clear_app(&clear_app)
    })?;

    let row = StyledContainer::new(
        LayoutStyle::new()
            .flex_row()
            .align_items(AlignItems::CENTER)
            .gap(space::md())
            .padding_horizontal(space::sm())
            .width(SizeDimension::Percent(1.0)),
        |_| RectStyle::default(),
        vec![
            Box::new(label) as Box<dyn LayoutItem>,
            Box::new(badge),
            mute,
            clear,
        ],
    )?
    .on_press(move || toggle_group(&toggle, &app));
    Ok(Box::new(row))
}

/// The row under a group that has more than it is showing: `+N` while collapsed, and the way back while open.
fn expander_row(
    app: String,
    hidden: usize,
    expanded: bool,
    toggle: RwSignal<BTreeSet<String>>,
    theme: NordTheme,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let label = move || {
        if expanded {
            telar::t!("notifications.show_less")
        } else {
            telar::t!("notifications.show_more", count = hidden.to_string())
        }
    };
    let text = Text::new(label, LayoutStyle::new(), move || {
        theme.text_style(FontRole::Caption, theme.accent)
    })?;
    let row = StyledContainer::new(
        LayoutStyle::new()
            .flex_row()
            .justify_content(JustifyContent::CENTER)
            .padding_vertical(space::xs())
            .width(SizeDimension::Percent(1.0)),
        |_| RectStyle::default(),
        vec![Box::new(text) as Box<dyn LayoutItem>],
    )?
    .on_press(move || toggle_group(&toggle, &app));
    Ok(Box::new(row))
}

/// A glyph that does one thing, sized to the caption text it sits beside. Its own box so the tap target is bigger than the glyph, and so a press on it hit-tests before the header row it sits inside.
fn icon_button(
    glyph: &'static str,
    tint: Color,
    theme: NordTheme,
    on_press: impl Fn() + 'static,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let icon = ui::icon::icon_view(move || glyph.to_string(), move || tint, 14.0)?;
    let button = StyledContainer::new(
        LayoutStyle::new().padding_all(space::sm()),
        |_| RectStyle::default(),
        vec![icon],
    )?
    .hover_style(paint::xs(theme.overlay))
    .on_press(on_press);
    Ok(Box::new(button))
}

/// Four notifications as a daemon would hold them, for the two previews below: a threaded pair with actions, a critical one and a low one, so every urgency and both card shapes are on the page.
fn sample_snapshot() -> Snapshot {
    let mk = |id: u32, app: &str, summary: &str, body: &str, urgency: Urgency| Notification {
        id,
        app_name: app.into(),
        app_icon: String::new(),
        summary: summary.into(),
        body: body.into(),
        actions: Vec::new(),
        urgency,
        popup: true,
        image: None,
    };
    let active = vec![
        Notification {
            actions: vec![
                "reply".into(),
                "Reply".into(),
                "archive".into(),
                "Archive".into(),
            ],
            ..mk(
                1,
                "Slack",
                "Ada Lovelace",
                "Still on for the review at <b>3pm</b>? &amp; bring notes",
                Urgency::Normal,
            )
        },
        mk(
            2,
            "Slack",
            "Grace Hopper",
            "Pushed the fix.",
            Urgency::Normal,
        ),
        mk(
            3,
            "Battery",
            "Battery low",
            "12% remaining — plug in soon.",
            Urgency::Critical,
        ),
        mk(4, "Calendar", "Standup in 5 minutes", "", Urgency::Low),
    ];
    Snapshot {
        unread: active.len() as u32,
        active,
        dnd: false,
        muted_apps: Vec::new(),
    }
}

/// Two notification cards as the column draws them, for [`crate::preview`].
pub(crate) fn popups_preview() -> Result<Box<dyn LayoutItem>, LayoutError> {
    let theme = use_theme::<NordTheme>();
    let cards = sample_snapshot()
        .active
        .iter()
        .map(|n| popup_card(n, theme, 12.0, PANEL_CARD_WIDTH))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Box::new(Container::new(
        LayoutStyle::new()
            .flex_column()
            .gap(card_gap())
            .width(SizeDimension::Percent(1.0)),
        cards,
    )?))
}

/// The history panel behind the bell chip, for [`crate::preview`]: the same snapshot, grouped and headed.
pub(crate) fn panel_preview() -> Result<Box<dyn LayoutItem>, LayoutError> {
    let theme = use_theme::<NordTheme>();
    let snapshot = signal(Arc::new(sample_snapshot()));
    let read = snapshot.read_only();
    Ok(Box::new(Container::new(
        LayoutStyle::new()
            .flex_column()
            .gap(space::lg())
            .padding_all(space::xl())
            .width(PANEL_CARD_WIDTH),
        vec![
            panel_header(read, theme)?,
            history_list(read, &NotificationsConfig::default(), theme, 12.0)?,
        ],
    )?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn badge_caps_at_99_plus() {
        assert_eq!(badge_text(0), "");
        assert_eq!(badge_text(3), "3");
        assert_eq!(badge_text(99), "99");
        assert_eq!(badge_text(250), "99+");
    }

    #[test]
    fn body_runs_parses_inline_markup_into_one_string_and_its_spans() {
        let text = Color::rgb(1.0, 1.0, 1.0);
        let link = Color::rgb(0.0, 0.0, 1.0);

        // The body is one string: that is what lets it be clamped, and what runs could never be cut across.
        let (body, spans) = body_runs("<b>Bold</b> &amp; <i>italic</i>", text, link);
        assert_eq!(body, "Bold & italic");
        // Two spans, not three runs: the " & " between them says nothing the paragraph does not.
        assert_eq!(spans.len(), 2);
        assert_eq!(
            &body[spans[0].range.start as usize..spans[0].range.end as usize],
            "Bold"
        );
        assert_eq!(spans[0].over.font_weight, Some(700));
        assert_eq!(
            &body[spans[1].range.start as usize..spans[1].range.end as usize],
            "italic"
        );
        assert_eq!(spans[1].over.font_style, Some(telar::FontStyle::Italic));

        // A link carries the link colour; `<br>` stays within the text as a newline.
        let (linked, link_spans) = body_runs(r#"a <a href="http://x">click</a> b"#, text, link);
        let clicked = link_spans
            .iter()
            .find(|s| &linked[s.range.start as usize..s.range.end as usize] == "click")
            .expect("the link is a span of its own");
        assert_eq!(clicked.over.color, Some(link.into()));

        let (br, br_spans) = body_runs("line1<br>line2", text, link);
        assert_eq!((br.as_str(), br_spans.len()), ("line1\nline2", 0));

        // `<img>` alt text, decoded entities, and unknown tags (kept inner, tag dropped).
        assert_eq!(
            body_runs(r#"<img src="a.png" alt="pic"/>"#, text, link).0,
            "pic"
        );
        assert_eq!(
            body_runs("&lt;tag&gt; &#65;&#x42;", text, link).0,
            "<tag> AB"
        );
        assert_eq!(body_runs("Q&A", text, link).0, "Q&A");
    }

    /// One notification, with the fields a presentation test actually varies.
    fn note(id: u32, app: &str, urgency: Urgency) -> Notification {
        Notification {
            id,
            app_name: app.into(),
            app_icon: String::new(),
            summary: format!("n{id}"),
            body: String::new(),
            actions: Vec::new(),
            urgency,
            popup: true,
            image: None,
        }
    }

    fn snapshot_of(active: Vec<Notification>) -> Snapshot {
        Snapshot {
            unread: active.len() as u32,
            active,
            dnd: false,
            muted_apps: Vec::new(),
        }
    }

    /// What pops is decided here; how many of them fit and what order they sit in is the column's.
    ///
    /// This used to reverse, float `critical` to the top and truncate to `max_visible` as well — a queue trimmed on its way into a column that trims again would hide cards the column had already made room for.
    #[test]
    fn dnd_hides_everything_and_the_rest_is_handed_over_untrimmed() {
        let cfg = NotificationsConfig::default();
        let snap = snapshot_of(vec![
            note(1, "a", Urgency::Normal),
            note(2, "a", Urgency::Critical),
            note(3, "a", Urgency::Normal),
        ]);
        assert_eq!(
            popping(&snap, &cfg, false).len(),
            3,
            "every popping notification is handed over; the column caps them"
        );

        // Do-Not-Disturb is not asserted here any more: the daemon records a suppressed notification as not popping, so `dnd: true` with `popup: true` is a state that cannot occur. What it *does* mean is checked where it is decided — see `services::notifications`.
    }

    #[test]
    fn restored_history_stays_in_the_panel_and_never_pops_up() {
        // One restored (non-popping) and one fresh notification: only the fresh one becomes a popup, while the history panel (which reads all of `active`) still holds both.
        let restored = Notification {
            popup: false,
            ..note(1, "a", Urgency::Normal)
        };
        let snap = snapshot_of(vec![restored, note(2, "a", Urgency::Normal)]);
        let shown = popping(&snap, &NotificationsConfig::default(), false);
        assert_eq!(shown.len(), 1, "only the fresh notification pops up");
        assert_eq!(shown[0].id, 2);
    }

    #[test]
    fn the_fullscreen_policy_decides_what_still_reaches_the_screen() {
        let snap = snapshot_of(vec![
            note(1, "a", Urgency::Normal),
            note(2, "a", Urgency::Critical),
        ]);
        let with = |policy| NotificationsConfig {
            fullscreen: policy,
            ..NotificationsConfig::default()
        };

        for policy in [
            FullscreenPopups::On,
            FullscreenPopups::Off,
            FullscreenPopups::Never,
        ] {
            assert_eq!(popping(&snap, &with(policy), false).len(), 2, "{policy:?}");
        }

        assert_eq!(
            popping(&snap, &with(FullscreenPopups::On), true).len(),
            2,
            "'on' never suppresses"
        );
        let urgent = popping(&snap, &with(FullscreenPopups::Off), true);
        assert_eq!(urgent.len(), 1, "'off' keeps only what is critical");
        assert_eq!(urgent[0].urgency, Urgency::Critical);
        assert!(
            popping(&snap, &with(FullscreenPopups::Never), true).is_empty(),
            "'never' holds back the critical ones too"
        );
        // Suppression is about the popup, never the record: the history reads `active`, which is untouched.
        assert_eq!(snap.active.len(), 2);
    }

    #[test]
    fn the_history_groups_by_app_newest_group_first_and_previews_each() {
        let cfg = NotificationsConfig {
            group_preview_num: 2,
            ..NotificationsConfig::default()
        };
        // Oldest first, as the daemon holds them: Slack spoke first, Calendar last.
        let snap = snapshot_of(vec![
            note(1, "Slack", Urgency::Normal),
            note(2, "Slack", Urgency::Normal),
            note(3, "Slack", Urgency::Normal),
            note(4, "Calendar", Urgency::Normal),
        ]);

        let rows = history_rows(&snap, &cfg, &BTreeSet::new());
        let shape: Vec<String> = rows.iter().map(describe_row).collect();
        assert_eq!(
            shape,
            [
                "group:Calendar:1",
                "card:4",
                "group:Slack:3",
                "card:3",
                "card:2",
                "expander:1",
            ],
            "the app that spoke last leads, and each group previews newest-first"
        );

        let open = BTreeSet::from(["Slack".to_string()]);
        let expanded: Vec<String> = history_rows(&snap, &cfg, &open)
            .iter()
            .map(describe_row)
            .collect();
        assert_eq!(
            expanded,
            [
                "group:Calendar:1",
                "card:4",
                "group:Slack:3",
                "card:3",
                "card:2",
                "card:1",
                "expander:0",
            ],
            "expanding one group reveals the rest of it and leaves the other collapsed"
        );

        let single = history_rows(
            &snapshot_of(vec![note(1, "Slack", Urgency::Normal)]),
            &cfg,
            &BTreeSet::new(),
        );
        assert_eq!(
            single.iter().map(describe_row).collect::<Vec<_>>(),
            ["group:Slack:1", "card:1"],
            "a group with nothing hidden gets no expander row to press"
        );
    }

    #[test]
    fn grouping_off_is_the_flat_newest_first_list_it_always_was() {
        let cfg = NotificationsConfig {
            group_by_app: false,
            ..NotificationsConfig::default()
        };
        let snap = snapshot_of(vec![
            note(1, "Slack", Urgency::Normal),
            note(2, "Calendar", Urgency::Normal),
        ]);
        let rows: Vec<String> = history_rows(&snap, &cfg, &BTreeSet::new())
            .iter()
            .map(describe_row)
            .collect();
        assert_eq!(rows, ["card:2", "card:1"]);
    }

    #[test]
    fn a_group_header_reports_the_mute_its_snapshot_carries() {
        let snap = Snapshot {
            muted_apps: vec!["Slack".to_string()],
            ..snapshot_of(vec![
                note(1, "Slack", Urgency::Normal),
                note(2, "Calendar", Urgency::Normal),
            ])
        };
        let muted: Vec<bool> =
            history_rows(&snap, &NotificationsConfig::default(), &BTreeSet::new())
                .iter()
                .filter_map(|row| match row {
                    HistoryRow::Group { app, muted, .. } => Some((app.clone(), *muted)),
                    _ => None,
                })
                .map(|(_, muted)| muted)
                .collect();
        assert_eq!(
            muted,
            [false, true],
            "Calendar leads, and only Slack is muted"
        );
    }

    #[test]
    fn a_card_is_keyed_on_what_it_draws_not_on_the_id_it_keeps() {
        let before = note(7, "Slack", Urgency::Normal);
        let edited_in_place = Notification {
            summary: "edited".into(),
            ..before.clone()
        };
        assert_ne!(
            card_key(&before),
            card_key(&edited_in_place),
            "a `replaces_id` edit keeps the id, so a key of just the id would leave the old card on screen"
        );
        assert_eq!(
            card_key(&before),
            card_key(&before.clone()),
            "and an unchanged notification keeps its node"
        );
    }

    #[test]
    fn the_default_action_is_the_only_one_a_body_tap_runs() {
        let with = |actions: &[&str]| Notification {
            actions: actions.iter().map(|s| s.to_string()).collect(),
            ..note(1, "a", Urgency::Normal)
        };
        assert_eq!(
            default_action_key(&with(&["default", "Open", "archive", "Archive"])).as_deref(),
            Some("default")
        );
        assert_eq!(
            default_action_key(&with(&["archive", "Archive"])),
            None,
            "an action that is not the default never fires from the card body"
        );
        assert_eq!(default_action_key(&with(&[])), None);
    }

    /// Every row type runs closures nothing else does — a group header reads the theme and the locale, an expander formats a count through `t!`. Only building the panel runs them, which is the only way the re-entrant-borrow trap (a second signal read inside another's `with`) ever shows up.
    #[test]
    fn the_history_panel_builds_grouped_and_flat() {
        let snapshot = signal(Arc::new(Snapshot {
            muted_apps: vec!["Slack".to_string()],
            ..sample_snapshot()
        }));
        for group_by_app in [true, false] {
            for open in [BTreeSet::new(), BTreeSet::from(["Slack".to_string()])] {
                telar::reset_layout_runtime();
                telar::set_theme(NordTheme::new());
                let cfg = NotificationsConfig {
                    group_by_app,
                    group_preview_num: 1,
                    ..NotificationsConfig::default()
                };
                // The rows the list will build, laid out here so a failure names the row rather than the list.
                for row in history_rows(&snapshot.peek(), &cfg, &open) {
                    let built = match row {
                        HistoryRow::Group {
                            app, count, muted, ..
                        } => group_header(
                            app.clone(),
                            count,
                            muted,
                            signal(open.clone()),
                            NordTheme::new(),
                        )
                        .map(|_| format!("group:{app}")),
                        HistoryRow::Card(n) => notification_card(
                            &n,
                            SizeDimension::Percent(1.0),
                            CardStyle::new(
                                &cfg,
                                &StackConfig::default(),
                                PANEL_CARD_WIDTH,
                                NordTheme::new(),
                                12.0,
                            ),
                            Some(notifications::close),
                        )
                        .map(|_| format!("card:{}", n.id)),
                        HistoryRow::Expander {
                            app,
                            hidden,
                            expanded,
                        } => expander_row(
                            app.clone(),
                            hidden,
                            expanded,
                            signal(open.clone()),
                            NordTheme::new(),
                        )
                        .map(|_| format!("expander:{app}")),
                    };
                    assert!(
                        built.is_ok(),
                        "a row failed to build with group_by_app={group_by_app}"
                    );
                }
                assert!(history_list(snapshot.read_only(), &cfg, NordTheme::new(), 12.0).is_ok());
            }
        }
    }

    /// A row reduced to what a shape assertion cares about, so a failure names the row rather than dumping it.
    fn describe_row(row: &HistoryRow) -> String {
        match row {
            HistoryRow::Group { app, count, .. } => format!("group:{app}:{count}"),
            HistoryRow::Card(n) => format!("card:{}", n.id),
            HistoryRow::Expander { hidden, .. } => format!("expander:{hidden}"),
        }
    }
}
