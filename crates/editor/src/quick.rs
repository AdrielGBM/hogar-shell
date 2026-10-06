use std::cell::{Cell, RefCell};
use std::rc::Rc;

use telar::{
    Accessible, AlignItems, Border, Color, Cursor, DismissRegistration, JustifyContent, Key,
    LayoutItem, LayoutStyle, NamedKey, NodeId, ReactiveList, Rect, RectStyle, RwSignal,
    StyledContainer, detached, effect, focus, signal, use_theme,
};

use config::theme::NordTheme;
use layout::LayerKind;
use surfaces::reconcile;
use surfaces::rects::{self, Node};
use ui::descriptor::Built;

use crate::host::{see_through, whole};
use crate::keys::{self, Chord, KeyOp, Run};
use crate::mode::{self, Mode};
use crate::modes::gesture;
use crate::session::{self, EditError, Selection};
use crate::tools::{self, Tool};

pub(crate) const BUTTON: f32 = 28.0;
const ICON: f32 = 16.0;
const SPACING: f32 = 2.0;
const PAD: f32 = 4.0;
pub(crate) const CLEARANCE: f32 = 8.0;
pub(crate) const INSET: f32 = 12.0;
pub(crate) const TALL: f32 = 1.25;
pub(crate) const SIZE_TAG_ROOM: f32 = 32.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Button {
    Customize,
    Panel,
    Radius,
    Padding,
    Link,
    Remove,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Bar {
    node: Node,
    buttons: Vec<Button>,
    vertical: bool,
}

thread_local! {
    static KEYBOARD: RwSignal<Option<usize>> = detached(|| signal(None));
    static FACES: RefCell<Vec<NodeId>> = const { RefCell::new(Vec::new()) };
    static OFFSETS: RwSignal<Vec<(Node, (f32, f32))>> = detached(|| signal(Vec::new()));
    static GRAB: Cell<Option<(f32, f32)>> = const { Cell::new(None) };
    static FOLD: RefCell<Option<DismissRegistration>> = const { RefCell::new(None) };
}

pub(crate) fn install() {
    for layer in LayerKind::ALL {
        keys::add_mode_key_op(
            layer,
            KeyOp {
                name: "quick-bar",
                keys: vec![Chord::char('.')],
                label: || telar::t!("editor.quick.key"),
                run: Run::Act(take_keyboard),
            },
        );
    }
    detached(|| {
        effect(|| {
            mode::active().with(|_| ());
            OFFSETS.with(|offsets| {
                if offsets.peek_with(|held| !held.is_empty()) {
                    offsets.set(Vec::new());
                }
            });
        });
        let last: RefCell<Option<Node>> = RefCell::default();
        effect(move || {
            let now = session::selection().get().node().cloned();
            if *last.borrow() != now {
                *last.borrow_mut() = now;
                release();
            }
        });
        effect(|| {
            let held = KEYBOARD.with(|keyboard| keyboard.get()).is_some();
            FOLD.with(|fold| {
                let folded = fold.borrow().is_some();
                match (held, folded) {
                    (true, false) => {
                        *fold.borrow_mut() = Some(DismissRegistration::new(Rc::new(release)))
                    }
                    (false, true) => drop(fold.borrow_mut().take()),
                    _ => {}
                }
            });
        });
    });
}

#[cfg(test)]
pub(crate) fn keyboard() -> Option<usize> {
    KEYBOARD.with(|keyboard| keyboard.get())
}

fn held() -> Option<usize> {
    KEYBOARD.with(|keyboard| keyboard.peek())
}

fn mark(at: Option<usize>) {
    KEYBOARD.with(|keyboard| {
        if keyboard.peek() != at {
            keyboard.set(at);
        }
    });
}

fn focus_face(at: usize) {
    mark(Some(at));
    if let Some(face) = FACES.with(|faces| faces.borrow().get(at).copied()) {
        focus::focus_first_in(face);
    }
}

pub(crate) fn release() {
    if held().is_some() {
        mark(None);
        focus::clear();
    }
}

fn take_keyboard(selection: &Selection) -> Result<(), EditError> {
    if shown_for(selection).is_none() {
        return Err(EditError::nothing());
    }
    focus_face(0);
    Ok(())
}

fn on_face_key(button: Button, key: &Key) -> bool {
    let Some(at) = held() else {
        return false;
    };
    let count = FACES.with(|faces| faces.borrow().len());
    if count == 0 {
        return false;
    }
    let last = count - 1;
    let at = at.min(last);
    let next = match key {
        Key::Named(NamedKey::ArrowRight | NamedKey::ArrowDown) => (at + 1) % count,
        Key::Named(NamedKey::ArrowLeft | NamedKey::ArrowUp) => at.checked_sub(1).unwrap_or(last),
        Key::Named(NamedKey::Home) => 0,
        Key::Named(NamedKey::End) => last,
        Key::Named(NamedKey::Enter | NamedKey::Space) => {
            press(button);
            return true;
        }
        Key::Char('.') => {
            release();
            return true;
        }
        _ => return false,
    };
    focus_face(next);
    true
}

fn on_face_focus(at: usize, face: NodeId, focused: bool) {
    let current = FACES.with(|faces| faces.borrow().get(at) == Some(&face));
    match focused {
        true if current && focus::current().is_some_and(focus::is_focus_visible) => mark(Some(at)),
        false if current && held() == Some(at) => mark(None),
        _ => {}
    }
}

pub(crate) fn buttons(selection: &Selection) -> Vec<Button> {
    let Some(node) = selection.node() else {
        return Vec::new();
    };
    let mut shown = vec![Button::Customize];
    if crate::panel::offered(node).is_some() {
        shown.push(Button::Panel);
    }
    if tools::offers(selection, Tool::Radius) {
        shown.push(Button::Radius);
    }
    if tools::offers(selection, Tool::Padding) {
        shown.push(Button::Padding);
    }
    if tools::active().is_some() {
        shown.push(Button::Link);
    }
    if removable(selection, node) {
        shown.push(Button::Remove);
    }
    shown
}

fn removable(selection: &Selection, node: &Node) -> bool {
    let draft = session::draft().get();
    match selection {
        Selection::None => false,
        Selection::Instance(_) => reconcile::desktop(node.output.as_deref())
            .is_some_and(|desktop| crate::context::removal(&draft, &desktop, node).is_ok()),
        Selection::Area(_) | Selection::Group(_) => {
            crate::steps::removal(selection, &draft).is_ok()
        }
    }
}

fn shown_for(selection: &Selection) -> Option<Bar> {
    if gesture::dragging() || crate::popover::customizing().is_some() {
        return None;
    }
    let node = selection.node()?.clone();
    if !mode::editing(&node) {
        return None;
    }
    let rect = rects::rect(&node)?;
    Some(Bar {
        buttons: buttons(selection),
        vertical: rect.height > rect.width * TALL,
        node,
    })
}

pub(crate) fn tool(mode: &Mode) -> Built {
    let output = mode.output.clone();
    let list = ReactiveList::with_style(
        whole(),
        || shown_for(&session::selection().get()).into_iter().collect(),
        |bar: &Bar| bar.clone(),
        move |bar: Bar| drawn(bar, output.clone()),
    )?;
    see_through(list)
}

fn drawn(bar: Bar, output: String) -> Built {
    let theme = use_theme::<NordTheme>();
    let mut items: Vec<Box<dyn LayoutItem>> = vec![grip(bar.node.clone(), bar.vertical, theme)?];
    let mut faces = Vec::new();
    for (at, button) in bar.buttons.iter().enumerate() {
        let built = face(*button, at, bar.node.clone(), theme)?;
        faces.push(built.layout_node());
        items.push(built);
    }
    let style = LayoutStyle::new()
        .absolute()
        .inset_start(0.0)
        .inset_top(0.0)
        .gap(SPACING)
        .padding_all(PAD)
        .align_items(AlignItems::CENTER);
    let style = match bar.vertical {
        true => style.flex_column(),
        false => style.flex_row(),
    };
    let last = bar.buttons.len().saturating_sub(1);
    let Bar { node, vertical, .. } = bar;
    let drawn = StyledContainer::new(
        style,
        move |_| {
            RectStyle::filled(theme.surface, ui::scale::corner::md())
                .with_border(Border::uniform(theme.overlay, 1.0))
        },
        items,
    )?
    .input_opaque()
    .a11y_label(|| telar::t!("editor.quick.bar"))
    .with_transform(move |laid| {
        let (x, y) = placed_now(&node, vertical, (laid.width, laid.height), &output);
        Some([1.0, 0.0, 0.0, 1.0, x - laid.x, y - laid.y])
    });
    FACES.with(|kept| *kept.borrow_mut() = faces);
    if let Some(at) = held() {
        focus_face(at.min(last));
    }
    Ok(Box::new(drawn))
}

fn grip(node: Node, vertical: bool, theme: NordTheme) -> Built {
    let name = match vertical {
        true => "grip-horizontal",
        false => "grip-vertical",
    };
    let icon = ui::icon::icon_view(move || name.to_string(), move || theme.subtle, ICON)?;
    let (width, height) = match vertical {
        true => (BUTTON, ICON),
        false => (ICON, BUTTON),
    };
    Ok(Box::new(
        StyledContainer::new(
            centred(LayoutStyle::new().width(width).height(height)),
            |_| RectStyle::default(),
            vec![icon],
        )?
        .cursor(Cursor::Grab)
        .on_drag(move |_, _| dragged(&node))
        .on_drag_end(|_, _| GRAB.with(|grab| grab.set(None)))
        .a11y_label(|| telar::t!("editor.quick.grip")),
    ))
}

fn centred(style: LayoutStyle) -> LayoutStyle {
    style
        .flex_row()
        .align_items(AlignItems::CENTER)
        .justify_content(JustifyContent::CENTER)
}

fn face(button: Button, at: usize, node: Node, theme: NordTheme) -> Built {
    let tinting = node.clone();
    let tint = move || match button {
        Button::Remove => theme.warning,
        _ if lit(button, &tinting) => theme.accent,
        _ => theme.text,
    };
    let lighting = node.clone();
    let icon = ui::icon::icon_view(move || icon_of(button).to_string(), tint, ICON)?;
    let radius = ui::scale::corner::xs();
    let face = StyledContainer::new(
        centred(LayoutStyle::new().width(BUTTON).height(BUTTON)),
        move |_| {
            let fill = match lit(button, &lighting) {
                true => theme.accent.with_alpha(0.2),
                false => Color::TRANSPARENT,
            };
            RectStyle::filled(fill, radius)
        },
        vec![icon],
    )?
    .hover_style(move |_| RectStyle::filled(theme.overlay, radius))
    .control(telar::Role::Button)
    .a11y_label(move || label_of(button))
    .on_press(move || press(button));
    let drawn = face.layout_node();
    let face = face
        .on_focused_key(move |key: &Key| on_face_key(button, key))
        .on_focus(move |focused| on_face_focus(at, drawn, focused));
    let lit_node = node.clone();
    let face = match button {
        Button::Panel | Button::Radius | Button::Padding | Button::Link => {
            face.toggled(move || lit(button, &lit_node))
        }
        Button::Customize | Button::Remove => face,
    };
    Ok(Box::new(face))
}

fn lit(button: Button, node: &Node) -> bool {
    match button {
        Button::Panel => crate::panel::offered(node).is_some_and(|offer| offer.owned.is_some()),
        Button::Radius => tools::active() == Some(Tool::Radius),
        Button::Padding => tools::active() == Some(Tool::Padding),
        Button::Link => tools::linked(),
        Button::Customize | Button::Remove => false,
    }
}

fn icon_of(button: Button) -> &'static str {
    match button {
        Button::Customize => "ellipsis",
        Button::Panel => "panel-top",
        Button::Radius => "square-round-corner",
        Button::Padding => "square-square",
        Button::Link if tools::linked() => "link",
        Button::Link => "unlink",
        Button::Remove => "trash-2",
    }
}

