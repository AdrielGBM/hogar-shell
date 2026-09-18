//! The form toolkit every settings section is built out of.
//!
//! A section is a heading, a column of fields, and one Save button. This is that vocabulary — the widgets, the write-back to `config.toml`, and the recorder that tells a button whether anything under it moved — so the sections themselves are a description of *which* fields they have rather than of how a field behaves.

use std::path::{Path, PathBuf};
use std::rc::Rc;
use ui::scale::{paint, space};

use serde::Serialize;
use telar::{
    AlignItems, Container, Input, LayoutError, LayoutItem, LayoutStyle, RwSignal, SizeDimension,
    StyledContainer, Text, box_item, signal,
};

use config::theme::{FontRole, NordTheme};
use config::{
    Align, Capitalize, Config, Edge, FullscreenPopups, MediaScroll, OpenMode, Shape,
    TemperatureUnit, Variant,
};

use crate::panel::MODULE;

pub(crate) const EDGES: &[&str] = &["top", "bottom", "left", "right"];
pub(crate) const ALIGNS: &[&str] = &["start", "center", "end"];
pub(crate) const SHAPES: &[&str] = &["bar", "sections", "chips"];
pub(crate) const LANGUAGES: &[&str] = &["en", "es"];
pub(crate) const MEDIA_SCROLLS: &[&str] = &["volume", "track", "seek", "none"];
pub(crate) const CAPITALIZATIONS: &[&str] = &["none", "upper", "lower", "title"];
pub(crate) const TEMPERATURE_UNITS: &[&str] = &["celsius", "fahrenheit"];
pub(crate) const WEEKDAYS: &[&str] = &["monday", "sunday", "saturday"];
pub(crate) const FULLSCREEN_POPUPS: &[&str] = &["on", "off", "never"];
pub(crate) const MODES: &[&str] = &["auto", "dark", "light"];
pub(crate) const VARIANTS: &[&str] = &["vibrant", "content", "expressive", "fidelity", "muted"];
pub(crate) const TRANSITIONS: &[&str] = &["fade", "wipe", "none"];
pub(crate) const SHOT_BACKENDS: &[&str] = &["auto", "image-copy-capture", "screencopy"];
pub(crate) const RECORDER_BACKENDS: &[&str] = &["auto", "wf-recorder", "gpu-screen-recorder"];
pub(crate) const CURVES: &[&str] = &["gentle", "snappy", "bouncy"];
pub(crate) const VARIANT_STYLES: &[&str] = &["default", "filled"];
pub(crate) const OPEN_MODES: &[&str] = &["drawer", "float"];
pub(crate) const EASINGS: &[&str] = &["linear", "ease-in", "ease-out", "ease-in-out"];
pub(crate) const PLACEMENTS: &[&str] = &[
    "center",
    "top_left",
    "top_center",
    "top_right",
    "center_left",
    "center_right",
    "bottom_left",
    "bottom_center",
    "bottom_right",
];
/// K14, the recorder half: every field the form helpers build, so a section knows when one of them moved.
///
/// A thread-local rather than a parameter because the alternative is threading a tracker through all forty `*_section` functions and every `text_field`/`toggle_field`/`enum_field` call inside them. The forms are built one at a time on the driver thread, and each ends with exactly one [`save_button`] — which is where the recording is drained. That is the whole contract: **a form's fields must be built before its button.**
///
/// Each entry is an effect that bumps `revision` when its field changes, plus the revision itself. Effects are handed to the button so they live exactly as long as the form does.
struct FormRecorder {
    revision: RwSignal<u64>,
    subscriptions: Vec<telar::Effect>,
}

thread_local! {
    static RECORDING: std::cell::RefCell<Option<FormRecorder>> = const { std::cell::RefCell::new(None) };
}

/// How long after the last keystroke a live-preview form applies itself. Long enough that typing a font name is one apply rather than nine, short enough to read as a preview rather than as a delay.
const LIVE_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(700);

