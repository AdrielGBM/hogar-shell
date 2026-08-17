//! Where a hosted surface sits, as the shell's own vocabulary.
//!
//! This lived in Telar until it was clear the framework never read most of it: of the eleven fields, the
//! scaffold consumes five and the other six are carry-through that only this backend looks at. A framework
//! type whose majority of fields the framework never reads is a backend's config struct wearing a framework's
//! name — so it is here, beside `LayerConfig`, which is what it lowers to.
//!
//! It sits in this crate rather than in `ui` because `ui` depends on *this* one: the producer is above the
//! implementor, and the type has to be visible to both.

use std::time::Duration;

/// What kind of secondary surface a placement describes. A backend maps the role to its own surface
/// primitives (a layer-shell backend picks a layer + namespace; a windowed backend a child window or an
/// in-window portal). Roles carry no behaviour of their own — the explicit [`SurfacePlacement`] fields do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceRole {
    /// A panel that slides off a bar/edge, dimming what's behind it.
    Drawer,
    /// A transient, positioned popup (a notification, a menu detached from its trigger).
    Popup,
    /// A brief, non-interactive status flash (volume/brightness), auto-dismissed.
    Osd,
    /// A free-floating window with its own title/close affordances.
    Float,
    /// A modal that owns the screen while it is up: a launcher, a command palette, a session menu. Unlike a
    /// [`Drawer`](Self::Drawer) it isn't anchored to an edge, and unlike a [`Float`](Self::Float) it expects to
    /// take the keyboard outright — the user is typing into it, not at whatever is behind it.
    Overlay,
}

/// How much of the keyboard a surface needs.
///
/// The distinction matters because it decides who receives a keystroke *before* any click. A panel with a text
/// field can wait to be clicked into ([`OnDemand`](Self::OnDemand)); a launcher cannot — it opens on a keybind
/// and the next keystroke is already its first search character, so it has to hold the keyboard from the moment
/// it maps ([`Exclusive`](Self::Exclusive)). Asking for more than is needed is not free: a surface holding the
/// keyboard takes it from the focused window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum KeyboardMode {
    /// Display-only; never takes keyboard focus.
    #[default]
    None,
    /// May be given focus on interaction, e.g. a click into a text field.
    OnDemand,
    /// Holds the keyboard for as long as it is mapped.
    Exclusive,
}

/// The screen edge (or centre) a surface hugs. The cross axis is aligned by [`SurfaceAlign`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceAnchor {
    Top,
    Bottom,
    Left,
    Right,
    Center,
}

/// Cross-axis alignment along the anchored edge (e.g. left/centre/right for a top-anchored surface).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceAlign {
    Start,
    Center,
    End,
}

/// A surface's size: a fixed logical pixel box, or derived from its content.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SurfaceSize {
    Fixed(u32, u32),
    Auto,
}

/// A backend-agnostic description of a secondary surface: where it sits, how big it is, and how it
/// behaves (scrim, outside-dismiss, auto-timeout). The intent lives here; a backend derives its own
/// surface config from it. Reusable by a windowed app (as an in-window portal) and by a shell (as a real
/// layer-shell surface) alike.
#[derive(Debug, Clone)]
pub struct SurfacePlacement {
    pub role: SurfaceRole,
    pub anchor: SurfaceAnchor,
    pub align: SurfaceAlign,
    pub size: SurfaceSize,
    /// Gap from the screen edges, as `(top, right, bottom, left)`. A full-screen scrim scaffold applies the
    /// whole tuple as padding (so the panel floats off every edge, not just the anchored one); a
    /// directly-anchored surface applies it as the compositor margin.
    pub margin: (i32, i32, i32, i32),
    /// Dim (and, with `dismiss_on_outside`, capture) the area behind the panel.
    pub scrim: bool,
    /// A press outside the panel dismisses the surface.
    pub dismiss_on_outside: bool,
    /// Auto-dismiss after this long; `None` keeps it until closed explicitly.
    pub timeout: Option<Duration>,
    /// The surface passes pointer input through to whatever is beneath it (a click-through OSD).
    pub input_transparent: bool,
    /// How much of the keyboard the surface needs; a backend maps this to its own focus model (e.g. layer-shell
    /// keyboard interactivity). Defaults to [`KeyboardMode::None`], so a panel is display-only and never steals
    /// the keyboard.
    pub keyboard: KeyboardMode,
    /// The monitor to place the surface on by name; `None` = the active/default output.
    pub output: Option<String>,
}

