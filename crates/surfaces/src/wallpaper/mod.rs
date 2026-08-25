//! The background surface: the wallpaper and its transition.
//!
//! One surface per monitor, at the bottom of the background layer, painting the image the wallpaper service says this screen should show — cover-cropped over the theme's base colour. Nothing else: what is drawn *over* the desktop is [`crate::widgets`], on a surface of its own, so a widget that repaints with the music does not repaint a screen-sized photograph with it.
//!
//! **The transition is why a wallpaper change is an event and not a rebuild.** A picture chosen at runtime is not a config edit — it is session state — and rebuilding the surface for it would be useless for a cross-fade anyway: a fresh tree has nothing left of the old image to fade *from*. So a runtime wallpaper change arrives as an event on the live surface, carrying an image the service already decoded off the UI thread, and the two layers are simply ping-ponged.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::SystemTime;

use telar::{
    App, Color, Component, Container, Image, ImageData, ImageFilter, LayoutError, LayoutItem,
    LayoutStyle, ObjectFit, RectStyle, SizeDimension, StyledContainer, WindowConfig, box_item,
    motion::Animated, reset_layout_runtime, set_theme, signal,
};

use config::{Config, WallpaperTransition};
use services::wallpaper;
use telar::WindowRoot;

/// How far a wipe travels when the compositor has not said how wide this screen is. Only reached before the output list has been read, and a wipe that starts slightly off-screen is invisible either way.
const FALLBACK_TRAVEL: f32 = 1920.0;

/// Reading where the hand-over between the two image layers has got to, and moving it. `Rc` on the reading half because both layers hold one; `Box` on the writing half because only the frame consumer does.
type FadeControl = (Rc<dyn Fn() -> f32>, Box<dyn Fn(f32)>);

/// Per-output wallpaper: a full-screen background surface painting the current image (cover-cropped, aspect preserved) over the theme's base colour, or just the base colour when no image resolves.
pub struct WallpaperApp {
    /// Read at every build rather than held: the surface outlives the config it was first drawn from, and a reload rebuilds it in place from whatever is in here now.
    pub config: config::LiveConfig,
    /// The monitor this wallpaper covers, so a `[background.monitors]` entry can target it.
    pub output: Option<String>,
}

impl App for WallpaperApp {
    fn root(&self) -> Box<dyn Component> {
        reset_layout_runtime();
        let config = self.config.get();
        set_theme(config.resolve_theme());
        services::locale::attach(config.language());
        Box::new(WindowRoot::wrapping(self.content(&config)).expect("wallpaper layout failed"))
    }

    fn clear_color(&self) -> Option<Color> {
        // The base colour shows before/without an image (and behind any transparency), covering "theme base colour when there is no image".
        Some(self.config.get().resolve_theme().base)
    }

    fn window_config(&self) -> Option<WindowConfig> {
        // Opaque: a wallpaper replaces whatever the compositor draws behind it.
        None
    }
}

/// The desktop as this surface draws it — the configured image, cover-cropped and decoded for real — for [`crate::preview`].
pub(crate) fn preview() -> Result<Box<dyn LayoutItem>, LayoutError> {
    let mut config = config::config()
        .map(|live| (*live).clone())
        .unwrap_or_else(Config::starter);
    config.background.enabled = true;
    // The settled desktop, not the crossfade into it: a preview captures a handful of frames, and a 600ms transition is still halfway through when the last of them is taken.
    config.background.transition = WallpaperTransition::None;
    let app = WallpaperApp {
        config: Arc::new(config.clone()).into(),
        output: None,
    };
    // No box of its own: the entry declares a `PreviewSurface`, which is what gives the image layers — absolutely positioned to fill their surface — something to fill.
    Ok(app.content(&config))
}

impl WallpaperApp {
    fn content(&self, config: &Config) -> Box<dyn LayoutItem> {
        let mut layers: Vec<Box<dyn LayoutItem>> = Vec::new();
        match self.image_layers(config) {
            Ok(Some(images)) => layers.push(images),
            Ok(None) => {}
            Err(e) => tracing::warn!("wallpaper images: {e}"),
        }
        Container::new(fill(), layers)
            .map(|container| Box::new(container) as Box<dyn LayoutItem>)
            .expect("wallpaper root container")
    }

