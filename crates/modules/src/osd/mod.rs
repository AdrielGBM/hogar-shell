use telar::{Color, LayoutItem, LayoutStyle, RectStyle, SizeDimension, StyledContainer, set_theme};

use config::theme::NordTheme;

/// Which live state an OSD reflects. A single-slot OSD (§6): one at a time, replaced on the next trigger.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OsdKind {
    Volume,
    Brightness,
    Microphone,
}

impl OsdKind {
    /// What the column keys an OSD card on. One slot per kind rather than one for every OSD, so a wheel spun ten notches redraws one card instead of pushing ten.
    pub(crate) fn id(self) -> &'static str {
        match self {
            OsdKind::Volume => "volume",
            OsdKind::Brightness => "brightness",
            OsdKind::Microphone => "microphone",
        }
    }
}

/// Which state the OSD being built reflects, provided into its surface's scope so `osd.rsx` reads it via `inject` — scoped to the surface, not a global thread-local.
#[derive(Clone, Copy)]
struct OsdCtx {
    kind: OsdKind,
}

/// The kind the OSD being built reflects; read by `osd.rsx`.
pub fn current_osd_kind() -> OsdKind {
    util::state::context::<OsdCtx>()
        .map(|ctx| ctx.kind)
        .unwrap_or(OsdKind::Volume)
}

/// The corner radius the OSD being built uses — the bar's, like every other panel's; read by `osd.rsx`.
pub fn current_osd_radius() -> f32 {
    ui::panel::content_radius()
}

/// Builds the OSD's content tree for `kind`/`theme` (declared in `osd.rsx`), putting both in scope for it first — which is why the surface calls this rather than the component directly.
pub(crate) fn osd_content(kind: OsdKind, theme: NordTheme) -> Box<dyn LayoutItem> {
    set_theme(theme);
    util::state::set_context(OsdCtx { kind });
    let content = osd::osd(osd::OsdProps::props().build(), telar::Children::default()).expect("osd content build failed");
    let Some(threshold) = crate::stack::swipe::column_threshold() else {
        return content;
    };
    // Wrapped only to carry the gesture: the box paints nothing, and the OSD inside it is unchanged.
    let Ok(draggable) = StyledContainer::new(
        LayoutStyle::new().width(SizeDimension::Percent(1.0)),
        |_| RectStyle::filled(Color::TRANSPARENT, 0.0),
        vec![content],
    ) else {
        return osd::osd(osd::OsdProps::props().build(), telar::Children::default()).expect("osd content build failed");
    };
    Box::new(crate::stack::swipe::swipe_aside(
        draggable,
        threshold,
        crate::stack::clear_osd,
    ))
}

/// Shows (or replaces) the single-slot OSD for `kind`.
///
/// It has no surface of its own any more: an OSD is a card in the shell's one column, so this posts it there and [`crate::stack`] decides where it goes, how long it stays and that it never takes the pointer.
pub fn show(kind: OsdKind) {
    crate::stack::show_osd(kind);
}

/// Percentage points to move for a scroll delta: one configured `increment` per notch, in the scrolled direction. `dy` is positive scrolling up (the platform already flips Wayland's axis), which is the direction that raises the level.
fn scroll_step(increment: i32, dy: f32) -> i32 {
    if dy > 0.0 { increment } else { -increment }
}

/// The wheel step for the audio chips, from `[audio] increment`.
fn audio_step(dy: f32) -> i32 {
    scroll_step(services::volume::settings().step(), dy)
}

/// The wheel step for the backlight chip, from `[brightness] increment`.
fn brightness_step(dy: f32) -> i32 {
    scroll_step(services::brightness::settings().step(), dy)
}

/// Flashes the volume OSD without changing anything — for callers that already moved the level (a keybind routed through IPC) and only want the feedback.
pub fn show_volume() {
    show(OsdKind::Volume);
}

pub fn show_brightness() {
    show(OsdKind::Brightness);
}

pub fn show_microphone() {
    show(OsdKind::Microphone);
}

pub fn mic_action() {
    services::volume::toggle_mic_mute();
    show(OsdKind::Microphone);
}

pub fn mic_scroll(_dx: f32, dy: f32) {
    services::volume::step_mic(audio_step(dy));
    show(OsdKind::Microphone);
}

pub fn volume_action() {
    services::volume::toggle_mute();
    show(OsdKind::Volume);
}

pub fn volume_scroll(_dx: f32, dy: f32) {
    services::volume::step(audio_step(dy));
    show(OsdKind::Volume);
}

pub fn brightness_action() {
    show(OsdKind::Brightness);
}

pub fn brightness_scroll(_dx: f32, dy: f32) {
    services::brightness::step(brightness_step(dy));
    show(OsdKind::Brightness);
}