/// Hands `effect` to the form being built, which keeps it alive until its button is gone.
fn park(build: impl FnOnce(RwSignal<u64>) -> telar::Effect) {
    RECORDING.with(|recording| {
        let mut recording = recording.borrow_mut();
        let recorder = recording.get_or_insert_with(|| FormRecorder {
            revision: signal(0u64),
            subscriptions: Vec::new(),
        });
        let effect = build(recorder.revision);
        recorder.subscriptions.push(effect);
    });
}

/// Registers `value` as one of the current form's fields. Called by every field helper.
pub(crate) fn record_field<T: Clone + PartialEq + 'static>(value: &RwSignal<T>) {
    let watched = value.read_only();
    park(move |revision| {
        // An effect fires once when it is registered, and that first run is the field being *seeded* — not a user changing anything. Reporting it would make every form apply itself the moment it was drawn.
        let seeded = std::cell::Cell::new(false);
        telar::effect(move || {
            let _ = watched.get();
            if seeded.replace(true) {
                revision.set(revision.peek() + 1);
            }
        })
    });
}

/// Binds a form's `String` field to the index the catalogue's `select` speaks in, and records it.
///
/// The sections speak in the value they write to `config.toml`, the widget in positions. The two are kept in step both ways, because a Revert writes the string back and the trigger has to follow it — an effect the form keeps, since a `.rsx` component cannot hold one of its own past the call that builds it.
pub(crate) fn option_index(
    value: RwSignal<String>,
    options: &'static [&'static str],
) -> RwSignal<u32> {
    record_field(&value);
    let index_of = |current: &str| options.iter().position(|o| *o == current).unwrap_or(0) as u32;
    let picked = signal(index_of(&value.peek()));
    let follow_value = value.read_only();
    let follow_index = picked;
    park(move |_| {
        telar::effect(move || {
            let at = index_of(&follow_value.get());
            if follow_index.peek() != at {
                follow_index.set(at);
            }
        })
    });
    picked
}

/// Writes the option at `at` back to the field it came from.
///
/// Guarded because a signal notifies on every write: re-picking what is already selected is not an edit, and counting it would apply the whole form.
pub(crate) fn pick_option(value: &RwSignal<String>, options: &'static [&'static str], at: u32) {
    let next = options[at as usize].to_string();
    if value.peek() != next {
        value.set(next);
    }
}

/// Wires the recorded fields to `apply`, debounced — the second half of K14.
///
/// Returns the subscriptions for the caller to hold. The window survives the reload its own write causes (the shell reconciles its surfaces in place rather than reopening them), so what the user is typing into is the same field it was before the change landed.
pub(crate) fn live_apply(apply: Rc<dyn Fn()>) -> Vec<telar::Effect> {
    let Some(recorder) = RECORDING.with(|recording| recording.borrow_mut().take()) else {
        return Vec::new();
    };
    let FormRecorder {
        revision,
        mut subscriptions,
    } = recorder;
    let watched = revision.read_only();
    subscriptions.push(telar::effect(move || {
        let at = watched.get();
        if at == 0 {
            return;
        }
        let apply = Rc::clone(&apply);
        let watched = watched;
        // Debounced by re-reading the counter when the timer fires: a change that arrived in the meantime has its own timer running, so only the last one in a burst applies.
        platform_wayland::timeout(LIVE_DEBOUNCE, move || {
            if watched.peek() == at {
                apply();
            }
        });
    }));
    subscriptions
}

