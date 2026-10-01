//! The edit mode's host: what it holds on the compositor while a mode is up, and the tree it draws over the edited screen.
//!
//! The host is one unanchored transient on the edited output's overlay window, taking the keyboard exclusively so Esc and modifiers arrive without a click (TA-4, F-6.9). Being a transient is also what puts it on the dismiss stack: Esc closes whatever opened after it first — a popover, a menu, a tool's own transaction registered there, a keyboard edit still held, the selection — and closes the host only once nothing is above it.
//!
//! **The edited layer is shown for real, not as a proxy.** The background and desktop windows are raised above the user's windows for the session; where the compositor cannot move a layer window, the same areas are built into the host instead and the window underneath sets its own aside (R-6). The top and overlay windows are above application windows already. The lock layer has no window of its own on an unlocked session, so its mode draws a preview of it (TA-8).
//!
//! Everything the host takes is a token held by its [`Session`], and dropping the session gives every one of them back.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use telar::{
    AlignItems, Border, Children, Color, JustifyContent, LayoutItem, LayoutStyle, ReactiveList,
    RectStyle, SizeDimension, StyledContainer, use_theme,
};

use config::theme::NordTheme;
use layout::{LayerKind, Within};
use platform_wayland::{KeyboardMode, Layer};
use surfaces::layer_window::{Concealment, Demand, Hold};
use surfaces::reconcile::{self, Desktop};
use surfaces::rects::{self, Part};
use surfaces::transient::{self, Place, Spec};
use ui::chrome::Chrome;
use ui::descriptor::Built;

use crate::indicator::{IndicatorProps, indicator};
use crate::mode::{self, Mode};

/// The layer the background and desktop windows are raised to for their mode.
///
/// Top rather than Overlay, which is where the host is: Hyprland appends a window moved to another layer to the end of that layer's list (`CLayerSurface::onCommit`, `emplace_back`), so a window raised into Overlay would stack over any overlay window already open — a stack card, an area above fullscreen, the host itself. Top is above every application window, and the host stays above it.
const RAISED_TO: Layer = Layer::Top;

/// How thick the accent border around the edited screen is.
const BORDER: f32 = 2.0;

/// What builds one layer's tools inside the host, given the mode they are built for.
pub type Tool = fn(&Mode) -> Built;

/// A button of a mode's toolbar: what it says, and what a press does.
pub(crate) type ToolbarButton = (fn() -> String, fn());

/// How far above the foot of what the reserving areas leave the toolbar sits.
const TOOLBAR_RISE: f32 = 64.0;

/// How big a split or join button is across.
pub(crate) const HOTSPOT: f32 = 28.0;

thread_local! {
    static TOOLS: RefCell<Vec<(LayerKind, Tool)>> = const { RefCell::new(Vec::new()) };
    static TOOLBAR: RefCell<Vec<(LayerKind, ToolbarButton)>> = const { RefCell::new(Vec::new()) };
    static SERIAL: Cell<u64> = const { Cell::new(0) };
    static STAND_IN_SAID: Cell<bool> = const { Cell::new(false) };
}

/// Mounts `tool` in the host of every `layer` mode, above the edited layer and the reference outlines and below the strip. Tools mount in the order they were added, each over the ones before it, in a box the size of the output that takes no pointer itself.
pub fn add_tool(layer: LayerKind, tool: Tool) {
    TOOLS.with(|tools| tools.borrow_mut().push((layer, tool)));
}

/// Adds `button` to the toolbar of every `layer` mode, after the buttons added before it. A mode with no buttons has no toolbar.
pub(crate) fn add_toolbar_button(layer: LayerKind, button: ToolbarButton) {
    TOOLBAR.with(|buttons| buttons.borrow_mut().push((layer, button)));
}

/// The buttons of the `layer` mode's toolbar, in order.
pub(crate) fn toolbar_of(layer: LayerKind) -> Vec<ToolbarButton> {
    TOOLBAR.with(|buttons| {
        buttons
            .borrow()
            .iter()
            .filter(|(of, _)| *of == layer)
            .map(|(_, button)| *button)
            .collect()
    })
}

/// Everything one mode holds on the compositor, given back when it is dropped.
pub(crate) struct Session {
    serial: u64,
    transient: String,
    _raise: Option<Demand>,
    _hold: Option<Hold>,
    _conceal: Option<Concealment>,
}

impl Session {
    pub(crate) fn serial(&self) -> u64 {
        self.serial
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        transient::close(&self.transient);
    }
}

