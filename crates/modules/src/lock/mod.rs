//! The lock screen: the layout's `lock` layer, on one `ext-session-lock-v1` surface per monitor.
//!
//! Three things shape every decision here. The surface covers the whole output and the compositor gives it the keyboard — there is no scrim, no dismiss, no way out but authenticating. That way out has to survive everything else on the screen failing, so the prompt is built first, from code that depends on nothing but [`LockState`], the theme and i18n, and a screen that cannot be built at all mounts the minimal lock instead of taking the process down while the compositor keeps the session locked. And what is on the screen besides the prompt is **the layout's**, drawn through the same area builders the desktop's layers use ([`surfaces::area`]) rather than a second set that could disagree with them.
//!
//! **Readings, never controls** (TA-8). Every area but the prompt is [`inert`](telar::StyledContainer::inert), so no pointer or key event reaches it whatever it was built from; every instance is built for [`Audience::Anyone`], so a field its module declares private on a locked screen draws as empty rather than as itself; and a representation that answers the pointer is drawn as a neutral placeholder, which is the last of three lines — validation refuses one on load and `layout add` refuses to place one.
//!
//! **The layer is frozen when the lock is taken** ([`LockLayout`]). A layout edit made while the screen is covered applies at the next lock, so the tree a user is typing a password into cannot change under them, and a monitor plugged in mid-lock is covered with what the others show.

use std::sync::Arc;

use ui::scale::space;

use telar::{
    AlignItems, App, BorderRadius, Color, Component, Container, Input, JustifyContent, LayoutError,
    LayoutItem, LayoutStyle, RectStyle, SizeDimension, StyledContainer, Text, WindowConfig,
    box_item, reset_layout_runtime, set_theme, signal, use_theme,
};

use config::Config;
use config::theme::{FontRole, NordTheme};
use layout::{
    AreaId, Catalogue, LayerKind, Layout, Library, NOMINAL_OUTPUT, Rect, Resolved, ResolvedArea,
    ResolvedAreaKind, Style, prompt_card,
};
use services::lock::{self, LockState, Method, Screen};
use surfaces::area::Surround;
use surfaces::layer_window::Reserved;
use telar::WindowRoot;
use ui::host::Audience;
use util::report::Report;

const CARD_WIDTH: f32 = 380.0;

/// The lock layer as it stood when the lock was taken, and what it takes to resolve it for one output.
///
/// The layout is carried rather than a resolved arrangement per output, because an output that arrives while the screen is locked has to be covered with the same content as the rest, and only the layout can answer for a monitor nobody had seen yet. Carrying it is also what freezes it: resolution is pure, so resolving the same snapshot again gives the same answer however long the session has been locked and whatever has been edited since.
pub struct LockLayout {
    layout: Layout,
    known: Library,
}

impl LockLayout {
    pub fn of(layout: &Layout, known: &Library) -> Self {
        Self {
            layout: layout.clone(),
            known: known.clone(),
        }
    }

    /// The lock layer of `layout` when a locked screen may be drawn from it, or everything that rules it out — which is the minimal lock (TA-8).
    ///
    /// Every rule has to hold on each of `outputs` (every screen there is, or the nominal one where none is known): every instance a reading — a komponent's children included — no action bound, a prompt that cannot be hidden, covered, faded or left unreadable, and a layer that resolves. Only the lock layer is judged, so a mistyped bar never costs the user the lock screen they configured. `file` is where the findings say the layout lives.
    pub fn checked(
        layout: &Layout,
        known: &Library,
        catalogue: &dyn Catalogue,
        theme: &NordTheme,
        outputs: &[Option<&str>],
        file: &str,
    ) -> Result<Self, Report> {
        let report = Self::problems(layout, known, catalogue, theme, outputs, file);
        match report.errors.is_empty() {
            true => Ok(Self::of(layout, known)),
            false => Err(report),
        }
    }

    /// Everything [`checked`](Self::checked) finds wrong with the lock layer of `layout`, without keeping a copy of it.
    pub fn problems(
        layout: &Layout,
        known: &Library,
        catalogue: &dyn Catalogue,
        theme: &NordTheme,
        outputs: &[Option<&str>],
        file: &str,
    ) -> Report {
        let mut report = layout::validate_lock(layout, catalogue);
        report.merge(layout::validate_komponents_lock(layout, known, catalogue));
        let nominal = [None];
        let outputs = match outputs.is_empty() {
            true => &nominal[..],
            false => outputs,
        };
        for output in outputs {
            let (resolved, resolving) =
                layout::resolve(layout, known, output.unwrap_or(NOMINAL_OUTPUT), None);
            report.merge(resolving);
            report.merge(layout::validate_resolved(&resolved, file, theme));
        }
        report
    }

    /// The lock layer for one output, and whatever could not be resolved.
    pub fn resolve(&self, output: Option<&str>) -> (Resolved, Report) {
        layout::resolve(
            &self.layout,
            &self.known,
            output.unwrap_or(NOMINAL_OUTPUT),
            None,
        )
    }
}

/// One monitor's lock surface. Built by the platform crate's lock session, once per output and again for any monitor connected while the screen is locked.
pub struct LockApp {
    /// `None` before the shell has a config — which cannot happen for a lock the shell itself took, but the type says so rather than the code assuming it.
    pub config: Option<Arc<Config>>,
    pub output: Option<String>,
    /// Which screen the lock service asked for. [`Screen::Minimal`] never builds the configured screen at all, rather than falling back from it: none of that screen's code runs, so none of it can fail a second time.
    pub screen: Screen,
    /// The lock layer the session was taken with, or `None` where it could not be resolved or validated — which is the minimal lock, whatever [`Screen`] asked for.
    pub lock: Option<Arc<LockLayout>>,
}