    /// The two stacked image slots and the animation that hands the screen from one to the other.
    ///
    /// A ping-pong rather than an "outgoing / incoming" pair: `fade` runs `0` → slot A visible, `1` → slot B visible, and each new image lands in whichever slot is currently hidden. One animation, no bookkeeping about which layer is on the way out, and an interrupted transition — a second change arriving mid-fade — simply retargets from wherever it had got to instead of snapping.
    fn image_layers(&self, config: &Config) -> Result<Option<Box<dyn LayoutItem>>, LayoutError> {
        let initial = wallpaper::current_image(config, self.output.as_deref());
        let first = initial
            .as_deref()
            .and_then(|path| decoded(self.output.as_deref(), path));
        if first.is_none() && initial.is_some() {
            tracing::warn!(
                "wallpaper '{}' could not be loaded; using the theme base colour",
                initial
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default()
            );
        }

        let slot_a = signal(first);
        let slot_b = signal(None::<Arc<ImageData>>);
        let transition = config.background.transition;
        let (read_fade, set_fade) = Self::fade_control(config);
        let travel = self.output_width();

        let layer_a = image_layer(
            slot_a.read_only(),
            read_fade.clone(),
            0.0,
            transition,
            travel,
        )?;
        let layer_b = image_layer(slot_b.read_only(), read_fade, 1.0, transition, travel)?;

        // Which slot holds the newest image. A plain `Cell`: it only ever changes on the driver thread, from the consumer below, so a signal would buy reactivity that nothing reads.
        let showing_b = Rc::new(Cell::new(false));
        platform_wayland::watch(
            wallpaper::frames(self.output.clone(), initial),
            move |frame: Option<wallpaper::Frame>| {
                // `None` is the producer's liveness heartbeat, not a wallpaper.
                let Some(frame) = frame else { return };
                let next_is_b = !showing_b.get();
                if next_is_b {
                    slot_b.set(Some(frame.image));
                } else {
                    slot_a.set(Some(frame.image));
                }
                showing_b.set(next_is_b);
                set_fade(if next_is_b { 1.0 } else { 0.0 });
            },
        );

        let stack = Container::new(fill(), vec![layer_a, layer_b])?;
        Ok(Some(Box::new(stack)))
    }

    /// How the layers are handed over: reading the current position, and moving it.
    ///
    /// Two shapes behind one pair of closures. An animated transition drives an `Animated`, built at `0.0` and retargeted — never at its destination, which would leave it inert. `transition = "none"` (and animation switched off globally) drives a plain signal instead of an `Animated` with a zero-length tween, because a tween that has no duration to divide by is a division waiting to happen, and "no transition" should not go anywhere near the ticker.
    fn fade_control(config: &Config) -> FadeControl {
        let instant = config.background.transition == WallpaperTransition::None
            || !config.animation.enabled
            || config.background.transition_ms == 0;
        if instant {
            let at = signal(0.0f32);
            let reading = at.read_only();
            return (
                Rc::new(move || reading.get()),
                Box::new(move |to| at.set(to)),
            );
        }
        let tween = config
            .animation
            .tween_ms(config.background.transition_ms, 10_000);
        let fade = Animated::new(0.0f32, tween);
        let reading = fade.clone();
        (
            Rc::new(move || reading.get()),
            Box::new(move |to| fade.retarget(to)),
        )
    }

    /// This screen's logical width, for how far a wipe has to travel.
    fn output_width(&self) -> f32 {
        platform_wayland::outputs()
            .into_iter()
            .find(|out| out.name == self.output || self.output.is_none())
            .and_then(|out| out.logical_size)
            .map(|(width, _)| width as f32)
            .filter(|width| *width > 0.0)
            .unwrap_or(FALLBACK_TRAVEL)
    }
}

