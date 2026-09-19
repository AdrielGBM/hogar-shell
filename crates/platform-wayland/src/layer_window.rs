//! A whole-output window per `(output, layer)`: the surface every node of one layer is drawn into, shown only while it has something to show, and takeable off screen and back without losing its handler, app or tree (see [`Mapping`] and [`LayerWindowHandle::set_mapped`]).

use std::sync::Arc;

use crate::config::{KeyboardInteractivity, Layer};
use crate::link::{SurfaceLink, SurfaceUpdate};

/// Where a surface stands between "on screen" and "off", from the driver's side of the protocol; a configure arriving while `armed` is false was sent for a mapping that no longer exists, and presenting on it would get the connection killed for attaching a buffer to an unconfigured surface.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Mapping {
    wanted: bool,
    armed: bool,
}

/// What a change of [`Mapping`] asks the driver to do to the surface.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Transition {
    None,
    /// Suspend the renderer — joining its thread, so no frame of its own can land after — then attach a null buffer and commit.
    Release,
    /// Ask for the layer-shell state again and commit without a buffer; the surface presents once the configure that answers it arrives.
    Rearm,
}

impl Default for Mapping {
    /// A surface is created wanted and armed: the commit that follows its creation carries no buffer.
    fn default() -> Self {
        Self {
            wanted: true,
            armed: true,
        }
    }
}

impl Mapping {
    pub(crate) fn wanted(self) -> bool {
        self.wanted
    }

    /// Whether a configure arriving now is one the surface may present on.
    pub(crate) fn accepts_configure(self) -> bool {
        self.armed
    }

    /// Takes the owner's request. `presented` is whether the surface may have a buffer attached — a renderer has been running on it, or it holds a reservation strip's buffer — which is what makes hiding it cost a null attach rather than nothing.
    pub(crate) fn set(&mut self, wanted: bool, presented: bool) -> Transition {
        match (self.wanted, wanted) {
            (true, false) => {
                self.wanted = false;
                if presented {
                    self.armed = false;
                    Transition::Release
                } else {
                    Transition::None
                }
            }
            (false, true) => {
                self.wanted = true;
                if self.armed {
                    Transition::None
                } else {
                    self.armed = true;
                    Transition::Rearm
                }
            }
            _ => Transition::None,
        }
    }
}

/// A live layer window. Dropping it — or calling [`close`](Self::close) — asks the driver to tear it down.
pub struct LayerWindowHandle {
    link: Arc<SurfaceLink>,
}

impl LayerWindowHandle {
    pub(crate) fn new(link: Arc<SurfaceLink>) -> Self {
        Self { link }
    }

    /// Takes the window off screen, or puts it back; hiding suspends its renderer and attaches a null buffer so it costs the compositor nothing, while the handler, app and tree survive and are re-presented whole on the next configure.
    pub fn set_mapped(&self, mapped: bool) {
        self.link.request_update(SurfaceUpdate::mapped(mapped));
    }

    /// Renegotiates how the window takes the keyboard — the maximum of what its mounted nodes ask for.
    pub fn set_keyboard(&self, keyboard: KeyboardInteractivity) {
        self.link
            .request_update(SurfaceUpdate::keyboard_interactivity(keyboard));
    }

    /// Moves the window to another layer. Needs layer-shell version 2; an older compositor keeps the layer the window was opened on and says so in the log.
    pub fn set_layer(&self, layer: Layer) {
        self.link.request_update(SurfaceUpdate::layer(layer));
    }

    /// Asks the compositor to blur what is behind `rects`, in logical window coordinates — the union of the nodes styled to blur their backdrop. See [`crate::request_blur_region`].
    pub fn set_blur_region(&self, rects: Vec<telar::Rect>) {
        self.link.request_update(SurfaceUpdate::blur_region(rects));
    }

    /// Builds the window's content again in place, dropping what the outgoing tree registered on the loop. Asked for while the window is hidden, it happens when the window is shown. See [`crate::SurfaceHandle::rebuild`].
    pub fn rebuild(&self) {
        self.link.request_rebuild();
    }

    pub fn close(&self) {
        self.link.request_close();
    }

    pub fn is_closing(&self) -> bool {
        self.link.is_closing()
    }
}

impl Drop for LayerWindowHandle {
    fn drop(&mut self) {
        self.link.request_close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hiding_a_window_that_never_presented_touches_nothing() {
        let mut mapping = Mapping::default();
        assert_eq!(mapping.set(false, false), Transition::None);
        assert!(!mapping.wanted());
        assert!(
            mapping.accepts_configure(),
            "no buffer was ever attached, so the configure its creation asked for is still good"
        );
        assert_eq!(
            mapping.set(true, false),
            Transition::None,
            "showing it is only a matter of starting its renderer"
        );
    }

    #[test]
    fn a_hide_and_show_cycle_releases_then_rearms() {
        let mut mapping = Mapping::default();
        assert_eq!(mapping.set(false, true), Transition::Release);
        assert!(
            !mapping.accepts_configure(),
            "a configure sent for the mapping that just ended must not license a buffer"
        );
        assert_eq!(mapping.set(true, false), Transition::Rearm);
        assert!(mapping.wanted());
        assert!(mapping.accepts_configure());
    }

    #[test]
    fn asking_for_the_state_already_asked_for_does_nothing() {
        let mut mapping = Mapping::default();
        assert_eq!(mapping.set(true, true), Transition::None);
        assert_eq!(mapping.set(false, true), Transition::Release);
        assert_eq!(
            mapping.set(false, false),
            Transition::None,
            "a second hide has nothing left to release"
        );
    }

    /// Hidden again after re-arming but before the configure arrived: nothing was attached, so nothing is released, and the configure that answers the re-arm still licenses the next show.
    #[test]
    fn a_window_hidden_before_its_rearm_was_answered_keeps_the_rearm() {
        let mut mapping = Mapping::default();
        mapping.set(false, true);
        assert_eq!(mapping.set(true, false), Transition::Rearm);
        assert_eq!(mapping.set(false, false), Transition::None);
        assert!(mapping.accepts_configure());
        assert_eq!(
            mapping.set(true, false),
            Transition::None,
            "armed already, so no second buffer-less commit"
        );
    }
}