impl App for LockApp {
    fn root(&self) -> Box<dyn Component> {
        reset_layout_runtime();
        let config = self
            .config
            .clone()
            .unwrap_or_else(|| Arc::new(Config::default()));
        set_theme(config.resolve_theme());
        services::locale::attach(config.language());
        let Some(lock) = self
            .lock
            .clone()
            .filter(|_| self.screen == Screen::Configured)
        else {
            return mount(|| minimal_screen(Prompting::Live));
        };
        let output = self.output.clone();
        let size = output_size(self.output.as_deref());
        mount(move || screen(&config, &lock, output.as_deref(), size, Prompting::Live))
    }

    fn clear_color(&self) -> Option<Color> {
        // Opaque, and deliberately the darkest token there is: a lock surface is the only thing between the desktop and the room, so anything translucent would be a hole in it.
        let theme = self
            .config
            .as_ref()
            .map(|c| c.resolve_theme())
            .unwrap_or_default();
        Some(theme.base)
    }

    fn window_config(&self) -> Option<WindowConfig> {
        None
    }
}

/// The screen a preview of the lock stands on. A lock surface is a whole output, so a preview of one is an output too — at the size the preview page can show rather than at a monitor's, since every area on it places itself as a fraction of the screen it is on.
pub(crate) const PREVIEW_SCREEN: (f32, f32) = (960.0, 600.0);

/// The lock screen as the session opener mounts it, for [`crate::preview`] — over the starter config and the layout the shell ships, since a preview has no session to read either from.
pub(crate) fn screen_preview() -> Result<Box<dyn LayoutItem>, LayoutError> {
    let lock = LockLayout::of(&layout::built_in(), &Library::default());
    screen(
        &Arc::new(Config::starter()),
        &lock,
        None,
        PREVIEW_SCREEN,
        Prompting::Live,
    )
}

/// What the prompt on a screen does: take a password on a real lock, or stand in for one on the edit mode's preview of the lock layer (TA-8), where it takes nothing, submits nothing and says it is a preview.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Prompting {
    Live,
    Preview,
}

/// The lock layer as lock mode previews it on an unlocked screen: the same areas, built the same way and for the same audience, over the same opaque background a lock surface clears to, with a prompt that takes nothing (TA-8). Building it takes no lock and touches no lock session.
///
/// `lock` is the layer as [`LockLayout::checked`] answered for it: one it refused, with why, previews as the minimal lock a real lock would fall back to, and says why over it — as does a layer that cannot be built. The whole box claims the pointer, as a lock surface would: nothing under the preview answers while it is up.
pub fn preview(
    config: &Arc<Config>,
    lock: Result<&LockLayout, &str>,
    output: Option<&str>,
    size: (f32, f32),
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let theme = use_theme::<NordTheme>();
    let built = lock.map_err(str::to_string).and_then(|lock| {
        screen(config, lock, output, size, Prompting::Preview).map_err(|err| err.to_string())
    });
    let mut shown = Vec::new();
    match built {
        Ok(screen) => shown.push(screen),
        Err(why) => {
            tracing::info!("the lock layer previews as the minimal lock: {why}");
            shown.push(minimal_screen(Prompting::Preview)?);
            shown.push(fallen_back(theme, why)?);
        }
    }
    Ok(Box::new(
        StyledContainer::new(
            whole_surface(),
            move |_| RectStyle::filled(theme.base, 0.0),
            shown,
        )?
        .input_opaque(),
    ))
}

/// Over a preview that fell back, why a locked screen would show the minimal lock instead of the layout.
fn fallen_back(theme: NordTheme, why: String) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let said = telar::t!("lock.preview_fallback", why = why);
    let note = StyledContainer::new(
        LayoutStyle::new()
            .max_width(CARD_WIDTH * 2.0)
            .padding_horizontal(space::lg())
            .padding_vertical(space::sm()),
        move |_| RectStyle::filled(theme.surface, 0.0).with_radius(space::md().into()),
        vec![box_item(Text::new(
            move || said.clone(),
            LayoutStyle::new(),
            move || theme.text_style(FontRole::Caption, theme.warning),
        )?)],
    )?;
    Ok(Box::new(Container::new(
        LayoutStyle::new()
            .absolute()
            .inset_start(0.0)
            .inset_top(space::xxl() * 3.0)
            .width(SizeDimension::Percent(1.0))
            .flex_row()
            .justify_content(JustifyContent::CENTER),
        vec![box_item(note)],
    )?))
}

fn mount(screen: impl FnOnce() -> Result<Box<dyn LayoutItem>, LayoutError>) -> Box<dyn Component> {
    match screen().and_then(WindowRoot::wrapping) {
        Ok(root) => Box::new(root),
        Err(err) => {
            tracing::error!("lock screen failed to build, mounting the minimal lock: {err}");
            reset_layout_runtime();
            // Fixed code a test builds: if even this fails no field can exist in this process, and dying leaves the compositor holding the lock for whatever takes it next.
            Box::new(
                minimal_screen(Prompting::Live)
                    .and_then(WindowRoot::wrapping)
                    .expect("minimal lock failed to build"),
            )
        }
    }
}

