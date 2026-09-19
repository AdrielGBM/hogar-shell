pub use smithay_client_toolkit::shell::wlr_layer::{Anchor, KeyboardInteractivity, Layer};

use crate::link::SurfaceUpdate;

#[derive(Clone)]
pub struct LayerConfig {
    pub output: Option<String>,
    pub layer: Layer,
    pub anchor: Anchor,
    pub exclusive_zone: i32,
    pub size: (u32, u32),
    pub margin: (i32, i32, i32, i32),
    pub keyboard_interactivity: KeyboardInteractivity,
    pub namespace: String,
    /// Layer surface reserves exclusive_zone only once mapped (needs a buffer).
    pub reserve_only: bool,
    /// Empty input region routes pointer/touch through to surfaces beneath.
    pub input_transparent: bool,
    /// Carves the input region from the surface's interactive widgets each frame (via `telar::interactive_rects`): pointer input lands on pressable content, everything else falls through. For click-through overlays with tappable parts, such as notification popups. Takes precedence over `input_transparent`.
    pub interactive_input_region: bool,
}

impl LayerConfig {
    /// What a live surface has to renegotiate to go from this configuration to `next` — only the fields that actually differ, so a surface asking the compositor for the state it is already in never commits.
    pub fn delta(&self, next: &LayerConfig) -> SurfaceUpdate {
        SurfaceUpdate {
            size: (self.size != next.size).then_some(next.size),
            margin: (self.margin != next.margin).then_some(next.margin),
            exclusive_zone: (self.exclusive_zone != next.exclusive_zone)
                .then_some(next.exclusive_zone),
            anchor: (self.anchor != next.anchor).then_some(next.anchor),
            layer: (self.layer != next.layer).then_some(next.layer),
            keyboard_interactivity: (self.keyboard_interactivity != next.keyboard_interactivity)
                .then_some(next.keyboard_interactivity),
            blur_region: None,
            mapped: None,
        }
    }

    /// Takes on every layer-shell field `change` names, so a surface keeps knowing the state it asked the compositor for — the state it has to ask for again when it re-arms after an unmap, since the protocol returns an unmapped layer surface to "the state it had right after `get_layer_surface`".
    pub(crate) fn absorb(&mut self, change: &SurfaceUpdate) {
        if let Some(size) = change.size {
            self.size = size;
        }
        if let Some(margin) = change.margin {
            self.margin = margin;
        }
        if let Some(zone) = change.exclusive_zone {
            self.exclusive_zone = zone;
        }
        if let Some(anchor) = change.anchor {
            self.anchor = anchor;
        }
        if let Some(layer) = change.layer {
            self.layer = layer;
        }
        if let Some(keyboard) = change.keyboard_interactivity {
            self.keyboard_interactivity = keyboard;
        }
    }

    /// Everything a live surface can renegotiate, asked for as it stands: what re-arming an unmapped surface sends before its buffer-less commit.
    pub(crate) fn as_update(&self) -> SurfaceUpdate {
        SurfaceUpdate {
            size: Some(self.size),
            margin: Some(self.margin),
            exclusive_zone: Some(self.exclusive_zone),
            anchor: Some(self.anchor),
            layer: Some(self.layer),
            keyboard_interactivity: Some(self.keyboard_interactivity),
            blur_region: None,
            mapped: None,
        }
    }

    /// A layer window: anchored to all four edges at the compositor's size, and opted out of every exclusive zone, so every window on an output shares one coordinate space. How its input region is decided is not read from here — a layer window always carves it from its drawn content.
    pub(crate) fn whole_output(output: Option<String>, layer: Layer, namespace: String) -> Self {
        Self {
            output,
            layer,
            anchor: Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT,
            exclusive_zone: -1,
            size: (0, 0),
            margin: (0, 0, 0, 0),
            keyboard_interactivity: KeyboardInteractivity::None,
            namespace,
            reserve_only: false,
            input_transparent: false,
            interactive_input_region: false,
        }
    }
}

impl Default for LayerConfig {
    fn default() -> Self {
        Self {
            output: None,
            layer: Layer::Top,
            anchor: Anchor::TOP.union(Anchor::LEFT).union(Anchor::RIGHT),
            exclusive_zone: 0,
            size: (0, 40),
            margin: (0, 0, 0, 0),
            keyboard_interactivity: KeyboardInteractivity::None,
            namespace: String::from("hogar-shell"),
            reserve_only: false,
            input_transparent: false,
            interactive_input_region: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct OutputDescriptor {
    pub name: Option<String>,
    pub logical_size: Option<(i32, i32)>,
    pub position: (i32, i32),
    pub scale: i32,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bar() -> LayerConfig {
        LayerConfig {
            size: (0, 40),
            exclusive_zone: 40,
            ..LayerConfig::default()
        }
    }

    /// What a live surface is told, folded back into what it knows it asked for, is exactly the configuration it was reconciled to — the state a re-armed surface must ask for again.
    #[test]
    fn a_delta_absorbed_is_the_configuration_it_was_taken_towards() {
        let before = bar();
        let after = LayerConfig {
            layer: Layer::Overlay,
            keyboard_interactivity: KeyboardInteractivity::OnDemand,
            margin: (-36, 0, 0, 0),
            exclusive_zone: 0,
            ..bar()
        };
        let mut known = before.clone();
        known.absorb(&before.delta(&after));
        assert_eq!(known.as_update(), after.as_update());
    }

    /// Mapping is not configuration: reconciling a surface against a changed config must never show or hide it, which is its owner's decision.
    #[test]
    fn a_delta_never_maps_or_unmaps() {
        let hidden_layer = LayerConfig {
            layer: Layer::Background,
            ..bar()
        };
        let change = bar().delta(&hidden_layer);
        assert_eq!(change.layer, Some(Layer::Background));
        assert_eq!(change.mapped, None);
        assert_eq!(change.blur_region, None);
    }

    /// Re-arming asks for every renegotiable field, because an unmapped layer surface may have forgotten any of them.
    #[test]
    fn re_arming_asks_for_every_layer_shell_field() {
        let update = bar().as_update();
        assert!(update.renegotiates());
        assert_eq!(update.size, Some((0, 40)));
        assert_eq!(update.exclusive_zone, Some(40));
        assert_eq!(update.layer, Some(Layer::Top));
        assert_eq!(
            update.keyboard_interactivity,
            Some(KeyboardInteractivity::None)
        );
        assert!(update.anchor.is_some() && update.margin.is_some());
        assert_eq!(
            update.mapped, None,
            "re-arming is what mapping does, not something it asks for"
        );
    }

    /// Every window on an output shares one coordinate space only if each covers the whole output and ignores every reservation, its own layer's included.
    #[test]
    fn a_layer_window_covers_its_output_and_reserves_nothing() {
        let window = LayerConfig::whole_output(
            Some("DP-1".into()),
            Layer::Bottom,
            "hogar-shell-desktop".into(),
        );
        assert_eq!(
            window.anchor,
            Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT
        );
        assert_eq!(window.size, (0, 0), "both axes are the compositor's");
        assert_eq!(window.exclusive_zone, -1);
        assert_eq!(window.margin, (0, 0, 0, 0));
        assert!(
            !window.input_transparent && !window.interactive_input_region,
            "a layer window's input region is never a creation-time choice"
        );
        assert!(!window.reserve_only);
    }
}