/// What the host draws under its chrome.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Under {
    /// The edited layer is on screen in its own window.
    Nothing,
    /// The edited layer's areas, built here because its window could not be raised.
    StandIn,
    LockPreview,
}

/// Opens the host for `mode`: raises or stands in for the edited window, keeps it on screen, and puts the chrome up. `restack` is whether the compositor can move a layer window to another layer.
pub(crate) fn open(mode: &Mode, restack: bool) -> Session {
    let serial = SERIAL.with(|next| {
        next.set(next.get() + 1);
        next.get()
    });
    let output = Some(mode.output.as_str());
    let raised = matches!(mode.layer, LayerKind::Background | LayerKind::Desktop);
    let under = match mode.layer {
        LayerKind::Lock => Under::LockPreview,
        _ if raised && !restack => Under::StandIn,
        _ => Under::Nothing,
    };
    let raise = (raised && restack)
        .then(|| transient::demands(output, mode.layer))
        .flatten()
        .map(|demands| demands.raise(RAISED_TO));
    // So a layer with nothing on it yet is still a window to edit.
    let hold = (under == Under::Nothing)
        .then(|| transient::hold(output, mode.layer))
        .flatten();
    let conceal = (under == Under::StandIn)
        .then(|| transient::conceal(output, mode.layer))
        .flatten();
    if under == Under::StandIn {
        say_standing_in(mode.layer);
    }

    let id = transient_id(&mode.output);
    let shown = mode.clone();
    transient::open(
        Spec::new(
            id.clone(),
            Place::Whole,
            Rc::new(move |_: &Chrome| tree(&shown, under)),
        )
        .output(Some(mode.output.clone()))
        .keyboard(KeyboardMode::Exclusive)
        .on_close(move || mode::leave_session(serial)),
    );
    Session {
        serial,
        transient: id,
        _raise: raise,
        _hold: hold,
        _conceal: conceal,
    }
}

/// Closes whichever of the transients an edit mode opens over its host is open — a popover, a menu, the palette and what it picked, the privacy card — keeping what each changed, so the one opening next is the only one.
pub(crate) fn close_transients() {
    crate::popover::close();
    transient::close(crate::context::ID);
    transient::close(crate::modes::palette::ID);
    crate::modes::palette::unpick();
    transient::close(crate::modes::lock::PRIVACY);
}

/// The transient the host of a mode on `output` is.
pub(crate) fn transient_id(output: &str) -> String {
    format!("edit:{output}")
}

/// R-6: said once, because it is a fact about the compositor rather than about any one session.
fn say_standing_in(layer: LayerKind) {
    if STAND_IN_SAID.with(|said| said.replace(true)) {
        return;
    }
    tracing::warn!(
        layer = %layer,
        "the compositor cannot move a layer window to another layer (zwlr_layer_shell_v1 below version 2), so edit modes draw the background and desktop layers in the overlay window instead of raising them"
    );
}

/// The host's whole tree, bottom to top: what stands in for the edited layer, the other layers' outlines, the tools, the border and the strip.
pub(crate) fn tree(mode: &Mode, under: Under) -> Built {
    crate::keys::reclaim();
    let theme = use_theme::<NordTheme>();
    let mut layers: Vec<Box<dyn LayoutItem>> = Vec::new();
    match under {
        Under::StandIn => {
            let layer = mode.layer;
            layers.push(following(&mode.output, move |desktop| {
                surfaces::area::stand_in(desktop, layer)
            })?);
        }
        Under::LockPreview => layers.push(crate::modes::lock::preview(&mode.output)?),
        Under::Nothing => {}
    }
    if mode.layer != LayerKind::Lock {
        layers.push(outlines(mode, theme)?);
    }
    if mode.refused.is_none() {
        layers.push(tools(mode)?);
        layers.push(toolbar(mode)?);
    }
    layers.push(border(theme)?);
    layers.push(strip(mode)?);
    Ok(Box::new(
        passthrough(whole(), layers)?.on_key(crate::keys::on_key),
    ))
}

/// What the reserving areas of the screen `output` leave, as its windows draw it now. Reactive.
pub(crate) fn usable(output: Option<&str>) -> telar::Rect {
    reconcile::desktop(output)
        .map(|desktop| desktop.reserved.box_of(Within::Usable, desktop.size))
        .unwrap_or_default()
}