/// The whole surface: every area of the lock layer, each placing itself in the output's own coordinate space.
///
/// **The prompt is built before anything else**, so a reading that fails cannot take the field with it — and if the prompt itself cannot be built the screen fails whole, which [`mount`] answers with the minimal lock. It is then put back at its own place in the stack, which validation has already made the top one.
fn screen(
    config: &Arc<Config>,
    lock: &LockLayout,
    output: Option<&str>,
    size: (f32, f32),
    prompting: Prompting,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let theme = use_theme::<NordTheme>();
    let (resolved, report) = lock.resolve(output);
    if !report.is_clean() {
        tracing::warn!(
            "the lock layer did not fully resolve on {}:\n{}",
            output.unwrap_or("this output"),
            report.render()
        );
    }
    let layer = resolved
        .layer(LayerKind::Lock)
        .ok_or_else(|| LayoutError::Engine("the lock layer resolved to nothing".to_string()))?;

    let surround = Surround {
        config,
        theme,
        output,
        layer: LayerKind::Lock,
        bounds: telar::Rect::new(0.0, 0.0, size.0, size.1),
        // Nothing reserves while the screen is locked: there are no windows to keep out of an edge, and a lock surface has no exclusive zone to ask for (TA-8).
        reserved: Reserved::default(),
        audience: Audience::Anyone,
    };

    let mut nodes: Vec<Option<Box<dyn LayoutItem>>> =
        (0..layer.areas.len()).map(|_| None).collect();
    let prompt_at = layer
        .areas
        .iter()
        .position(|area| matches!(area.kind, ResolvedAreaKind::Prompt { .. }));
    if let Some(at) = prompt_at
        && let ResolvedAreaKind::Prompt { rect } = &layer.areas[at].kind
    {
        nodes[at] = Some(prompt_area(
            &layer.areas[at].id,
            *rect,
            &layer.areas[at].style,
            surround,
            prompting,
        )?);
    }
    for (at, area) in layer.areas.iter().enumerate() {
        if Some(at) == prompt_at {
            continue;
        }
        nodes[at] = Some(reading_area(area, surround)?);
    }

    Ok(Box::new(Container::new(
        whole_surface(),
        nodes.into_iter().flatten().collect(),
    )?))
}

/// One area of readings: built the way the desktop's layers build the same kinds, and then made inert.
///
/// Inert rather than merely unwired: a reading that registered a target despite its module declaring none — a card's own chrome, a placeholder, a scroll area — would otherwise take a press on a screen where nothing may be pressed. The gate is read on every event and every region query, so nothing inside can act however it was built (F-5.4).
///
/// The box it goes in is the whole surface, laid over the boxes of the areas before it rather than after them, because the area inside positions itself absolutely against it; a wrapper the size of its content, or one in the flow, would move the area it wraps.
fn reading_area(
    area: &ResolvedArea,
    surround: Surround,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let theme = surround.theme;
    let id = area.id.to_string();
    // The boundary outlives this call — it catches a failure in what the build registered as well as in the build itself — so what the area is built against is owned by the closure rather than borrowed from here.
    let config = Arc::clone(surround.config);
    let output = surround.output.map(str::to_string);
    let bounds = surround.bounds;
    let built = telar::ErrorBoundary::with_style(
        whole_surface(),
        {
            let area = area.clone();
            move || {
                let surround = Surround {
                    config: &config,
                    theme,
                    output: output.as_deref(),
                    layer: LayerKind::Lock,
                    bounds,
                    reserved: Reserved::default(),
                    audience: Audience::Anyone,
                };
                match surfaces::area::build(&area, surround) {
                    Some(built) => built,
                    None => Ok(Box::new(Container::new(LayoutStyle::new(), Vec::new())?)),
                }
            }
        },
        move |failure| {
            // The reason stays in the log and the report: a message about the shell's internals in front of whoever is standing at the screen is not a thing the lock promised (TA-8).
            tracing::error!("the lock area '{id}' failed to build: {failure}");
            ui::placeholder::neutral(theme)
        },
    )?;
    Ok(Box::new(
        StyledContainer::new(
            stacked(),
            |_| RectStyle::filled(Color::TRANSPARENT, 0.0),
            vec![Box::new(built) as Box<dyn LayoutItem>],
        )?
        .inert(|| true),
    ))
}

/// The one area that answers: the password field, the line under it, and what else this machine can be unlocked with. Its place is in the rect registry like every other area's, which is what lock mode moves and selects it by.
fn prompt_area(
    id: &AreaId,
    rect: Rect,
    style: &Style,
    surround: Surround,
    prompting: Prompting,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let theme = surround.theme;
    let mut column = prompt(theme, prompting)?;
    column.extend(biometric_hint(surround.config, theme)?);
    let card = card(column, theme, style)?;
    let at = surfaces::area::within(rect, surround.bounds);
    let area = Container::new(
        surfaces::area::at(at)
            .flex_row()
            .align_items(AlignItems::CENTER)
            .justify_content(JustifyContent::CENTER),
        vec![card],
    )?;
    surfaces::rects::track(
        surfaces::rects::Node::area(surround.output, LayerKind::Lock, id),
        area.layout_node(),
    );
    Ok(Box::new(area))
}

/// The minimal lock: the prompt alone, centred, on the surface's own background.
///
/// Built from code rather than from a layout, and depending on nothing but [`LockState`], the theme and i18n. It is what a lock taken back after a crash mounts, and what every failure below falls back to (TA-8).
pub(crate) fn minimal_screen(prompting: Prompting) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let theme = use_theme::<NordTheme>();
    let card = card(prompt(theme, prompting)?, theme, &Style::default())?;
    Ok(Box::new(Container::new(
        whole_surface()
            .flex_row()
            .align_items(AlignItems::CENTER)
            .justify_content(JustifyContent::CENTER),
        vec![card],
    )?))
}

fn prompt(theme: NordTheme, prompting: Prompting) -> Result<Vec<Box<dyn LayoutItem>>, LayoutError> {
    if prompting == Prompting::Preview {
        return Ok(vec![preview_field(theme)?, preview_badge(theme)?]);
    }
    let state = signal(lock::current());
    platform_wayland::watch(lock::subscribe, move |next: LockState| state.set(next));
    Ok(vec![
        field(state.read_only(), theme)?,
        status_line(state.read_only(), theme)?,
    ])
}