thread_local! {
    /// Which `config.toml` the forms on this window read and write. Ambient rather than an argument threaded through all fifty-one of them: a form is not given a file, it edits *the* file, and the panel is the only thing that ever chose one. A test points it at a scratch copy the same way.
    static SOURCE: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

/// Names the file the forms edit, for as long as this surface lives.
pub(crate) fn set_source(path: PathBuf) {
    SOURCE.with(|slot| *slot.borrow_mut() = Some(path));
}

/// The file the forms edit, defaulting to the running shell's own.
pub(crate) fn source_path() -> PathBuf {
    SOURCE.with(|slot| slot.borrow().clone().unwrap_or_else(Config::default_path))
}

/// What a form seeds itself from: the file as it stands *now*, and where it is.
///
/// Read per form rather than once per window, because a form rebuilt is a form re-seeded — that is how Revert and an edit made by hand reach a page that is already open.
pub(crate) fn source() -> (Config, PathBuf) {
    let path = source_path();
    (Config::load_or_default(&path), path)
}

thread_local! {
    /// The page area's scroll window, for the one form that draws more rows than fit in it. Ambient for the same reason the source file is: a section takes no arguments, and threading a viewport through every one of them to reach a single list is the shape `Build` exists not to have.
    static VIEWPORT: std::cell::RefCell<Option<telar::ScrollViewport>> =
        const { std::cell::RefCell::new(None) };
}

/// Names the scroll window the forms on this page sit in.
pub(crate) fn set_viewport(viewport: telar::ScrollViewport) {
    VIEWPORT.with(|slot| *slot.borrow_mut() = Some(viewport));
}

/// The scroll window, or `None` for a form built outside a page — a preview or a test, where there is nothing to virtualise against and a plain list is the right answer.
pub(crate) fn viewport() -> Option<telar::ScrollViewport> {
    VIEWPORT.with(|slot| slot.borrow().clone())
}

thread_local! {
    /// The file exactly as it was when this settings window first opened, which is what Revert restores.
    static OPENED_WITH: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

/// Takes the Revert snapshot, once per window. A second call while one is held is the window rebuilding itself after a reload, and overwriting it there would make Revert restore the change it is meant to undo.
pub(crate) fn remember_opened(path: &Path) {
    OPENED_WITH.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.is_none() {
            *slot = std::fs::read_to_string(path).ok();
        }
    });
}

/// Drops the snapshot, so the next window reverts to the file as *it* found it.
pub(crate) fn forget_opened() {
    OPENED_WITH.with(|slot| *slot.borrow_mut() = None);
}

/// Puts `config.toml` back to how it was when this settings window opened, and lets the config watcher apply it — the Revert half of K14.
///
/// The whole file rather than a per-section undo stack: with apply-on-change there is no single edit to undo, and "how it was when I opened this" is the state a user actually means. It therefore also discards a change made to the file by hand while the window was open, which is why it is a button and not automatic.
pub(crate) fn revert_to_opened(path: &Path) {
    let snapshot = OPENED_WITH.with(|slot| slot.borrow().clone());
    let Some(text) = snapshot else {
        return;
    };
    // This window's own write, like a save — what it does to the forms it decides itself, below.
    surfaces::shell::authored_change(MODULE);
    if let Err(e) = std::fs::write(path, text) {
        tracing::warn!("settings: could not revert {}: {e}", path.display());
    }
}

pub(crate) fn persist<T: Serialize>(path: &Path, name: &str, value: &T) {
    // Written before the write, not after: the config watcher can notice the file inside the same turn.
    surfaces::shell::authored_change(MODULE);
    if let Err(e) = Config::save_section(path, name, value) {
        tracing::warn!("settings: could not save [{name}]: {e}");
    }
}

/// [`persist`] for a form that owns only *part* of a `[toml]` section.
///
/// `save_section` replaces the whole table, so every form has to hand it the keys it does not edit as well — and taking those from the snapshot the form was built with is what makes two forms over one section destructive: the applications page marks a favourite, the launcher form saves a width ten seconds later, and the favourite is gone. Reading the file at save time is also what makes a hand-edit made while the settings window was open survive it.
pub(crate) fn persist_with<T: Serialize>(
    path: &Path,
    name: &str,
    build: impl FnOnce(&Config) -> T,
) {
    persist(path, name, &build(&Config::load_or_default(path)));
}

pub(crate) fn section(
    title: impl Fn() -> String + 'static,
    mut rows: Vec<Box<dyn LayoutItem>>,
    save: Box<dyn LayoutItem>,
    theme: NordTheme,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let mut children = vec![section_label(title, theme)?];
    children.append(&mut rows);
    children.push(save);
    let column = Container::new(
        LayoutStyle::new()
            .flex_column()
            .gap(space::md())
            .width(SizeDimension::Percent(1.0)),
        children,
    )?;
    Ok(Box::new(column))
}