/// What `build` makes of the edited screen's arrangement, built again whenever that arrangement changes and only then — a monitor plugged in elsewhere changes nothing here.
fn following(output: &str, build: impl Fn(&Desktop) -> Built + 'static) -> Built {
    let output = output.to_string();
    Ok(Box::new(ReactiveList::with_style(
        whole(),
        move || reconcile::desktop(Some(&output)).into_iter().collect(),
        arrangement,
        move |desktop: Desktop| build(&desktop),
    )?))
}

pub(crate) fn arrangement(desktop: &Desktop) -> String {
    format!(
        "{:?}{:?}{:?}{:p}",
        desktop.resolved,
        desktop.size,
        desktop.reserved,
        Arc::as_ptr(&desktop.config)
    )
}

/// Every area of the layers not being edited, as a dimmed outline where it is on screen: there for reference, and never in the way of the pointer (F-7).
fn outlines(mode: &Mode, theme: NordTheme) -> Built {
    let (output, layer) = (mode.output.clone(), mode.layer);
    let list = ReactiveList::with_style(
        whole(),
        move || others(&output, layer),
        |node: &rects::Node| node.clone(),
        move |node: rects::Node| outline(node, theme),
    )?;
    Ok(Box::new(passthrough(whole(), vec![Box::new(list)])?))
}

fn others(output: &str, edited: LayerKind) -> Vec<rects::Node> {
    let mut nodes: Vec<rects::Node> = Vec::new();
    let areas = LayerKind::SESSION
        .into_iter()
        .filter(|layer| *layer != edited)
        .flat_map(|layer| rects::on(Some(output), layer))
        .filter(|(node, _)| node.part == Part::Area);
    for (node, _) in areas {
        if !nodes.contains(&node) {
            nodes.push(node);
        }
    }
    nodes
}

fn outline(node: rects::Node, theme: NordTheme) -> Built {
    let fill = theme.base.with_alpha(0.2);
    let edge = theme.muted.with_alpha(0.7);
    let radius = ui::scale::corner::xs();
    Ok(Box::new(
        StyledContainer::new(
            LayoutStyle::new(),
            move |_| RectStyle::filled(fill, radius).with_border(Border::uniform(edge, 1.0)),
            Vec::new(),
        )?
        .styled_by(move || surfaces::area::at(rects::rect(&node).unwrap_or_default()))
        .input_transparent(),
    ))
}

pub(crate) fn tools(mode: &Mode) -> Built {
    let tools: Vec<Tool> = TOOLS.with(|tools| {
        tools
            .borrow()
            .iter()
            .filter(|(layer, _)| *layer == mode.layer)
            .map(|(_, tool)| *tool)
            .collect()
    });
    let built = tools
        .into_iter()
        .map(|tool| ui::chrome::or_empty("edit tool", tool(mode)))
        .collect();
    Ok(Box::new(passthrough(whole(), built)?))
}

