use std::rc::Rc;
use ui::scale::paint;

use platform_wayland::request_close;
use telar::{
    LayoutError, LayoutItem, LayoutStyle, StyledContainer, SurfaceFrameStyle, SurfaceToken,
    box_item, window_frame, use_theme,
};

use crate::drawer::{content_radius, module_panel, panel_wants_keyboard};
use config::SurfaceEnv;
use config::theme::{FontRole, NordTheme};
use ui::panel::PanelSurface;
use ui::placement::{Centred, Placement};

/// Opens `module_id`'s panel as a centred, titled, closable window on the bar's own monitor, sized per its `[modules.<id>]` override or `[panels.float]`; the shell only declares the placement, the rsx surface host and `window_frame` realize the window chrome. Toggle/close is the caller's job ([`crate::panel::toggle_panel`]) via the returned token.
///
/// `[modules.<id>]` (or `[panels.float]`) is the size the window opens at, and for now the size it keeps.
///
/// **The corner grip is gone until this is an xdg toplevel.** Dragging one costs a swapchain rebuild per
/// step, and a rebuild is a `vkDeviceWaitIdle` on the device every surface shares — measured at ~11 ms
/// against ~1.5 ms to draw the frame. Making that cheaper means not rebuilding on every step, which means
/// holding the layout still for a moment; and a layer surface has no interactive-resize protocol, so the grip
/// is the client's own — it reads laid-out rects, and the pointer only reaches it through an input region
/// built from that same laid-out tree. Holding the layout holds the pointer, and the drag then advances at
/// the throttle's pace rather than the cursor's. `xdg_toplevel.resize` gives the grab and the sizing to the
/// compositor, which unties it.
pub(crate) fn open_float(env: &SurfaceEnv, module_id: &str) -> SurfaceToken {
    let module = module_id.to_string();
    let (width, height) = env.config.float_size_for(module_id);
    let placement = Placement::centred(Centred::Float)
        .size(width, height)
        .keyboard(panel_wants_keyboard(module_id))
        .output(env.output.clone());
    // A float hangs off no edge of its own, so it reads the bar its chip lives on — which is what makes its
    // radius, gaps and opacity match the drawer showing the very same panel.
    PanelSurface::new(placement, move |env| {
        let theme = use_theme::<NordTheme>();
        let radius = content_radius();
        let body = module_panel(&module).expect("float panel build failed");
        let style = SurfaceFrameStyle {
            background: env.config.panel_fill(),
            title_bar: theme.overlay,
            title_text: theme.text,
            close: theme.muted,
            radius,
            font_size: theme.font(FontRole::Title),
        };
        let close: Rc<dyn Fn()> = Rc::new(request_close);
        window_frame(module.clone(), style, close, body, None).expect("surface frame build failed")
    })
    .edge(env.edge)
    .open()
}

/// The window chrome a float is presented in — title bar, ✕ and a placeholder body — for [`crate::preview`].
/// The chrome rather than a module's panel, because *which* panel a float shows is the caller's choice and
/// every one of them already previews on its own.
pub(crate) fn frame_preview() -> Result<Box<dyn LayoutItem>, LayoutError> {
    let theme = use_theme::<NordTheme>();
    let body = box_item(StyledContainer::new(
        LayoutStyle::new().width(220.0).height(90.0),
        paint::md(theme.overlay),
        vec![],
    )?);
    let style = SurfaceFrameStyle {
        background: theme.surface,
        title_bar: theme.overlay,
        title_text: theme.text,
        close: theme.muted,
        radius: 14.0,
        font_size: theme.font(FontRole::Title),
    };
    window_frame("Clock", style, Rc::new(|| {}), body, None)
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use platform_headless::{FrameSink, HeadlessPlatform};
    use telar::{
        App, AppConfig, AppPathsProvider, Color, Component, SurfaceRoot, WindowConfig,
        reset_layout_runtime, run_with_platform, set_theme,
    };

    use config::theme::NordTheme;

    struct NullPaths;
    impl AppPathsProvider for NullPaths {
        fn config_dir(&self) -> Option<std::path::PathBuf> {
            None
        }
        fn data_dir(&self) -> Option<std::path::PathBuf> {
            None
        }
        fn cache_dir(&self) -> Option<std::path::PathBuf> {
            None
        }
    }

    /// The float's chrome under the enter animation, which is the one thing a `[preview]` cannot show: the
    /// preview page renders a tree, and this is about what the *surface root* does to it over several frames.
    struct AnimatedFloat;

    impl App for AnimatedFloat {
        fn root(&self) -> Box<dyn Component> {
            reset_layout_runtime();
            set_theme(NordTheme::new());
            let frame = super::frame_preview().expect("float frame build failed");
            Box::new(
                SurfaceRoot::new(frame)
                    .expect("float surface root failed")
                    .animate_in(),
            )
        }
        fn window_config(&self) -> Option<WindowConfig> {
            Some(WindowConfig {
                is_transparent: true,
                ..WindowConfig::default()
            })
        }
        fn clear_color(&self) -> Option<Color> {
            None
        }
    }

    /// A float that animates in must *land*. The enter transition fades the whole surface from transparent, so
    /// a transition that never completes leaves a window the user cannot see — and every other check passes,
    /// because the tree is built, laid out and drawn exactly as it should be. Only the pixels say otherwise.
    #[test]
    fn a_float_that_animates_in_ends_up_visible() {
        const SIDE: u32 = 240;
        let sink: FrameSink = Arc::new(Mutex::new(None));
        // The headless platform paces at a real 60fps, so 20 frames is a comfortable margin over the 200ms
        // enter transition.
        let platform = HeadlessPlatform::new(SIDE, SIDE)
            .with_frames(20)
            .capture_into(sink.clone());
        run_with_platform::<_, _, ()>(
            platform,
            AppConfig::default(),
            Box::new(NullPaths) as Box<dyn AppPathsProvider>,
            AnimatedFloat,
            "hyprshell-float-test",
        )
        .expect("headless run");

        let pixels = sink.lock().unwrap().take().expect("a frame was captured");
        let opaque = pixels.chunks_exact(4).filter(|px| px[3] > 250).count();
        assert!(
            opaque > (SIDE * SIDE / 10) as usize,
            "the settled frame is {opaque} solid pixels of {}: the enter transition never finished",
            SIDE * SIDE
        );
    }
}