pub(crate) fn section_label(
    label: impl Fn() -> String + 'static,
    theme: NordTheme,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let text = Text::new(label, LayoutStyle::new(), move || {
        theme
            .text_style(FontRole::Body, theme.text)
            .with_font_weight(700)
    })?;
    Ok(Box::new(text))
}

pub(crate) fn subheader(
    label: impl Fn() -> String + 'static,
    theme: NordTheme,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let text = Text::new(label, LayoutStyle::new(), move || {
        theme
            .text_style(FontRole::Caption, theme.muted)
            .with_font_weight(700)
    })?;
    Ok(Box::new(text))
}

pub(crate) fn labelled(
    label: impl Fn() -> String + 'static,
    control: Box<dyn LayoutItem>,
    theme: NordTheme,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let label_text = Text::new(label, LayoutStyle::new().width(120.0), move || {
        theme.text_style(FontRole::Body, theme.subtle)
    })?;
    let row = Container::new(
        LayoutStyle::new()
            .flex_row()
            .align_items(AlignItems::CENTER)
            .gap(space::md())
            .width(SizeDimension::Percent(1.0)),
        vec![Box::new(label_text), control],
    )?;
    Ok(Box::new(row))
}

pub(crate) fn text_field(
    label: impl Fn() -> String + 'static,
    value: RwSignal<String>,
    placeholder: &str,
    theme: NordTheme,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    record_field(&value);
    let input = Input::new(
        value,
        LayoutStyle::new()
            .flex_grow(1.0)
            .height(theme.font(FontRole::Body) * 1.6),
        move || theme.text_style(FontRole::Body, theme.text),
    )?
    .placeholder(placeholder.to_string());
    let boxed = StyledContainer::new(
        LayoutStyle::new()
            .flex_grow(1.0)
            .padding_horizontal(space::md())
            .padding_vertical(space::sm()),
        paint::md(theme.base),
        vec![box_item(input)],
    )?;
    labelled(label, Box::new(boxed), theme)
}

/// A switch. The catalogue's `toggle` carries its own label, which this form has already drawn in the row's left column — so it takes an empty one and the row stays the shape every other field is.
pub(crate) fn toggle_field(
    label: impl Fn() -> String + 'static,
    value: RwSignal<bool>,
    theme: NordTheme,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    record_field(&value);
    let control = telar::toggle(
        telar::ToggleProps::props()
            .checked(value)
            .color(telar::Reactive::of(move || theme.accent))
            .build(),
        telar::Children::default(),
    )?;
    labelled(label, control, theme)
}

/// A picker: the current option, and a panel of all of them on press.
pub(crate) fn enum_field(
    label: impl Fn() -> String + 'static,
    value: RwSignal<String>,
    options: &'static [&'static str],
    theme: NordTheme,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let picked = option_index(value, options);
    let control = telar::select(
        telar::SelectProps::props()
            .selected(picked)
            .color(telar::Reactive::of(move || theme.accent))
            .stretch(true)
            .on_select(std::rc::Rc::new(move |at| pick_option(&value, options, at)))
            .build(),
        // The choices are rows now, not strings: one `item` per option, rebuilt whenever the list reopens.
        telar::Children::new(move || {
            let mut slots = telar::Slots::new();
            for opt in options {
                let row = telar::item(
                    telar::ItemProps::props()
                        .label(telar::Reactive::of(move || opt.to_string()))
                        .build(),
                    telar::Children::default(),
                )?;
                slots.push(None, row);
            }
            Ok(slots)
        }),
    )?;
    labelled(label, control, theme)
}

/// A form's action button — and, with live preview on, where that form's fields get wired to it.
///
/// The wiring lives here because every `*_section` builds its fields and then calls this exactly once, so this is the one point in the file that has both the form's fields (through [`RECORDING`]) and the action they feed. The alternative was a fortieth argument on forty functions.
#[derive(telar::Props)]
pub struct SaveButtonProps {
    pub label: telar::Reactive<String>,
    pub on_press: std::rc::Rc<dyn Fn()>,
}