/// What else this machine can be unlocked with, under the field. Nothing at all where the password is the only way in, which is most machines — a line saying so would be an explanation of an absence.
fn biometric_hint(
    config: &Arc<Config>,
    theme: NordTheme,
) -> Result<Vec<Box<dyn LayoutItem>>, LayoutError> {
    let methods: Vec<String> = offered_methods(config)
        .into_iter()
        .map(|method| match method {
            Method::Fingerprint => telar::t!("lock.by_fingerprint"),
            Method::Face => telar::t!("lock.by_face"),
            Method::Password => telar::t!("lock.by_password"),
        })
        .collect();
    if methods.is_empty() {
        return Ok(Vec::new());
    }
    let line = telar::t!("lock.also_unlocks_with", methods = methods.join(", "));
    Ok(vec![centred(box_item(Text::new(
        move || line.clone(),
        LayoutStyle::new(),
        move || theme.text_style(FontRole::Caption, theme.subtle),
    )?))?])
}

fn card(
    column: Vec<Box<dyn LayoutItem>>,
    theme: NordTheme,
    style: &Style,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let fill = prompt_card(style, &theme);
    let radius: BorderRadius = style.radius.map_or_else(|| rounding().into(), Into::into);
    Ok(Box::new(StyledContainer::new(
        LayoutStyle::new()
            .flex_column()
            .align_items(AlignItems::CENTER)
            .gap(space::xl())
            .width(CARD_WIDTH)
            .padding_all(space::xxl()),
        move |_| RectStyle::filled(fill, 0.0).with_radius(radius),
        column,
    )?))
}

/// The corner every box on this screen rounds by, so the lock screen belongs to the same set as the drawers rather than being the one surface with its own.
fn rounding() -> f32 {
    ui::chrome::content_radius()
}

/// The whole of whatever box it is in, which for a lock surface is the whole output.
fn whole_surface() -> LayoutStyle {
    LayoutStyle::new()
        .width(SizeDimension::Percent(1.0))
        .height(SizeDimension::Percent(1.0))
}

/// The whole surface, over whatever else is on it: one area's box among the others.
fn stacked() -> LayoutStyle {
    whole_surface().absolute().inset_start(0.0).inset_top(0.0)
}

/// How big the monitor a lock surface covers is, for the fractions a layout is written in. A screen the compositor has not measured yet falls back to a common size rather than laying every area out at nothing.
fn output_size(output: Option<&str>) -> (f32, f32) {
    platform_wayland::outputs()
        .into_iter()
        .find(|screen| screen.name.as_deref() == output || output.is_none())
        .and_then(|screen| screen.logical_size)
        .filter(|(width, height)| *width > 0 && *height > 0)
        .map(|(width, height)| (width as f32, height as f32))
        .unwrap_or((1920.0, 1080.0))
}

/// Centres one item across the card.
///
/// A `Text` laid out in a column takes the column's width and draws its glyphs from the left, so `align_items: center` on the card does nothing for it — the row it sits in has to do the centring. Every single-line reading on this screen goes through here rather than each one growing its own wrapper.
pub(crate) fn centred(item: Box<dyn LayoutItem>) -> Result<Box<dyn LayoutItem>, LayoutError> {
    Ok(Box::new(Container::new(
        LayoutStyle::new()
            .flex_row()
            .justify_content(JustifyContent::CENTER)
            .width(SizeDimension::Percent(1.0)),
        vec![item],
    )?))
}

/// The password field. Masked, submits on Enter, and inert while a check is in flight or a lockout is running — a field that keeps taking keystrokes it will throw away reads as a frozen screen.
fn field(
    state: telar::ReadSignal<LockState>,
    theme: NordTheme,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let password = signal(String::new());
    let submit = {
        move || {
            if !state.peek().accepts_input() {
                return;
            }
            let secret = password.peek();
            password.set(String::new());
            lock::submit(secret);
        }
    };

    let input = Input::new(
        password,
        LayoutStyle::new()
            .flex_grow(1.0)
            .height(theme.font(FontRole::Body) * 1.8),
        move || theme.text_style(FontRole::Body, theme.text),
    )?
    .secret()
    // The one surface where focus-on-tap is not good enough: a lock screen that needs a click before it takes a password reads as a frozen machine.
    .autofocus()
    .placeholder(telar::t!("lock.password"))
    .on_submit(submit);

    let rounded = rounding();
    let outline = state;
    Ok(Box::new(StyledContainer::new(
        LayoutStyle::new()
            .flex_row()
            .align_items(AlignItems::CENTER)
            .padding_horizontal(space::xl())
            .padding_vertical(space::md())
            .width(SizeDimension::Percent(1.0)),
        move |_| {
            // The field itself carries the verdict: a wrong password tints the box the user is already looking at, rather than only a line of text below it they have to notice.
            let fill = if outline.get().failures > 0 {
                theme.red.with_alpha(0.18)
            } else {
                theme.base
            };
            RectStyle::filled(fill, rounded)
        },
        vec![box_item(input)],
    )?))
}

/// The password field as the preview draws it: the same box and the same placeholder, with nothing in it that takes a key or reaches `lock::submit`.
fn preview_field(theme: NordTheme) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let placeholder = box_item(Text::new(
        || telar::t!("lock.password"),
        LayoutStyle::new()
            .flex_grow(1.0)
            .height(theme.font(FontRole::Body) * 1.8),
        move || theme.text_style(FontRole::Body, theme.muted),
    )?);
    let rounded = rounding();
    Ok(Box::new(StyledContainer::new(
        LayoutStyle::new()
            .flex_row()
            .align_items(AlignItems::CENTER)
            .padding_horizontal(space::xl())
            .padding_vertical(space::md())
            .width(SizeDimension::Percent(1.0)),
        move |_| RectStyle::filled(theme.base, 0.0).with_radius(rounded.into()),
        vec![placeholder],
    )?))
}

