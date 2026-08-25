//! Which module id a panel surface fills itself from.
//!
//! A drawer, a float and the settings preview all show *a module's panel*, and none of them knows which module that is until it opens. The mapping used to be a `match` beside the drawer, which meant the surface that merely presents a panel had to name every module that has one — the one edge that stopped the surfaces being buildable without them. Registered instead, by whoever owns the module list.
//!
//! The keyboard mode is registered with the builder rather than beside it, because the two cannot be allowed to drift: a panel that gains a text field and is not granted the keyboard is a field that cannot be typed into. Asking for it costs more than an unused capability — a layer surface granted keyboard focus takes it from the focused window, and the compositor re-focuses that window when the panel closes; a layout that follows focus moves the viewport on the way back — so a panel that only displays readings must not ask.

use std::cell::RefCell;
use std::collections::HashMap;

use platform_wayland::KeyboardMode;
use telar::{LayoutError, LayoutItem};

pub type PanelBuilder = fn() -> Result<Box<dyn LayoutItem>, LayoutError>;

pub struct PanelDef {
    pub build: PanelBuilder,
    pub keyboard: KeyboardMode,
    /// What the panel keeps only while it is open, dropped when the user closes it. A reload never comes through here — it leaves open panels alone — so this fires exactly on "the user is done with it".
    pub on_close: Option<fn()>,
}

/// Every module that has a panel, and what opening it needs.
pub struct PanelRegistry {
    panels: HashMap<String, PanelDef>,
    /// What an unregistered module shows. A panel surface is opened before anything can check the id, so there has to be something to draw.
    fallback: PanelBuilder,
}

impl PanelRegistry {
    pub fn new(fallback: PanelBuilder) -> Self {
        Self {
            panels: HashMap::new(),
            fallback,
        }
    }

    pub fn register(&mut self, id: &str, build: PanelBuilder, keyboard: KeyboardMode) {
        self.panels.insert(
            id.to_string(),
            PanelDef {
                build,
                keyboard,
                on_close: None,
            },
        );
    }

    /// Registers what `id`'s panel forgets when the user closes it. Separate from [`Self::register`] because only a panel that carries state across its own rebuilds has anything to drop.
    pub fn on_close(&mut self, id: &str, forget: fn()) {
        if let Some(def) = self.panels.get_mut(id) {
            def.on_close = Some(forget);
        }
    }

    pub fn def(&self, id: &str) -> Option<&PanelDef> {
        self.panels.get(id)
    }

    /// Every registered id, sorted. What lets a test check this list against the modules wired to open a panel.
    pub fn ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.panels.keys().cloned().collect();
        ids.sort();
        ids
    }
}

thread_local! {
    static LIVE: RefCell<Option<PanelRegistry>> = const { RefCell::new(None) };
}

/// Publishes the registry every panel surface resolves against. Set once at startup.
pub fn install(registry: PanelRegistry) {
    LIVE.with(|live| *live.borrow_mut() = Some(registry));
}

/// The raw panel content for a module, shared by the drawer and floating-window presentations.
pub fn build(module: &str) -> Result<Box<dyn LayoutItem>, LayoutError> {
    LIVE.with(|live| {
        let live = live.borrow();
        let Some(registry) = live.as_ref() else {
            return Err(LayoutError::Engine(
                "no panel registry is installed".to_string(),
            ));
        };
        match registry.def(module) {
            Some(def) => (def.build)(),
            None => {
                tracing::warn!("no panel registered for module '{module}'");
                (registry.fallback)()
            }
        }
    })
}

/// Tells `module`'s panel the user closed it, so whatever it kept for the length of that visit is dropped.
pub fn closed(module: &str) {
    let forget = LIVE.with(|live| {
        live.borrow()
            .as_ref()
            .and_then(|registry| registry.def(module))
            .and_then(|def| def.on_close)
    });
    if let Some(forget) = forget {
        forget();
    }
}

/// Whether `module`'s panel needs the keyboard — because it hosts editable text, or because it is navigable with the arrow keys.
pub fn wants_keyboard(module: &str) -> KeyboardMode {
    LIVE.with(|live| {
        live.borrow()
            .as_ref()
            .and_then(|registry| registry.def(module))
            .map(|def| def.keyboard)
            .unwrap_or(KeyboardMode::None)
    })
}