pub(crate) fn label_of(button: Button) -> String {
    let (label, row) = match button {
        Button::Customize => (telar::t!("editor.quick.customize"), Some("customize")),
        Button::Panel => {
            let owned = session::selected()
                .node()
                .and_then(crate::panel::offered)
                .is_some_and(|offer| offer.owned.is_some());
            let label = match owned {
                true => telar::t!("editor.panel.edit"),
                false => telar::t!("editor.panel.give"),
            };
            (label, None)
        }
        Button::Radius => (telar::t!("editor.quick.radius"), Some("radius-tool")),
        Button::Padding => (telar::t!("editor.quick.padding"), Some("padding-tool")),
        Button::Link if tools::linked() => (telar::t!("editor.quick.linked"), Some("link-four")),
        Button::Link => (telar::t!("editor.quick.independent"), Some("link-four")),
        Button::Remove => (telar::t!("editor.quick.remove"), Some("remove")),
    };
    match row.and_then(keys::spelled_key) {
        Some(key) => telar::t!("editor.quick.hinted", what = label, key = key),
        None => label,
    }
}

pub(crate) fn press(button: Button) {
    mode::clear_refusal();
    let selection = session::selected();
    match button {
        Button::Customize => {
            release();
            mode::said(crate::popover::open_for(&selection));
        }
        Button::Panel => {
            if let Some(node) = selection.node() {
                mode::said(crate::panel::give(node, crate::panel::Shape::Beside));
            }
        }
        Button::Radius => mode::said(tools::toggle(Tool::Radius)),
        Button::Padding => mode::said(tools::toggle(Tool::Padding)),
        Button::Link => tools::toggle_linked(),
        Button::Remove => mode::said(keys::remove(&selection)),
    }
}