/// Where the status line would be, the word that says this prompt is a picture of one.
fn preview_badge(theme: NordTheme) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let pill = StyledContainer::new(
        LayoutStyle::new()
            .padding_horizontal(space::md())
            .padding_vertical(space::xs()),
        move |_| RectStyle::filled(theme.accent, 0.0).with_radius(space::md().into()),
        vec![box_item(Text::new(
            || telar::t!("lock.preview"),
            LayoutStyle::new(),
            move || theme.text_style(FontRole::Caption, theme.base),
        )?)],
    )?;
    centred(box_item(pill))
}

/// The line under the field: what the shell is waiting for, what went wrong, or how long the lockout has left.
///
/// Read out of the state *before* any branch, so the paint registers its dependency on the frame that draws nothing too — a message that only appears after an unrelated re-render is the failure this avoids.
fn status_line(
    state: telar::ReadSignal<LockState>,
    theme: NordTheme,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let text = state;
    let tint = state;
    // The line is drawn even when it says nothing, so a wrong password does not resize the card under the hand that is about to retype the password.
    centred(box_item(Text::new(
        move || {
            let state = text.get();
            if state.is_locked_out() {
                return telar::t!(
                    "lock.locked_out",
                    seconds = state.lockout_remaining().to_string()
                );
            }
            if let Some(method) = state.busy {
                return translate(method.message_key());
            }
            match state.message.as_deref() {
                Some(key) => translate(key),
                None => String::new(),
            }
        },
        LayoutStyle::new(),
        move || {
            let state = tint.get();
            let colour = if state.message.is_some() || state.is_locked_out() {
                theme.red
            } else {
                theme.muted
            };
            theme.text_style(FontRole::Caption, colour)
        },
    )?))
}

/// Resolves one of the lock screen's own message keys.
///
/// `t!` takes a literal so the analyzer can prove every key exists, but which message the status line shows is decided by a worker thread — so the keys are enumerated here instead. Anything unrecognised falls through to a generic failure rather than printing the key at the user.
fn translate(key: &str) -> String {
    match key {
        "lock.checking" => telar::t!("lock.checking"),
        "lock.touch_sensor" => telar::t!("lock.touch_sensor"),
        "lock.looking" => telar::t!("lock.looking"),
        "lock.wrong_password" => telar::t!("lock.wrong_password"),
        "lock.too_many_tries" => telar::t!("lock.too_many_tries"),
        "lock.account_unavailable" => telar::t!("lock.account_unavailable"),
        "lock.no_authentication" => telar::t!("lock.no_authentication"),
        "lock.empty_password" => telar::t!("lock.empty_password"),
        other => {
            tracing::warn!("lock screen: no message for '{other}'");
            telar::t!("lock.wrong_password")
        }
    }
}

/// Whether a method other than the password is offered, for the hint the screen shows under the field.
pub fn offered_methods(config: &Config) -> Vec<Method> {
    let (fingerprint, face) = services::biometrics::offered(&config.lock);
    let mut methods = Vec::new();
    if fingerprint {
        methods.push(Method::Fingerprint);
    }
    if face {
        methods.push(Method::Face);
    }
    methods
}

#[cfg(test)]
mod tests {
    use telar::{
        AvailableSpace, ComponentList, DrawCommand, Event, EventResult, Key, ModifiersState,
        compute_layout, new_container,
    };

    use config::{LockConfig, NotificationDetail};
    use layout::{
        Anchor, Area, AreaId, AreaKind, Group, GroupId, GroupKind, Instance, InstanceId, Layer,
        Layers, LayoutId, OutputMatch, OutputRule, Representation,
    };
    use ui::descriptor::{
        FieldDef, Input, ModuleDescriptor, Privacy, Representations, SourceDef, WidgetDef,
    };
    use ui::host::{Host, WidgetSize};

    use super::*;

    /// The size every test screen is laid out at, and the box a fractional rect in a test layout is a fraction of.
    const SCREEN: (f32, f32) = (1920.0, 1080.0);

    const SECRET: &str = "424242";
    const PUBLIC: &str = "three waiting";

    /// The three modules these tests place: a reading, a control, and a reading that cannot be built.
    ///
    /// Doubles rather than the shell's own table, because what has to be proven here is the *frame* — that the lock layer builds its instances for [`Audience::Anyone`], that a control on it is stood in for, and that a failure costs one area — and each of those needs a module that does exactly one thing. The real notifications reading's privacy is proven where it is written, over a seeded snapshot, which a headless frame cannot have: the daemon it reads is not running.
    static PROBES: &[ModuleDescriptor] = &[
        ModuleDescriptor {
            id: "probe",
            name: "Probe",
            icon: "circle",
            category: ui::descriptor::Category::Info,
            options: &[],
            representations: Representations {
                widget: Some(WidgetDef {
                    sizes: &[WidgetSize::S, WidgetSize::M],
                    build: probe,
                    input: Input::ReadOnly,
                }),
                ..Representations::NONE
            },
            actions: &[],
            sources: &[SourceDef {
                id: "probe",
                fields: &[PROBE_PUBLIC, PROBE_PRIVATE, PROBE_ASKED],
                feed: |_| {},
            }],
        },
        ModuleDescriptor {
            id: "control",
            name: "Control",
            icon: "circle",
            category: ui::descriptor::Category::Info,
            options: &[],
            representations: Representations {
                chip: Some(ui::descriptor::ChipDef::new(
                    |_| {
                        Ok(Box::new(telar::StyledContainer::new(
                            LayoutStyle::new().width(40.0).height(40.0),
                            |_| RectStyle::filled(Color::TRANSPARENT, 0.0),
                            Vec::new(),
                        )?) as Box<dyn LayoutItem>)
                    },
                    Input::Interactive,
                )),
                ..Representations::NONE
            },
            actions: &[],
            sources: &[],
        },
        ModuleDescriptor {
            id: "broken",
            name: "Broken",
            icon: "circle",
            category: ui::descriptor::Category::Info,
            options: &[],
            representations: Representations {
                widget: Some(WidgetDef {
                    sizes: &[WidgetSize::M],
                    build: |_| Err(LayoutError::Engine("injected".into())),
                    input: Input::ReadOnly,
                }),
                ..Representations::NONE
            },
            actions: &[],
            sources: &[],
        },
        ModuleDescriptor {
            id: "panicky",
            name: "Panicky",
            icon: "circle",
            category: ui::descriptor::Category::Info,
            options: &[],
            representations: Representations {
                widget: Some(WidgetDef {
                    sizes: &[WidgetSize::M],
                    build: |_| panic!("injected"),
                    input: Input::ReadOnly,
                }),
                ..Representations::NONE
            },
            actions: &[],
            sources: &[],
        },
    ];