pub(crate) fn save_button(
    props: SaveButtonProps,
    _children: telar::Children,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let SaveButtonProps { label, on_press } = props;
    live_apply(Rc::clone(&on_press));

    // The catalogue's button with no `fill` of its own: unset means "the theme's `primary`", which is this theme's accent, darkened on hover — the three states this form used to spell out by hand.
    let button = telar::button(
        telar::ButtonProps::props()
            .label(label)
            .on_press(on_press)
            .build(),
        telar::Children::default(),
    )?;
    Ok(button)
}

pub(crate) fn opt_num<T: ToString>(value: Option<T>) -> String {
    value.map(|v| v.to_string()).unwrap_or_default()
}

pub(crate) fn opt_string(s: &str) -> Option<String> {
    let s = s.trim();
    (!s.is_empty()).then(|| s.to_string())
}

pub(crate) fn opt_u32(s: &str) -> Option<u32> {
    s.trim().parse().ok()
}

pub(crate) fn opt_f32(s: &str) -> Option<f32> {
    s.trim().parse().ok()
}

pub(crate) fn parse_u32(s: &str, fallback: u32) -> u32 {
    s.trim().parse().unwrap_or(fallback)
}

pub(crate) fn parse_i32(s: &str, fallback: i32) -> i32 {
    s.trim().parse().unwrap_or(fallback)
}

pub(crate) fn parse_u64(s: &str, fallback: u64) -> u64 {
    s.trim().parse().unwrap_or(fallback)
}

pub(crate) fn parse_f32(s: &str, fallback: f32) -> f32 {
    s.trim().parse().unwrap_or(fallback)
}

pub(crate) fn join_csv(items: &[String]) -> String {
    items.join(", ")
}

pub(crate) fn split_csv(s: &str) -> Vec<String> {
    s.split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_string)
        .collect()
}

pub(crate) fn edge_str(edge: Edge) -> &'static str {
    edge.as_str()
}

pub(crate) fn variant_str(variant: Variant) -> &'static str {
    match variant {
        Variant::Filled => "filled",
        Variant::Default => "default",
    }
}

pub(crate) fn parse_variant(s: &str) -> Variant {
    match s {
        "filled" => Variant::Filled,
        _ => Variant::Default,
    }
}

pub(crate) fn open_mode_str(mode: OpenMode) -> &'static str {
    match mode {
        OpenMode::Float => "float",
        OpenMode::Drawer => "drawer",
    }
}

pub(crate) fn parse_open_mode(s: &str) -> OpenMode {
    match s {
        "float" => OpenMode::Float,
        _ => OpenMode::Drawer,
    }
}

pub(crate) fn parse_edge(s: &str) -> Edge {
    match s {
        "bottom" => Edge::Bottom,
        "left" => Edge::Left,
        "right" => Edge::Right,
        _ => Edge::Top,
    }
}

pub(crate) fn fullscreen_popups_str(policy: FullscreenPopups) -> &'static str {
    match policy {
        FullscreenPopups::On => "on",
        FullscreenPopups::Off => "off",
        FullscreenPopups::Never => "never",
    }
}

pub(crate) fn parse_fullscreen_popups(s: &str) -> FullscreenPopups {
    match s {
        "on" => FullscreenPopups::On,
        "never" => FullscreenPopups::Never,
        _ => FullscreenPopups::Off,
    }
}

pub(crate) fn align_str(align: Align) -> &'static str {
    match align {
        Align::Start => "start",
        Align::Center => "center",
        Align::End => "end",
    }
}

pub(crate) fn parse_align(s: &str) -> Align {
    match s {
        "start" => Align::Start,
        "end" => Align::End,
        _ => Align::Center,
    }
}

pub(crate) fn shape_str(shape: Shape) -> &'static str {
    match shape {
        Shape::Bar => "bar",
        Shape::Sections => "sections",
        Shape::Chips => "chips",
    }
}

pub(crate) fn parse_shape(s: &str) -> Shape {
    match s {
        "sections" => Shape::Sections,
        "chips" => Shape::Chips,
        _ => Shape::Bar,
    }
}

