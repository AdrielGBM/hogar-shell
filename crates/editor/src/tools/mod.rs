//! The radius and padding tools: handles laid over the selection in every mode that round its corners or hold what it holds off its edges, in place of the popover's rows for the same values.
//!
//! **Linked or independent.** The four corners or sides move together while they are linked, and each on its own while they are not; Alt held on a drag or an arrow isolates the one it is on while they are linked. Which of the two it is stays as it was left for the rest of the session, whichever tool, selection or mode comes next. Linked, a gesture writes one number; independent, or with Alt, it writes all four.
//!
//! **Where they write.** A bar's corners are its own `shape.radius`; every other area's, a group's and an instance's are its `style.radius`. Padding is the `style.padding` of an area that holds groups — a grid, a panel, a free area, a dock, a bar — or of a container, and never of an instance, a prompt, a stack, a wallpaper region or a texture ([`target::has_padding`]).
//!
//! **One undo entry each.** A drag previews from its first move and is recorded as it is let go, an arrow held on a focused handle is previewed as the keyboard repeats it and recorded as it is let go, as every held key of the editor is, and Esc before either is let go, or the other button mid-drag, puts all four back. The tool is put away when the selection changes or a popover opens over it.
//!
//! **Every handle has a key (WCAG 2.5.7).** `r` and `i` take up the radius and the padding tool for the selection and put them away again, `u` links or unlinks the four, and the arrows move the focused handle's corner or side — all four while they are linked, unless Alt is held — as a drag does.

mod marks;
mod padding;
mod radius;
pub(crate) mod target;

use std::cell::RefCell;
use std::rc::Rc;

use telar::{Color, ReactiveList, RwSignal, detached, effect, signal};

use layout::{LayerKind, Layout, LayoutOp};
use surfaces::reconcile::{self, Desktop};
use surfaces::rects::{self, Node};
use ui::descriptor::Built;

use crate::host::{self, see_through, whole};
use crate::keys::{self, Chord, KeyOp, Run};
use crate::mode::{self, Mode};
use crate::modes::gesture;
use crate::popover::handles::{Ends, Four, most_padding_in, most_radius_in};
use crate::session::{self, Edit, EditError, Selection};

/// What a tool's handles change on the selection.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Tool {
    Radius,
    Padding,
}

thread_local! {
    static ACTIVE: RwSignal<Option<Tool>> = detached(|| signal(None));
    static LINKED: RwSignal<bool> = detached(|| signal(true));
}

pub fn active() -> Option<Tool> {
    ACTIVE.with(|active| active.get())
}

/// Whether the four corners or sides move together, read as a signal.
pub fn linked() -> bool {
    LINKED.with(|linked| linked.get())
}

pub fn linked_now() -> bool {
    LINKED.with(|linked| linked.peek())
}

pub fn offers(selection: &Selection, tool: Tool) -> bool {
    target::offers(selection, tool)
}

/// Takes `tool` up for the selection, or puts it away when it is up already; refused where the selection has nothing it changes.
pub fn toggle(tool: Tool) -> Result<(), EditError> {
    toggle_for(&session::selected(), tool)
}

/// Links the four corners or sides when they move on their own, and unlinks them when they move together.
pub fn toggle_linked() {
    let now = !linked_now();
    LINKED.with(|linked| linked.set(now));
    mode::confirm(match now {
        true => telar::t!("editor.tool.linked"),
        false => telar::t!("editor.tool.independent"),
    });
}

pub fn put_away() {
    ACTIVE.with(|active| {
        if active.peek().is_some() {
            active.set(None);
        }
    });
}

fn toggle_for(selection: &Selection, tool: Tool) -> Result<(), EditError> {
    if ACTIVE.with(|active| active.peek()) == Some(tool) {
        put_away();
        return Ok(());
    }
    if !offers(selection, tool) {
        return Err(match (tool, selection) {
            (_, Selection::None) | (Tool::Radius, _) => EditError::nothing(),
            (Tool::Padding, _) => EditError::refused(telar::t!("editor.tool.no_padding")),
        });
    }
    ACTIVE.with(|active| active.set(Some(tool)));
    Ok(())
}

/// Mounts the tools over every mode, above every other tool so their handles take the pointer before what is under them, adds their keys, and puts the tool away whenever the selection changes. Installed once, from [`crate::install`].
pub(crate) fn install() {
    for layer in LayerKind::ALL {
        host::add_tool(layer, overlay);
        keys::add_mode_key_op(
            layer,
            KeyOp {
                name: "radius-tool",
                keys: vec![Chord::char('r')],
                label: || telar::t!("editor.keys.op.radius-tool"),
                run: Run::Act(|selection| toggle_for(selection, Tool::Radius)),
            },
        );
        keys::add_mode_key_op(
            layer,
            KeyOp {
                name: "padding-tool",
                keys: vec![Chord::char('i')],
                label: || telar::t!("editor.keys.op.padding-tool"),
                run: Run::Act(|selection| toggle_for(selection, Tool::Padding)),
            },
        );
        keys::add_mode_key_op(
            layer,
            KeyOp {
                name: "link-four",
                keys: vec![Chord::char('u')],
                label: || telar::t!("editor.keys.op.link-four"),
                run: Run::Act(|_| {
                    toggle_linked();
                    Ok(())
                }),
            },
        );
    }
    detached(|| {
        let last: RefCell<Option<Node>> = RefCell::default();
        effect(move || {
            let now = session::selection().get().node().cloned();
            if *last.borrow() != now {
                *last.borrow_mut() = now;
                put_away();
            }
        });
    });
}