/// The mode's toolbar, at the foot of what the reserving areas leave: the buttons added for its layer ([`add_toolbar_button`]).
fn toolbar(mode: &Mode) -> Built {
    let added = toolbar_of(mode.layer);
    if added.is_empty() {
        return Ok(Box::new(passthrough(LayoutStyle::new(), Vec::new())?));
    }
    let buttons = added
        .into_iter()
        .map(|(label, act)| {
            telar::button(
                telar::ButtonProps::props()
                    .label(telar::Reactive::of(label))
                    .on_press(Rc::new(act))
                    .build(),
                Children::default(),
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    let theme = use_theme::<NordTheme>();
    let bar = StyledContainer::new(
        LayoutStyle::new()
            .flex_row()
            .gap(ui::scale::space::sm())
            .padding_all(ui::scale::space::sm()),
        move |_| RectStyle::filled(theme.surface, ui::scale::corner::md()),
        buttons,
    )?
    .input_opaque();
    let output = mode.output.clone();
    Ok(Box::new(
        passthrough(LayoutStyle::new(), vec![Box::new(bar)])?.styled_by(move || {
            let usable = usable(Some(&output));
            LayoutStyle::new()
                .absolute()
                .inset_start(usable.x)
                .inset_top(usable.y + usable.height - TOOLBAR_RISE)
                .width(usable.width)
                .flex_row()
                .justify_content(JustifyContent::CENTER)
                .align_items(AlignItems::START)
        }),
    ))
}

/// The 2 px accent border around the edited screen, the half of the indicator nobody has to read (F-7).
fn border(theme: NordTheme) -> Built {
    Ok(Box::new(
        StyledContainer::new(
            whole(),
            move |_| {
                RectStyle::filled(Color::TRANSPARENT, 0.0)
                    .with_border(Border::uniform(theme.accent, BORDER))
            },
            Vec::new(),
        )?
        .input_transparent(),
    ))
}

/// The strip, centred at the top of what the screen's reserving areas leave, so it never sits over a bar being edited.
fn strip(mode: &Mode) -> Built {
    let shown = indicator(
        IndicatorProps::props()
            .layer(mode.layer)
            .output(mode.output.clone())
            .refused(mode.refused.clone())
            .build(),
        Children::default(),
    )?;
    let output = mode.output.clone();
    Ok(Box::new(
        passthrough(LayoutStyle::new(), vec![shown])?.styled_by(move || {
            let usable = usable(Some(&output));
            LayoutStyle::new()
                .absolute()
                .inset_start(usable.x)
                .inset_top(usable.y + ui::scale::space::lg())
                .width(usable.width)
                .flex_row()
                .justify_content(JustifyContent::CENTER)
                .align_items(AlignItems::START)
        }),
    ))
}

/// `target`, `side` across, drawn with its middle wherever `at` answers as it is painted, and off the screen while it answers `None`.
pub(crate) fn centred(
    target: StyledContainer,
    side: f32,
    at: impl Fn() -> Option<(f32, f32)> + 'static,
) -> StyledContainer {
    target.with_transform(move |laid| {
        let (x, y) = at().unwrap_or((-2.0 * side, -2.0 * side));
        Some([
            1.0,
            0.0,
            0.0,
            1.0,
            x - side / 2.0 - laid.x,
            y - side / 2.0 - laid.y,
        ])
    })
}

/// A split or join button on the screen itself (F-7): a disc with `mark` drawn in it, its middle wherever `at` answers.
pub(crate) fn hotspot(
    mark: Box<dyn LayoutItem>,
    at: impl Fn() -> Option<(f32, f32)> + 'static,
) -> Result<StyledContainer, telar::LayoutError> {
    let theme = use_theme::<NordTheme>();
    let face = StyledContainer::new(
        LayoutStyle::new()
            .absolute()
            .inset_start(0.0)
            .inset_top(0.0)
            .width(HOTSPOT)
            .height(HOTSPOT)
            .flex_row()
            .align_items(AlignItems::CENTER)
            .justify_content(JustifyContent::CENTER),
        move |_| {
            RectStyle::filled(theme.surface, HOTSPOT / 2.0)
                .with_border(Border::uniform(theme.accent, 2.0))
        },
        vec![mark],
    )?;
    Ok(centred(face, HOTSPOT, at).control(telar::Role::Button))
}

/// The mark of a hotspot that cuts: a line across it, upright where the cut is.
pub(crate) fn cut_mark(upright: bool) -> Built {
    let (width, height) = match upright {
        true => (2.0, HOTSPOT / 2.0),
        false => (HOTSPOT / 2.0, 2.0),
    };
    mark(width, height, 1.0)
}

/// The mark of a hotspot that joins: a dot.
pub(crate) fn join_mark() -> Built {
    mark(HOTSPOT / 3.0, HOTSPOT / 3.0, HOTSPOT / 6.0)
}

fn mark(width: f32, height: f32, radius: f32) -> Built {
    let theme = use_theme::<NordTheme>();
    Ok(Box::new(StyledContainer::new(
        LayoutStyle::new().width(width).height(height),
        move |_| RectStyle::filled(theme.accent, radius),
        Vec::new(),
    )?))
}

/// A box that paints nothing and takes nothing from the pointer, so the windows and layers under the host still answer wherever nothing in it does (F-10.41).
pub(crate) fn passthrough(
    style: LayoutStyle,
    children: Vec<Box<dyn LayoutItem>>,
) -> Result<StyledContainer, telar::LayoutError> {
    Ok(StyledContainer::new(style, |_| RectStyle::default(), children)?.input_transparent())
}

/// `item` laid over the whole screen, taking the pointer only where something in it does. A box answers for every point it covers, so a full-screen list laid bare over another tool's controls would take every press meant for them.
pub(crate) fn see_through(
    item: impl LayoutItem + 'static,
) -> Result<Box<dyn LayoutItem>, telar::LayoutError> {
    Ok(Box::new(passthrough(whole(), vec![Box::new(item)])?))
}

pub(crate) fn whole() -> LayoutStyle {
    LayoutStyle::new()
        .absolute()
        .inset_start(0.0)
        .inset_top(0.0)
        .width(SizeDimension::Percent(1.0))
        .height(SizeDimension::Percent(1.0))
}