pub(crate) fn capitalize_str(capitalize: Capitalize) -> &'static str {
    match capitalize {
        Capitalize::None => "none",
        Capitalize::Upper => "upper",
        Capitalize::Lower => "lower",
        Capitalize::Title => "title",
    }
}

pub(crate) fn parse_capitalize(s: &str) -> Capitalize {
    match s {
        "upper" => Capitalize::Upper,
        "lower" => Capitalize::Lower,
        "title" => Capitalize::Title,
        _ => Capitalize::None,
    }
}

pub(crate) fn temperature_unit_str(unit: TemperatureUnit) -> &'static str {
    match unit {
        TemperatureUnit::Celsius => "celsius",
        TemperatureUnit::Fahrenheit => "fahrenheit",
    }
}

pub(crate) fn parse_temperature_unit(s: &str) -> TemperatureUnit {
    match s {
        "fahrenheit" => TemperatureUnit::Fahrenheit,
        _ => TemperatureUnit::Celsius,
    }
}

pub(crate) fn media_scroll_str(scroll: MediaScroll) -> &'static str {
    match scroll {
        MediaScroll::Volume => "volume",
        MediaScroll::Track => "track",
        MediaScroll::Seek => "seek",
        MediaScroll::None => "none",
    }
}
pub(crate) fn parse_media_scroll(raw: &str) -> MediaScroll {
    match raw {
        "track" => MediaScroll::Track,
        "seek" => MediaScroll::Seek,
        "none" => MediaScroll::None,
        _ => MediaScroll::Volume,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn csv_round_trips_and_trims() {
        assert_eq!(
            split_csv("workspaces,  clock ,notes"),
            vec![
                "workspaces".to_string(),
                "clock".to_string(),
                "notes".to_string(),
            ]
        );
        assert_eq!(split_csv("  ,, "), Vec::<String>::new());
        assert_eq!(join_csv(&["a".to_string(), "b".to_string()]), "a, b");
    }

    /// A reorder must not cost an entry its own settings. The comma-separated field this replaced could only carry ids, so it had to reconstruct `{ id = "clock", accent = "red" }` by claiming entries back by name; the pill editor moves the entry itself, and this is the guard that it keeps doing so — including across zones, where losing the accent would look like the module having been re-added rather than moved.
    #[test]
    fn enum_helpers_round_trip() {
        for e in Edge::ALL {
            assert_eq!(parse_edge(edge_str(e)), e);
        }
        for (s, a) in [
            ("start", Align::Start),
            ("center", Align::Center),
            ("end", Align::End),
        ] {
            assert_eq!(align_str(a), s);
            assert_eq!(parse_align(s), a);
        }
        for (s, sh) in [
            ("bar", Shape::Bar),
            ("sections", Shape::Sections),
            ("chips", Shape::Chips),
        ] {
            assert_eq!(shape_str(sh), s);
            assert_eq!(parse_shape(s), sh);
        }
    }

    /// K14's one subtle rule: an effect fires once when it is registered, and that run is the field being seeded from the file — not a user changing anything. Counting it would make every form on the page write itself back the moment it was drawn, which with a dozen forms on a page is a dozen config saves and a dozen reloads for a window the user has only just opened.
    #[test]
    fn seeding_a_form_is_not_a_change_to_it() {
        telar::reset_runtime();
        RECORDING.with(|recording| *recording.borrow_mut() = None);

        let name = signal("nord".to_string());
        let filled = signal(false);
        record_field(&name);
        record_field(&filled);

        let recorder = RECORDING.with(|recording| recording.borrow_mut().take());
        let recorder = recorder.expect("two fields were recorded");
        assert_eq!(recorder.subscriptions.len(), 2);
        assert_eq!(
            recorder.revision.peek(),
            0,
            "drawing the form is not editing it"
        );

        name.set("rose-pine".to_string());
        assert_eq!(recorder.revision.peek(), 1);
        filled.set(true);
        assert_eq!(recorder.revision.peek(), 2, "either field counts");

        // And the recording is per form: the next one starts empty, or a section would apply its neighbour's fields as well as its own.
        assert!(RECORDING.with(|recording| recording.borrow().is_none()));
    }
}
