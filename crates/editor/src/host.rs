//! The edit mode's host: what it holds on the compositor while a mode is up, and the tree it draws over the edited screen.
//!
//! The host is one unanchored transient on the edited output's overlay window, taking the keyboard exclusively so Esc and modifiers arrive without a click (TA-4, F-6.9). Being a transient is also what puts it on the dismiss stack: Esc closes whatever opened after it first — a popover, a menu, a tool's own transaction registered there, a keyboard edit still held, the selection — and closes the host only once nothing is above it.
//!
//! **The edited layer is shown for real, not as a proxy.** The background and desktop windows are raised above the user's windows for the session; where the compositor cannot move a layer window, the same areas are built into the host instead and the window underneath sets its own aside (R-6). The top and overlay windows are above application windows already. The lock layer has no window of its own on an unlocked session, so its mode draws a preview of it (TA-8).
//!
//! Everything the host takes is a token held by its [`Session`], and dropping the session gives every one of them back.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::Arc;

use telar::{
    AlignItems, Border, Children, Color, JustifyContent, LayoutItem, LayoutStyle, ReactiveList,
    RectStyle, SizeDimension, StyledContainer, use_theme,
};

use config::theme::NordTheme;
use layout::{LayerKind, Layout, Within};
use modules::lock::LockLayout;
use platform_wayland::{KeyboardMode, Layer};
use surfaces::layer_window::{Concealment, Demand, Hold};
use surfaces::reconcile::{self, Desktop};
use surfaces::rects::{self, Part};
use surfaces::transient::{self, Place, Spec};
use ui::chrome::Chrome;
use ui::descriptor::Built;

use crate::indicator::{IndicatorProps, indicator};
use crate::mode::{self, Mode};
use crate::session;

/// The layer the background and desktop windows are raised to for their mode.
///
/// Top rather than Overlay, which is where the host is: Hyprland appends a window moved to another layer to the end of that layer's list (`CLayerSurface::onCommit`, `emplace_back`), so a window raised into Overlay would stack over any overlay window already open — a stack card, an area above fullscreen, the host itself. Top is above every application window, and the host stays above it.
const RAISED_TO: Layer = Layer::Top;

/// How thick the accent border around the edited screen is.
const BORDER: f32 = 2.0;

/// What builds one layer's tools inside the host, given the mode they are built for.
pub type Tool = fn(&Mode) -> Built;

thread_local! {
    static TOOLS: RefCell<Vec<(LayerKind, Tool)>> = const { RefCell::new(Vec::new()) };
    static SERIAL: Cell<u64> = const { Cell::new(0) };
    static STAND_IN_SAID: Cell<bool> = const { Cell::new(false) };
}