impl SurfacePlacement {
    pub fn new(role: SurfaceRole, anchor: SurfaceAnchor) -> Self {
        Self {
            role,
            anchor,
            align: SurfaceAlign::Center,
            size: SurfaceSize::Auto,
            margin: (0, 0, 0, 0),
            scrim: false,
            dismiss_on_outside: false,
            timeout: None,
            input_transparent: false,
            keyboard: KeyboardMode::None,
            output: None,
        }
    }

    /// A modal that owns the screen: centred, scrimmed, dismissed by a press outside, and holding the keyboard
    /// from the moment it maps so the first keystroke after the keybind is already typed into it.
    pub fn overlay() -> Self {
        Self {
            scrim: true,
            dismiss_on_outside: true,
            keyboard: KeyboardMode::Exclusive,
            ..Self::new(SurfaceRole::Overlay, SurfaceAnchor::Center)
        }
    }

    pub fn drawer(anchor: SurfaceAnchor) -> Self {
        Self {
            scrim: true,
            dismiss_on_outside: true,
            ..Self::new(SurfaceRole::Drawer, anchor)
        }
    }

    pub fn osd() -> Self {
        Self {
            input_transparent: true,
            ..Self::new(SurfaceRole::Osd, SurfaceAnchor::Top)
        }
    }

    pub fn float() -> Self {
        Self::new(SurfaceRole::Float, SurfaceAnchor::Center)
    }

    pub fn align(mut self, align: SurfaceAlign) -> Self {
        self.align = align;
        self
    }

    pub fn size(mut self, size: SurfaceSize) -> Self {
        self.size = size;
        self
    }

    pub fn margin(mut self, margin: (i32, i32, i32, i32)) -> Self {
        self.margin = margin;
        self
    }

    pub fn inset(mut self, px: i32) -> Self {
        let (t, r, b, l) = self.margin;
        self.margin = match self.anchor {
            SurfaceAnchor::Top => (px, r, b, l),
            SurfaceAnchor::Bottom => (t, r, px, l),
            SurfaceAnchor::Left => (t, r, b, px),
            SurfaceAnchor::Right => (t, px, b, l),
            SurfaceAnchor::Center => (t, r, b, l),
        };
        self
    }

    pub fn scrim(mut self, scrim: bool) -> Self {
        self.scrim = scrim;
        self
    }

    pub fn dismiss_on_outside(mut self, dismiss: bool) -> Self {
        self.dismiss_on_outside = dismiss;
        self
    }

    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    pub fn input_transparent(mut self, transparent: bool) -> Self {
        self.input_transparent = transparent;
        self
    }

    /// Opt the surface into focus-on-interaction, for panels that host editable text (a search box, a note
    /// title). Sugar for [`keyboard_mode`](Self::keyboard_mode) with
    /// [`OnDemand`](KeyboardMode::OnDemand)/[`None`](KeyboardMode::None).
    pub fn keyboard(mut self, wants_keyboard: bool) -> Self {
        self.keyboard = if wants_keyboard {
            KeyboardMode::OnDemand
        } else {
            KeyboardMode::None
        };
        self
    }

    /// Sets exactly how much of the keyboard the surface takes.
    pub fn keyboard_mode(mut self, mode: KeyboardMode) -> Self {
        self.keyboard = mode;
        self
    }

    /// Whether the surface takes keyboard focus at all.
    pub fn wants_keyboard(&self) -> bool {
        self.keyboard != KeyboardMode::None
    }

    pub fn output(mut self, output: Option<String>) -> Self {
        self.output = output;
        self
    }

    /// Whether the surface needs a full-viewport scaffold (to draw a scrim or catch outside presses)
    /// rather than being anchored directly at its content size.
    pub fn needs_scaffold(&self) -> bool {
        self.scrim || self.dismiss_on_outside
    }
}
