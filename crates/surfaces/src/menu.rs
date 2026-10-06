//! The context menu a secondary press asks for, on an item or on an area's empty space: what it is about, which window it was asked in and where — or, on a screen's empty background, the shell's own menu.
//!
//! What a menu holds and what its rows do is the editor's, which this crate cannot depend on, so the editor installs the opener at startup and everything here only asks for one. Nothing is ever asked for on the lock layer (TA-8).

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::thread::LocalKey;

use telar::{
    Component, Event, EventResult, Key, LayoutItem, ModifiersState, NamedKey, NodeId, RenderNode,
};

use layout::LayerKind;
use ui::host::Audience;

use crate::layer_window::LayerWindowContext;
use crate::rects::Node;

/// One request for a menu.
#[derive(Clone, Debug, PartialEq)]
pub struct Asked {
    /// An instance, or an area for its empty space.
    pub node: Node,
    /// The window it was asked in, which is the one it opens in (DEC-9).
    pub window: LayerKind,
    /// Where, in that window's coordinates; `None` from the keyboard, which opens it on the node itself.
    pub at: Option<(f32, f32)>,
}

/// A request for the shell's own menu, asked on a screen's empty background rather than about anything drawn there.
#[derive(Clone, Debug, PartialEq)]
pub struct ShellAsked {
    pub output: Option<String>,
    /// In the coordinates of the window it was asked in, which are its output's.
    pub at: Option<(f32, f32)>,
}

type Installed<A> = RefCell<Option<Rc<dyn Fn(A)>>>;

thread_local! {
    static OPENER: Installed<Asked> = const { RefCell::new(None) };
    static SHELL_OPENER: Installed<ShellAsked> = const { RefCell::new(None) };
    static POINTER: Cell<Option<(f32, f32)>> = const { Cell::new(None) };
}

/// Installs what opens a menu. Set once at startup, before any window builds; until it is, nothing offers one.
pub fn install(open: impl Fn(Asked) + 'static) {
    OPENER.with(|opener| *opener.borrow_mut() = Some(Rc::new(open)));
}

/// Opens the menu `asked` names, where an opener is installed.
pub fn open(asked: Asked) {
    let opener = OPENER.with(|opener| opener.borrow().clone());
    if let Some(open) = opener {
        open(asked);
    }
}

/// What a secondary press on `node` does: opens its menu where the pointer is, in the window being built. `None` on the lock layer, and while no opener is installed.
pub fn on(node: Node, audience: Audience) -> Option<Rc<dyn Fn()>> {
    if node.layer == LayerKind::Lock {
        return None;
    }
    let window = LayerWindowContext::current().map_or(node.layer, |window| window.layer);
    offer(&OPENER, audience, move |at| Asked {
        node: node.clone(),
        window,
        at,
    })
}

/// Installs what opens the shell's own menu, as [`install`] does for the menus of what is drawn.
pub fn install_shell(open: impl Fn(ShellAsked) + 'static) {
    SHELL_OPENER.with(|opener| *opener.borrow_mut() = Some(Rc::new(open)));
}

/// What a secondary press on the empty background of `output` does: opens the shell's menu where the pointer is. `None` for anyone but the owner, and while no opener is installed.
pub fn shell_on(output: Option<&str>, audience: Audience) -> Option<Rc<dyn Fn()>> {
    let output = output.map(str::to_string);
    offer(&SHELL_OPENER, audience, move |at| ShellAsked {
        output: output.clone(),
        at,
    })
}

/// A secondary press handing what `ask` makes of where the pointer is to the opener `installed` holds when it is pressed. `None` for anyone but the owner, and while nothing is installed there.
fn offer<A: 'static>(
    installed: &'static LocalKey<Installed<A>>,
    audience: Audience,
    ask: impl Fn(Option<(f32, f32)>) -> A + 'static,
) -> Option<Rc<dyn Fn()>> {
    let offered = audience == Audience::Owner && installed.with(|opener| opener.borrow().is_some());
    if !offered {
        return None;
    }
    Some(Rc::new(move || {
        let opener = installed.with(|opener| opener.borrow().clone());
        if let Some(open) = opener {
            open(ask(POINTER.with(Cell::get)));
        }
    }))
}

/// Where the pointer last was, in the coordinates of the window it was over — which, every window of an output being the whole output, are that output's. What a drag across a window reads the pointer from, since a handler is told where the pointer is only relative to its own box, and that box may move under it.
pub fn pointer() -> Option<(f32, f32)> {
    POINTER.with(Cell::get)
}

/// Whether `key` asks for the focused thing's menu: the menu key, or Shift+F10 on a keyboard without one.
pub fn is_menu_key(key: &Key, modifiers: ModifiersState) -> bool {
    match key {
        Key::Named(NamedKey::ContextMenu) => true,
        Key::Named(NamedKey::F10) => {
            modifiers.is_shift && !modifiers.is_ctrl && !modifiers.is_alt && !modifiers.is_meta
        }
        _ => false,
    }
}

/// A window's root, noting where each pointer event lands before the tree answers it: a secondary press is answered on the release, and a press handler is told which button rather than where, so this is how the menu it opens knows the point to open at.
pub struct Pointed(Box<dyn LayoutItem>);

impl Pointed {
    pub fn new(root: Box<dyn LayoutItem>) -> Self {
        Self(root)
    }
}

impl Component for Pointed {
    fn view(&self) -> RenderNode {
        self.0.view()
    }

    fn on_event(&mut self, event: &Event) -> EventResult {
        let at = match event {
            Event::PointerMoved { x, y, .. }
            | Event::PointerPressed { x, y, .. }
            | Event::PointerReleased { x, y, .. } => Some((*x as f32, *y as f32)),
            _ => None,
        };
        if let Some(at) = at {
            POINTER.with(|pointer| pointer.set(Some(at)));
        }
        self.0.on_event(event)
    }

    fn debug_name(&self) -> &'static str {
        "Pointed"
    }
}

impl LayoutItem for Pointed {
    fn layout_node(&self) -> NodeId {
        self.0.layout_node()
    }

    fn occludes(&self) -> bool {
        self.0.occludes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_menu_key_and_shift_f10_ask_for_a_menu_and_nothing_else_does() {
        let plain = ModifiersState::default();
        let shift = ModifiersState {
            is_shift: true,
            ..plain
        };
        let ctrl_shift = ModifiersState {
            is_ctrl: true,
            ..shift
        };
        assert!(is_menu_key(&Key::Named(NamedKey::ContextMenu), plain));
        assert!(is_menu_key(&Key::Named(NamedKey::F10), shift));
        assert!(!is_menu_key(&Key::Named(NamedKey::F10), plain));
        assert!(!is_menu_key(&Key::Named(NamedKey::F10), ctrl_shift));
        assert!(!is_menu_key(&Key::Named(NamedKey::Enter), shift));
    }
}