/// What the tools draw over the selection: which tool, over what, whether the four are linked, and the most the box takes.
#[derive(Clone, Debug, PartialEq, Hash)]
struct Up {
    tool: Tool,
    node: Node,
    linked: bool,
    most: u32,
}

fn up() -> Option<Up> {
    let tool = active()?;
    let linked = linked();
    let node = session::selection().get().node()?.clone();
    if !mode::editing(&node) || crate::popover::current().is_some() {
        return None;
    }
    let rect = rects::rect(&node)?;
    let most = match tool {
        Tool::Radius => most_radius_in(rect),
        Tool::Padding => most_padding_in(rect),
    };
    Some(Up {
        tool,
        node,
        linked,
        most: most as u32,
    })
}

fn overlay(_: &Mode) -> Built {
    let last: Rc<RefCell<Option<Up>>> = Rc::default();
    let shown = ReactiveList::with_style(
        whole(),
        move || {
            if gesture::dragging() {
                return last.borrow().clone().into_iter().collect();
            }
            let now = up();
            last.borrow_mut().clone_from(&now);
            now.into_iter().collect()
        },
        |up: &Up| up.clone(),
        |up: Up| match up.tool {
            Tool::Radius => radius::handles(up.node, up.most as f32, up.linked),
            Tool::Padding => padding::handles(up.node, up.most as f32, up.linked),
        },
    )?;
    see_through(shown)
}

type Read = fn(&Desktop, &Node) -> Option<[f32; 4]>;
type Plan = fn(&Node, &Layout, [f32; 4]) -> Result<Vec<LayoutOp>, EditError>;

/// What `read` finds of `node` as the screen draws it at this moment, without following it.
fn read_now(read: Read, node: &Node) -> Option<[f32; 4]> {
    reconcile::desktop_now(node.output.as_deref()).and_then(|desktop| read(&desktop, node))
}

/// The four values of `node` a tool's handles drag, kept to what the screen draws between gestures, and what the handles tell the edit each gesture previews into: called `label` in the history, planned by `plan` against the layout as it was when a drag began, or as the held arrow's edit has left it. A gesture that leaves the four as they were drawn writes nothing.
fn live(
    node: &Node,
    most: f32,
    read: Read,
    plan: Plan,
    label: String,
    color: Color,
) -> (Four, Ends) {
    let values = read_now(read, node).unwrap_or_default().map(signal);
    let edit = Edit::new(label.clone());
    {
        let (node, edit) = (node.clone(), edit.clone());
        effect(move || {
            let Some(drawn) = reconcile::desktop(node.output.as_deref())
                .and_then(|desktop| read(&desktop, &node))
            else {
                return;
            };
            if !edit.is_open() {
                show(values, drawn);
            }
        });
    }
    let previewing = (node.clone(), edit.clone());
    let moved = Rc::new(move |now: [f32; 4]| {
        let (node, edit) = &previewing;
        if !edit.is_open() && edit.begin().is_err() {
            return;
        }
        let Some(before) = edit.transaction().before() else {
            return;
        };
        if let Err(why) = plan(node, &before, now).and_then(|ops| edit.preview(ops)) {
            tracing::info!("{why}");
        }
    });
    let committing = (node.clone(), edit.clone());
    let kept = Rc::new(move || {
        let (node, edit) = &committing;
        let now = values.map(|value| value.peek());
        if !edit.is_open() && (read_now(read, node) == Some(now) || edit.begin().is_err()) {
            return;
        }
        let Some(before) = edit.transaction().before() else {
            return;
        };
        match plan(node, &before, now).and_then(|ops| edit.preview(ops)) {
            Ok(()) => mode::said(edit.commit()),
            Err(why) => {
                mode::refuse(why);
                let _ = edit.revert();
            }
        }
    });
    let stepping = node.clone();
    let stepped = Rc::new(move |now: [f32; 4]| {
        if read_now(read, &stepping) == Some(now) {
            return;
        }
        let node = stepping.clone();
        if let Err(why) =
            keys::handle_step(label.clone(), move |_, layout| plan(&node, layout, now))
        {
            mode::refuse(why);
            if let Some(drawn) = read_now(read, &stepping) {
                show(values, drawn);
            }
        }
    });
    let dropped = Rc::new(move || {
        if edit.is_open() {
            let _ = edit.revert();
        }
    });
    (
        Four { values, most },
        Ends {
            moved,
            kept,
            stepped,
            dropped,
            color,
        },
    )
}

fn show(values: [RwSignal<f32>; 4], drawn: [f32; 4]) {
    for (value, now) in values.iter().zip(drawn) {
        if value.peek() != now {
            value.set(now);
        }
    }
}
