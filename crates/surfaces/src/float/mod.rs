use std::rc::Rc;
use ui::scale::paint;

use telar::{
    Color, Container, LayoutError, LayoutItem, LayoutStyle, StyledContainer, SurfaceFrameStyle,
    box_item, use_theme, window_frame,
};

use config::theme::{FontRole, NordTheme};
use ui::chrome::{Chrome, content_radius};
use ui::descriptor::Built;
use ui::host::{Host, Instance, Representation, Size};

/// There is no resize grip: a layer surface has no interactive-resize protocol, and a client-drawn grip reads the laid-out rects the input region is carved from, so throttling it throttles the pointer.
pub(crate) fn content(instance: &Instance, chrome: &Chrome) -> Built {
    let theme = use_theme::<NordTheme>();
    let module_id = &*instance.module;
    let (width, height) = instance.presentation(&chrome.config).float_size(module_id);
    let host = Host::in_chrome(
        instance.clone(),
        Representation::Panel,
        chrome,
        Size {
            width: width as f32,
            height: height as f32,
        },
    );
    let body = ui::descriptor::build_panel(&host)?;
    let style = frame_style(theme, chrome.config.panel_fill(), content_radius());
    let module = module_id.to_string();
    let close: Rc<dyn Fn()> = Rc::new(move || crate::transient::close(&module));
    let frame = window_frame(module_id.to_string(), None, style, close, body, None)?;
    Ok(box_item(Container::new(
        LayoutStyle::new()
            .flex_column()
            .width(width as f32)
            .height(height as f32),
        vec![frame],
    )?))
}
fn frame_style(theme: NordTheme, background: Color, radius: f32) -> SurfaceFrameStyle {
    SurfaceFrameStyle {
        background,
        title_bar: theme.overlay,
        title_text: theme.text,
        close: theme.muted,
        radius,
        font_size: theme.font(FontRole::Title),
        // A layer-shell surface has no top-level window: nothing to minimize, and nothing to drag with the compositor's own move. The frame draws close and, where the backend can renegotiate, a grip.
        controls: Default::default(),
        body_inset: 12.0,
        control_hover: Color::TRANSPARENT,
        close_hover: Color::TRANSPARENT,
    }
}

/// The window chrome a float is presented in — title bar, ✕ and a placeholder body — for [`crate::preview`]. The chrome rather than a module's panel, because *which* panel a float shows is the caller's choice and every one of them already previews on its own.
pub(crate) fn frame_preview() -> Result<Box<dyn LayoutItem>, LayoutError> {
    let theme = use_theme::<NordTheme>();
    let body = box_item(StyledContainer::new(
        LayoutStyle::new().width(220.0).height(90.0),
        paint::md(theme.overlay),
        vec![],
    )?);
    let style = frame_style(theme, theme.surface, 14.0);
    window_frame("Clock", None, style, Rc::new(|| {}), body, None)
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use platform_headless::{FrameSink, HeadlessPlatform};
    use telar::{
        App, AppConfig, AppPathsProvider, Color, Component, WindowConfig, WindowRoot,
        reset_layout_runtime, run_with_platform, set_theme,
    };

    use config::theme::NordTheme;

    /// The float's chrome under the enter animation, which is the one thing a `[preview]` cannot show: the preview page renders a tree, and this is about what the *surface root* does to it over several frames.
    struct AnimatedFloat;

    impl App for AnimatedFloat {
        fn root(&self) -> Box<dyn Component> {
            reset_layout_runtime();
            set_theme(NordTheme::new());
            let frame = super::frame_preview().expect("float frame build failed");
            Box::new(
                WindowRoot::wrapping(frame)
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

    /// A float that animates in must *land*. The enter transition fades the whole surface from transparent, so a transition that never completes leaves a window the user cannot see — and every other check passes, because the tree is built, laid out and drawn exactly as it should be. Only the pixels say otherwise.
    #[test]
    fn a_float_that_animates_in_ends_up_visible() {
        const SIDE: u32 = 240;
        let sink: FrameSink = Arc::new(Mutex::new(None));
        // The headless platform paces at a real 60fps, so 20 frames is a comfortable margin over the 200ms enter transition.
        let platform = HeadlessPlatform::new(SIDE, SIDE)
            .with_frames(20)
            .capture_into(sink.clone());
        run_with_platform::<_, _, ()>(
            platform,
            AppConfig::default(),
            std::sync::Arc::new(telar::NoPaths) as std::sync::Arc<dyn AppPathsProvider>,
            AnimatedFloat,
            "hogar-shell-float-test",
        )
        .expect("headless run");

        let pixels = sink.lock().unwrap().take().expect("a frame was captured");
        let opaque = pixels
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|px| px[3] > 250)
            .count();
        assert!(
            opaque > (SIDE * SIDE / 10) as usize,
            "the settled frame is {opaque} solid pixels of {}: the enter transition never finished",
            SIDE * SIDE
        );
    }
}