/// The picture this screen opens on, decoded at most once per file.
///
/// A config reload rebuilds this surface's content, and a settings form applies itself while the user is still typing — so decoding the same file again on the UI thread would put a full image decode between every burst of keystrokes and the frame that answers it. One entry per screen, holding what that screen's live surface is already holding, and keyed by mtime as well as path so a picture overwritten in place still lands.
///
/// Only the *opening* image comes through here: a wallpaper chosen at runtime arrives already decoded, off the UI thread, from the wallpaper service.
fn decoded(output: Option<&str>, path: &Path) -> Option<Arc<ImageData>> {
    /// The picture a screen last opened on: which file, when it was written, and its pixels.
    type Opened = (PathBuf, SystemTime, Arc<ImageData>);
    thread_local! {
        static LAST: RefCell<HashMap<Option<String>, Opened>> = RefCell::new(HashMap::new());
    }
    let stamp = std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok();
    let key = output.map(str::to_string);
    let hit = LAST.with(|last| {
        last.borrow()
            .get(&key)
            .filter(|(cached, at, _)| cached == path && stamp == Some(*at))
            .map(|(_, _, image)| Arc::clone(image))
    });
    if hit.is_some() {
        return hit;
    }
    let image = Arc::new(util::picture::decode(path)?);
    if let Some(stamp) = stamp {
        LAST.with(|last| {
            last.borrow_mut()
                .insert(key, (path.to_path_buf(), stamp, Arc::clone(&image)))
        });
    }
    Some(image)
}

/// A full-surface style, used for every layer so they stack rather than sit side by side.
fn fill() -> LayoutStyle {
    LayoutStyle::new()
        .width(SizeDimension::Percent(1.0))
        .height(SizeDimension::Percent(1.0))
}

/// One image slot: absolutely filling the surface, shown in proportion to how close `fade` is to `visible_at`.
///
/// The image itself is rebuilt whenever the slot's signal changes — `Image::new` takes the data as a closure, so the layer is one node for the life of the surface and swapping the picture is a signal write, not a re-layout.
fn image_layer(
    slot: telar::ReadSignal<Option<Arc<ImageData>>>,
    fade: Rc<dyn Fn() -> f32>,
    visible_at: f32,
    transition: WallpaperTransition,
    travel: f32,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let data = slot.clone();
    let image = Image::new(
        fill(),
        move || data.get().unwrap_or_else(blank),
        || ImageFilter::Linear,
        || ObjectFit::Cover,
    )?;

    let present = slot;
    let opacity_fade = Rc::clone(&fade);
    let mut layer = StyledContainer::new(
        fill().absolute_fill(),
        |_| RectStyle::default(),
        vec![box_item(image)],
    )?
    .with_opacity(move || {
        // Both read before the early return: a slot that is empty this frame must still re-run when it fills.
        let at = opacity_fade();
        let filled = present.get().is_some();
        if !filled {
            return 0.0;
        }
        // `visible_at` is 0 or 1, so this is "how close the hand-over has got to me".
        1.0 - (at - visible_at).abs()
    });

    if transition == WallpaperTransition::Wipe {
        // A wipe is the incoming layer sliding over the outgoing one, so only the layer being *left* moves — the arriving one has to end up at rest exactly where the other was.
        layer = layer.with_transform(move |_| {
            let distance = (fade() - visible_at).abs();
            (distance != 0.0).then_some([1.0, 0.0, 0.0, 1.0, distance * travel, 0.0])
        });
    }
    Ok(Box::new(layer))
}

/// A single transparent pixel, stood in for an empty slot.
///
/// Shared rather than built per call: `ImageData::new` mints a new id every time, and a slot that handed the renderer a fresh id on every frame would fill the texture cache with copies of nothing.
fn blank() -> Arc<ImageData> {
    thread_local! {
        static BLANK: Arc<ImageData> = Arc::new(ImageData::new(vec![0, 0, 0, 0], 1, 1));
    }
    BLANK.with(Arc::clone)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn built(config: Config) -> Box<dyn Component> {
        WallpaperApp {
            config: Arc::new(config).into(),
            output: None,
        }
        .root()
    }

    #[test]
    fn the_surface_builds_with_and_without_an_image_and_under_every_transition() {
        // The build is where a layout error would surface, and nothing else runs these closures.
        let _ = built(Config::starter());

        for transition in WallpaperTransition::ALL {
            let mut config = Config::starter();
            config.background.enabled = true;
            config.background.transition = transition;
            let _ = built(config);
        }
    }

    /// The wallpaper is asked for by a picture, and by nothing that is merely drawn over it.
    #[test]
    fn a_widget_is_not_a_reason_to_paint_the_desktop() {
        let mut config = Config::starter();
        config.widgets.clock.enabled = true;
        config.widgets.visualiser.enabled = true;
        assert!(!config.background.is_enabled());
    }
}