fn offset_in(held: &[(Node, (f32, f32))], node: &Node) -> (f32, f32) {
    held.iter()
        .find(|(of, _)| of == node)
        .map_or((0.0, 0.0), |(_, moved)| *moved)
}

fn offset_of(node: &Node) -> (f32, f32) {
    OFFSETS.with(|offsets| offsets.with(|held| offset_in(held, node)))
}

fn dragged(node: &Node) {
    let Some((x, y)) = surfaces::menu::pointer() else {
        return;
    };
    let (dx, dy) = OFFSETS.with(|offsets| offsets.peek_with(|held| offset_in(held, node)));
    let (grab_x, grab_y) = GRAB.with(|grab| {
        grab.get().unwrap_or_else(|| {
            let taken = (x - dx, y - dy);
            grab.set(Some(taken));
            taken
        })
    });
    let moved = (x - grab_x, y - grab_y);
    OFFSETS.with(|offsets| {
        offsets.update(|held| {
            held.retain(|(of, _)| of != node);
            held.push((node.clone(), moved));
        })
    });
}

fn placed_now(node: &Node, vertical: bool, size: (f32, f32), output: &str) -> (f32, f32) {
    let Some(selection) = rects::rect(node) else {
        return (-2.0 * size.0, -2.0 * size.1);
    };
    let (screen, inset) = reconcile::with_desktop(Some(output), |desktop| {
        (
            Rect::new(0.0, 0.0, desktop.size.0, desktop.size.1),
            tools::target::padding_of(desktop, node).unwrap_or_default(),
        )
    })
    .unwrap_or_default();
    let below = match crate::select::size_tag_shown() {
        true => SIZE_TAG_ROOM,
        false => CLEARANCE,
    };
    let (x, y) = place(Placing {
        selection,
        size,
        screen,
        vertical,
        below,
        inset,
    });
    let (dx, dy) = offset_of(node);
    kept_on(screen, size, (x + dx, y + dy))
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Placing {
    pub(crate) selection: Rect,
    pub(crate) size: (f32, f32),
    pub(crate) screen: Rect,
    pub(crate) vertical: bool,
    pub(crate) below: f32,
    pub(crate) inset: [f32; 4],
}

pub(crate) fn place(placing: Placing) -> (f32, f32) {
    let Placing {
        selection: s,
        size: (width, height),
        screen,
        vertical,
        below,
        inset,
    } = placing;
    let (left, top) = (screen.x + CLEARANCE, screen.y + CLEARANCE);
    let (right, bottom) = (
        screen.x + screen.width - CLEARANCE,
        screen.y + screen.height - CLEARANCE,
    );
    let across = (s.x + s.width / 2.0 - width / 2.0)
        .min(right - width)
        .max(left);
    let down = (s.y + s.height / 2.0 - height / 2.0)
        .min(bottom - height)
        .max(top);
    let at = match vertical {
        true if s.x + s.width + CLEARANCE + width <= right => (s.x + s.width + CLEARANCE, down),
        true if s.x - CLEARANCE - width >= left => (s.x - CLEARANCE - width, down),
        true => (s.x + inset[3] + INSET, down),
        false if s.y - CLEARANCE - height >= top => (across, s.y - CLEARANCE - height),
        false if s.y + s.height + below + height <= bottom => (across, s.y + s.height + below),
        false => (across, s.y + inset[0] + INSET),
    };
    kept_on(screen, (width, height), at)
}

fn kept_on(screen: Rect, (width, height): (f32, f32), (x, y): (f32, f32)) -> (f32, f32) {
    (
        x.min(screen.x + screen.width - CLEARANCE - width)
            .max(screen.x + CLEARANCE),
        y.min(screen.y + screen.height - CLEARANCE - height)
            .max(screen.y + CLEARANCE),
    )
}

#[cfg(test)]
pub(crate) fn drawn_box() -> Option<Rect> {
    let bar = shown_for(&session::selected())?;
    let along = 2.0 * PAD + ICON + bar.buttons.len() as f32 * (BUTTON + SPACING);
    let across = 2.0 * PAD + BUTTON;
    let size = match bar.vertical {
        true => (across, along),
        false => (along, across),
    };
    let output = bar.node.output.clone()?;
    let (x, y) = placed_now(&bar.node, bar.vertical, size, &output);
    Some(Rect::new(x, y, size.0, size.1))
}

#[cfg(test)]
pub(crate) fn centre_of(button: Button) -> Option<(f32, f32)> {
    let bar = shown_for(&session::selected())?;
    let at = bar.buttons.iter().position(|shown| *shown == button)?;
    let drawn = drawn_box()?;
    let along = PAD + ICON + SPACING + at as f32 * (BUTTON + SPACING) + BUTTON / 2.0;
    Some(match bar.vertical {
        true => (drawn.x + drawn.width / 2.0, drawn.y + along),
        false => (drawn.x + along, drawn.y + drawn.height / 2.0),
    })
}

#[cfg(test)]
pub(crate) fn grip_centre() -> Option<(f32, f32)> {
    let drawn = drawn_box()?;
    let vertical = drawn.height > drawn.width;
    Some(match vertical {
        true => (drawn.x + drawn.width / 2.0, drawn.y + PAD + ICON / 2.0),
        false => (drawn.x + PAD + ICON / 2.0, drawn.y + drawn.height / 2.0),
    })
}