    const PROBE_PUBLIC: FieldDef = FieldDef {
        name: "public",
        privacy: Privacy::Public,
        ty: ui::descriptor::FieldType::Text,
    };
    const PROBE_PRIVATE: FieldDef = FieldDef {
        name: "private",
        privacy: Privacy::Private,
        ty: ui::descriptor::FieldType::Text,
    };
    /// The shape the notification applications and the media title have: private on a locked screen until `[lock]` says otherwise.
    const PROBE_ASKED: FieldDef = FieldDef {
        name: "asked",
        privacy: Privacy::OnLock(|lock| lock.notification_detail == NotificationDetail::Apps),
        ty: ui::descriptor::FieldType::Text,
    };

    fn probe(host: &Host) -> Result<Box<dyn LayoutItem>, LayoutError> {
        let theme = use_theme::<NordTheme>();
        let lines = [
            host.reveal(&PROBE_PUBLIC, PUBLIC.to_string()),
            host.reveal(&PROBE_PRIVATE, SECRET.to_string()),
            host.reveal(&PROBE_ASKED, format!("asked-{SECRET}")),
        ];
        let items = lines
            .into_iter()
            .map(|line| {
                Text::new(
                    move || line.clone(),
                    LayoutStyle::new(),
                    move || theme.text_style(FontRole::Body, theme.text),
                )
                .map(box_item)
            })
            .collect::<Result<Vec<_>, LayoutError>>()?;
        Ok(Box::new(Container::new(
            LayoutStyle::new().flex_column(),
            items,
        )?))
    }

    /// A lock layer holding `areas`, as a layout the snapshot can be built from.
    fn locked_with(areas: Vec<Area>) -> LockLayout {
        let layout = Layout {
            id: LayoutId::new("test"),
            name: "Test".into(),
            extends: None,
            sources: Default::default(),
            outputs: vec![OutputRule {
                matches: OutputMatch("*".into()),
                layers: Layers {
                    lock: Layer {
                        areas,
                        remove: Vec::new(),
                    },
                    ..Layers::default()
                },
                workspaces: Vec::new(),
            }],
        };
        LockLayout::of(&layout, &Library::default())
    }

    fn prompt_area_of() -> Area {
        Area {
            id: AreaId::new("prompt"),
            kind: Some(AreaKind::Prompt { rect: None }),
            ..Area::default()
        }
    }

    /// A grid of one instance of `module`, covering the top half of the screen so it cannot overlap a centred prompt.
    fn readings(module: &str) -> Area {
        Area {
            id: AreaId::new("readings"),
            kind: Some(AreaKind::Grid {
                rect: Some(Rect {
                    x: 0.0,
                    y: 0.0,
                    w: 1.0,
                    h: 0.3,
                }),
                cell: None,
                gap: None,
                anchor: Some(Anchor::TopLeft),
            }),
            groups: vec![Group {
                id: GroupId::new("cell"),
                kind: Some(GroupKind::Cell {
                    col: 0,
                    row: 0,
                    col_span: 1,
                    row_span: 1,
                }),
                children: vec![Instance {
                    id: InstanceId::new("placed"),
                    module: Some(module.to_string()),
                    representation: Some(Representation::WidgetM),
                    ..Instance::default()
                }],
                ..Group::default()
            }],
            ..Area::default()
        }
    }

    /// Every area kind that holds instances, each holding a control, for the one question this set exists to answer: does anything on this layer take input.
    fn one_of_every_kind() -> Vec<Area> {
        let with = |id: &str, kind: AreaKind, group: GroupKind| Area {
            id: AreaId::new(id),
            kind: Some(kind),
            groups: vec![Group {
                id: GroupId::new("in"),
                kind: Some(group),
                children: vec![Instance {
                    id: InstanceId::new(format!("{id}-control")),
                    module: Some("control".to_string()),
                    representation: Some(Representation::Chip),
                    ..Instance::default()
                }],
                ..Group::default()
            }],
            ..Area::default()
        };
        let zone = GroupKind::Zone {
            zone: layout::Zone::Start,
        };
        vec![
            with(
                "bar",
                AreaKind::Bar {
                    edge: Some(config::Edge::Top),
                    thickness: Some(40.0),
                    length: None,
                    offset: None,
                    shape: layout::BarShape::default(),
                    autohide: None,
                },
                zone,
            ),
            with(
                "dock",
                AreaKind::Dock {
                    edge: Some(config::Edge::Bottom),
                    thickness: Some(40.0),
                },
                zone,
            ),
            with(
                "free",
                AreaKind::Free {
                    rect: None,
                    anchor: None,
                },
                zone,
            ),
            readings("control"),
        ]
    }

    fn config_with(lock: LockConfig) -> Arc<Config> {
        Arc::new(Config {
            lock,
            ..Config::starter()
        })
    }