/// Mounts `tool` in the host of every `layer` mode, above the edited layer and the reference outlines and below the strip. Tools mount in the order they were added, each over the ones before it, in a box the size of the output that takes no pointer itself.
pub fn add_tool(layer: LayerKind, tool: Tool) {
    TOOLS.with(|tools| tools.borrow_mut().push((layer, tool)));
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
enum Under {
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
fn tree(mode: &Mode, under: Under) -> Built {
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
        Under::LockPreview => layers.push(lock_preview(&mode.output)?),
        Under::Nothing => {}
    }
    if mode.layer != LayerKind::Lock {
        layers.push(outlines(mode, theme)?);
    }
    if mode.refused.is_none() {
        layers.push(tools(mode)?);
    }
    layers.push(border(theme)?);
    layers.push(strip(mode)?);
    Ok(Box::new(
        passthrough(whole(), layers)?.on_key(crate::keys::on_key),
    ))
}

/// The edited screen's arrangement as its windows show it, an undecided edit's preview included.
fn edited(output: &str) -> Option<Desktop> {
    reconcile::desktops()
        .iter()
        .find(|desktop| desktop.output.as_deref() == Some(output))
        .cloned()
}

/// What `build` makes of the edited screen's arrangement, built again whenever that arrangement changes and only then — a monitor plugged in elsewhere changes nothing here.
fn following(output: &str, build: impl Fn(&Desktop) -> Built + 'static) -> Built {
    let output = output.to_string();
    Ok(Box::new(ReactiveList::with_style(
        whole(),
        move || edited(&output).into_iter().collect(),
        arrangement,
        move |desktop: Desktop| build(&desktop),
    )?))
}

fn arrangement(desktop: &Desktop) -> String {
    format!(
        "{:?}{:?}{:?}{:p}",
        desktop.resolved,
        desktop.size,
        desktop.reserved,
        Arc::as_ptr(&desktop.config)
    )
}

/// The lock layer of the layout being drawn, previewed over the whole screen (TA-8): the draft's, so an undecided edit shows here as it would on the lock screen, built again whenever the draft or the screen changes.
fn lock_preview(output: &str) -> Built {
    let output = output.to_string();
    let draft = session::draft();
    let seen: RefCell<(u64, Option<Layout>)> = RefCell::new((0, None));
    Ok(Box::new(ReactiveList::with_style(
        whole(),
        move || {
            let mut seen = seen.borrow_mut();
            let changed = draft.with(|layout| seen.1.as_ref() != Some(layout));
            if changed {
                *seen = (seen.0.wrapping_add(1), Some(draft.peek()));
            }
            let version = seen.0;
            edited(&output)
                .map(|desktop| (version, desktop))
                .into_iter()
                .collect()
        },
        |(version, desktop): &(u64, Desktop)| (*version, arrangement(desktop)),
        move |(_, desktop): (u64, Desktop)| {
            let layout = draft.peek();
            let lock = surfaces::layouts::read(|store| LockLayout::of(&layout, store.all()))
                .unwrap_or_else(|| LockLayout::of(&layout, &BTreeMap::new()));
            modules::lock::preview(
                &desktop.config,
                &lock,
                desktop.output.as_deref(),
                desktop.size,
            )
        },
    )?))
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

fn tools(mode: &Mode) -> Built {
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
            let usable = edited(&output)
                .map(|desktop| desktop.reserved.box_of(Within::Usable, desktop.size))
                .unwrap_or_default();
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

/// A box that paints nothing and takes nothing from the pointer, so the windows and layers under the host still answer wherever nothing in it does (F-10.41).
pub(crate) fn passthrough(
    style: LayoutStyle,
    children: Vec<Box<dyn LayoutItem>>,
) -> Result<StyledContainer, telar::LayoutError> {
    Ok(StyledContainer::new(style, |_| RectStyle::default(), children)?.input_transparent())
}

pub(crate) fn whole() -> LayoutStyle {
    LayoutStyle::new()
        .absolute()
        .inset_start(0.0)
        .inset_top(0.0)
        .width(SizeDimension::Percent(1.0))
        .height(SizeDimension::Percent(1.0))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use telar::{
        AvailableSpace, ComponentList, Container, DrawCommand, Text, box_item, compute_layout,
        new_container,
    };

    use config::Config;
    use config::theme::FontRole;

    use super::*;

    const SCREEN: (f32, f32) = (1920.0, 1080.0);

    fn marker(said: &'static str) -> Built {
        let theme = use_theme::<NordTheme>();
        Ok(box_item(Text::new(
            move || said.to_string(),
            LayoutStyle::new(),
            move || theme.text_style(FontRole::Body, theme.text),
        )?))
    }

    fn desktop_tool(_: &Mode) -> Built {
        marker("a desktop tool")
    }

    fn top_tool(_: &Mode) -> Built {
        marker("a top tool")
    }

    fn lock_tool(_: &Mode) -> Built {
        marker("a lock tool")
    }

    /// Everything the host's tree says, laid out over a whole screen.
    fn said(mode: &Mode, under: Under) -> Vec<String> {
        telar::reset_layout_runtime();
        telar::set_locale("en");
        telar::set_theme(Arc::new(Config::default()).resolve_theme());
        let item = tree(mode, under).expect("the host builds");
        let page = || LayoutStyle::new().width(SCREEN.0).height(SCREEN.1);
        let root = new_container(page(), &[item.layout_node()]).expect("a root");
        let tree = ComponentList::new(Container::new(page(), vec![item]).expect("a page"));
        compute_layout(
            root,
            AvailableSpace::Definite(SCREEN.0),
            AvailableSpace::Definite(SCREEN.1),
        )
        .expect("the host lays out");
        tree.commands()
            .iter()
            .filter_map(|command| match command {
                DrawCommand::Text { text, .. } => Some(text.to_string()),
                _ => None,
            })
            .collect()
    }

    /// A mode's tools mount in its host and only there: the strip names the mode, the screen and the way out, and a tool added for another layer is nowhere to be seen.
    #[test]
    fn a_mode_mounts_its_own_tools_under_its_strip() {
        add_tool(LayerKind::Desktop, desktop_tool);
        add_tool(LayerKind::Top, top_tool);
        let mode = Mode {
            layer: LayerKind::Desktop,
            output: "DP-1".to_string(),
            refused: None,
        };
        let said = said(&mode, Under::Nothing);
        for expected in ["Desktop", "DP-1", "Done", "a desktop tool"] {
            assert!(
                said.iter().any(|text| text == expected),
                "{expected}: {said:?}"
            );
        }
        assert!(!said.iter().any(|text| text == "a top tool"), "{said:?}");
    }

    /// TA-8: lock mode on a machine that cannot lock opens, and says why where its tools would be, instead of offering tools for a lock screen it could never show.
    #[test]
    fn a_refused_lock_mode_says_why_instead_of_mounting_tools() {
        add_tool(LayerKind::Lock, lock_tool);
        let mode = Mode {
            layer: LayerKind::Lock,
            output: "DP-1".to_string(),
            refused: Some("Preview only: this machine cannot lock (no PAM)".to_string()),
        };
        let said = said(&mode, Under::Nothing);
        assert!(said.iter().any(|text| text.contains("no PAM")), "{said:?}");
        assert!(!said.iter().any(|text| text == "a lock tool"), "{said:?}");
        assert!(
            said.iter().any(|text| text == "Done"),
            "the way out is still there"
        );
    }
}