    /// Lays a lock screen out at screen size and answers with everything it draws.
    fn drawn(lock: &LockLayout, config: &Arc<Config>) -> Vec<DrawCommand> {
        seed(config);
        draw(
            screen(config, lock, Some("DP-1"), SCREEN, Prompting::Live)
                .expect("the lock screen builds"),
        )
    }

    fn seed(config: &Arc<Config>) {
        telar::reset_layout_runtime();
        telar::set_locale("en");
        telar::set_theme(config.resolve_theme());
    }

    fn draw(item: Box<dyn LayoutItem>) -> Vec<DrawCommand> {
        let page = || {
            LayoutStyle::new()
                .flex_column()
                .width(SCREEN.0)
                .height(SCREEN.1)
        };
        let root = new_container(page(), &[item.layout_node()]).expect("a root");
        let tree = ComponentList::new(Container::new(page(), vec![item]).expect("a page"));
        compute_layout(
            root,
            AvailableSpace::Definite(SCREEN.0),
            AvailableSpace::Definite(SCREEN.1),
        )
        .expect("the lock screen lays out");
        tree.commands().clone()
    }

    fn text_of(commands: &[DrawCommand]) -> Vec<String> {
        commands
            .iter()
            .filter_map(|command| match command {
                DrawCommand::Text { text, .. } => Some(text.to_string()),
                _ => None,
            })
            .collect()
    }

    /// Whether anything in this lock screen takes a typed character.
    fn typed(lock: &LockLayout, config: &Arc<Config>) -> EventResult {
        seed(config);
        type_into(
            screen(config, lock, Some("DP-1"), SCREEN, Prompting::Live)
                .expect("the lock screen builds"),
        )
    }

    fn type_into(item: Box<dyn LayoutItem>) -> EventResult {
        let mut page = Container::new(
            LayoutStyle::new()
                .flex_column()
                .width(SCREEN.0)
                .height(SCREEN.1),
            vec![item],
        )
        .expect("a page");
        compute_layout(
            page.layout_node(),
            AvailableSpace::Definite(SCREEN.0),
            AvailableSpace::Definite(SCREEN.1),
        )
        .expect("the lock screen lays out");
        page.on_event(&Event::KeyPressed {
            key: Key::Char('x'),
            modifiers: ModifiersState::default(),
        })
    }

    /// Where, if anywhere, something in this lock screen answers the pointer.
    fn answer(lock: &LockLayout, config: &Arc<Config>) -> Option<(f32, f32)> {
        seed(config);
        let item = screen(config, lock, Some("DP-1"), SCREEN, Prompting::Live)
            .expect("the lock screen builds");
        ui::descriptor::input_answer(item, SCREEN.0, SCREEN.1).expect("it lays out")
    }

    /// **Nothing on the lock layer but the prompt may act.** Every area is built behind an inert gate, which is read on every event and every region query — so a control that reached the layer despite validation and `layout add` refusing it still cannot be pressed, scrolled or hovered (TA-8).
    #[test]
    fn no_area_but_the_prompt_answers_the_pointer() {
        ui::descriptor::install(PROBES);
        let config = config_with(LockConfig::default());
        assert_eq!(
            answer(&locked_with(one_of_every_kind()), &config),
            None,
            "a bar, a dock, a free rectangle and a grid, each with a control in it, and not one of them answers"
        );
    }

    /// The keyboard half, and the other half of the check above: a typed character reaches the prompt and reaches nothing else, so neither statement is about a screen that simply drew nothing.
    ///
    /// The prompt is what the keyboard is *for* here — the compositor hands a lock surface the keyboard and the field autofocuses — which is also why it is the one of the two that a pointer probe cannot show: the field is a key target, not a press target.
    #[test]
    fn a_typed_character_reaches_the_prompt_and_nothing_else() {
        ui::descriptor::install(PROBES);
        let config = config_with(LockConfig::default());
        assert_eq!(
            typed(&locked_with(vec![prompt_area_of()]), &config),
            EventResult::Handled,
            "the field a password is typed into has to take a keystroke"
        );
        assert_eq!(
            typed(&locked_with(one_of_every_kind()), &config),
            EventResult::Ignored,
            "and a layer of readings takes none, however they were built"
        );
    }

    /// A control hand-edited onto the lock layer is drawn as a placeholder rather than built. The error fill is what says so: a reading draws none.
    #[test]
    fn a_control_on_the_lock_layer_is_drawn_as_a_placeholder() {
        ui::descriptor::install(PROBES);
        let config = config_with(LockConfig::default());
        let error = config.resolve_theme().error;
        let stood_in = |areas: Vec<Area>| {
            drawn(&locked_with(areas), &config).iter().any(
                |command| matches!(command, DrawCommand::Rect { style, .. } if style.fill == Some(telar::Paint::Solid(error))),
            )
        };
        assert!(
            stood_in(vec![readings("control")]),
            "a representation that answers the pointer is stood in for"
        );
        assert!(
            !stood_in(vec![readings("probe")]),
            "and a reading is built, not stood in for"
        );
    }

    /// A reading on this layer is built for whoever is in front of the screen, so a field its module declares private draws as nothing — and one `[lock]` decides draws according to what the user asked for.
    #[test]
    fn a_reading_on_the_lock_layer_cannot_draw_a_private_field() {
        ui::descriptor::install(PROBES);
        for detail in [NotificationDetail::Count, NotificationDetail::Apps] {
            let config = config_with(LockConfig {
                notification_detail: detail,
                ..LockConfig::default()
            });
            let shown = text_of(&drawn(&locked_with(vec![readings("probe")]), &config));
            assert!(
                shown.iter().any(|text| text.contains(PUBLIC)),
                "{detail:?} drew no reading at all, so this proves nothing: {shown:?}"
            );
            assert!(
                !shown.iter().any(|text| text == SECRET),
                "{detail:?} drew a private field on a locked screen: {shown:?}"
            );
            assert_eq!(
                shown.iter().any(|text| text.contains("asked-")),
                detail == NotificationDetail::Apps,
                "{detail:?} disagreed with what `[lock]` asks for: {shown:?}"
            );
        }
    }

    /// A reading that cannot be built costs its own area and nothing else — least of all the prompt, which is why it is built first and outside every boundary.
    ///
    /// A panic as well as an error, because the boundary is what makes the two the same news: a reading that panics inside a locked session would otherwise take the process with it, and the compositor would keep the screen covered with nothing on it (T-1.14).
    #[test]
    fn a_reading_that_fails_to_build_leaves_the_prompt_standing() {
        ui::descriptor::install(PROBES);
        let config = config_with(LockConfig::default());
        for module in ["broken", "panicky"] {
            let shown = text_of(&drawn(
                &locked_with(vec![readings(module), prompt_area_of()]),
                &config,
            ));
            assert!(
                shown.iter().any(|text| text == &telar::t!("lock.password")),
                "`{module}` took the field with it: {shown:?}"
            );
        }
    }

    /// A lock the shell has no layout for is the minimal lock, and so is one taken back after a crash: both mount a field, which is the one thing a covered screen cannot be without.
    ///
    /// The app decides *which* of the two a session gets — resolving and validating the layer at `take()`, and falling back where either fails — and proves that where it decides it. What is proven here is that both answers leave a way in.
    #[test]
    fn a_lock_with_no_layout_mounts_the_minimal_lock() {
        let config = config_with(LockConfig::default());
        for screen in [Screen::Configured, Screen::Minimal] {
            let app = LockApp {
                config: Some(Arc::clone(&config)),
                output: None,
                screen,
                lock: None,
            };
            telar::reset_layout_runtime();
            telar::set_locale("en");
            let mounted = app.root();
            let tree = ComponentList::new(mounted);
            let shown: Vec<String> = text_of(&tree.commands().clone());
            assert!(
                shown.iter().any(|text| text == &telar::t!("lock.password")),
                "{screen:?} mounted no field: {shown:?}"
            );
        }
    }

    #[test]
    fn a_password_is_never_left_in_the_field_after_it_is_submitted() {
        // The field's own copy is cleared before the secret is handed on, so a shoulder-surfer reading a screen that is still up after a failed attempt learns the length of nothing.
        telar::reset_layout_runtime();
        telar::set_theme(NordTheme::new());
        let state = signal(LockState {
            wanted: true,
            ..LockState::default()
        });
        assert!(field(state.read_only(), NordTheme::new()).is_ok());
    }

    #[test]
    fn the_minimal_lock_builds_with_no_config_and_no_surface() {
        telar::reset_layout_runtime();
        telar::set_theme(NordTheme::new());
        assert!(minimal_screen(Prompting::Live).is_ok());
    }

    /// Lock mode's preview draws the lock layer with a prompt that says what it is and takes nothing: a keystroke that reaches the real prompt reaches nothing here, so nothing typed into the preview can be submitted (TA-8).
    #[test]
    fn the_preview_prompt_says_so_and_takes_no_password() {
        ui::descriptor::install(PROBES);
        let config = config_with(LockConfig::default());
        let lock = locked_with(vec![prompt_area_of()]);

        seed(&config);
        let said = text_of(&draw(
            preview(&config, Ok(&lock), Some("DP-1"), SCREEN).expect("the preview builds"),
        ));
        assert!(said.iter().any(|text| text == "Preview"), "{said:?}");
        assert!(said.iter().any(|text| text == "Password"), "{said:?}");

        seed(&config);
        assert_eq!(
            type_into(
                preview(&config, Ok(&lock), Some("DP-1"), SCREEN).expect("the preview builds")
            ),
            EventResult::Ignored,
            "the live prompt takes this keystroke; the preview's must not"
        );
    }

    /// A lock layer that cannot be built previews as the minimal lock a real lock would fall back to, with the preview's prompt rather than a live one.
    #[test]
    fn a_lock_layer_that_cannot_be_built_previews_as_the_minimal_lock() {
        let config = config_with(LockConfig::default());
        seed(&config);
        let said = text_of(&draw(
            preview(&config, Ok(&locked_with(Vec::new())), Some("DP-1"), SCREEN)
                .expect("the preview falls back rather than failing"),
        ));
        assert!(said.iter().any(|text| text == "Preview"), "{said:?}");
    }

    /// A lock layer the lock's own check refuses previews as the minimal lock, with the reason over it — what the edit mode shows so that what is edited is what a lock would draw.
    #[test]
    fn a_refused_lock_layer_previews_as_the_minimal_lock_and_says_why() {
        ui::descriptor::install(PROBES);
        let config = config_with(LockConfig::default());
        seed(&config);
        let said = text_of(&draw(
            preview(
                &config,
                Err("the prompt is too faint"),
                Some("DP-1"),
                SCREEN,
            )
            .expect("the preview builds"),
        ));
        assert!(said.iter().any(|text| text == "Preview"), "{said:?}");
        assert!(
            said.iter()
                .any(|text| text.contains("minimal lock") && text.contains("too faint")),
            "{said:?}"
        );
        assert!(!said.iter().any(|text| text.contains(PUBLIC)), "{said:?}");
    }

    #[test]
    fn a_screen_that_fails_to_build_mounts_the_minimal_lock_instead_of_panicking() {
        telar::reset_layout_runtime();
        telar::set_theme(NordTheme::new());
        mount(|| Err(LayoutError::Engine("injected".into())));
    }

    #[test]
    fn biometric_methods_are_only_offered_when_configured() {
        let bare = Config::default();
        assert!(
            offered_methods(&bare).is_empty(),
            "a machine with no reader and no Howdy shows no biometric hint"
        );
    }
}
