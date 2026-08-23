//! The capture flows: what a keybind, a chip or an IPC call means by "take a screenshot" or "record this".
//!
//! The services below know how to capture pixels and drive a recorder; this is the layer that decides *which*
//! pixels and what to do with them, so every entry point — a bar chip, `hogar-shell screenshot region`, the
//! utilities panel — performs the same flow rather than each assembling its own request.

mod picker;

pub use picker::{Picked, close as close_picker, is_open as picker_is_open, pick};

use config::ScreenshotConfig;
use services::recorder::{self, Scope};
use services::screenshot::{self, Request, Target};

fn config() -> ScreenshotConfig {
    config::config()
        .map(|c| c.screenshot.clone())
        .unwrap_or_default()
}

/// Every screen, composed into one picture.
pub fn screenshot_screen() {
    screenshot::take(Request::from_config(Target::Screen, &config()));
}

/// The focused screen. Falls back to the whole desktop when the compositor names no focused output — which is
/// one screen's worth on a single-monitor session anyway.
pub fn screenshot_output() {
    let target = match surfaces::shell::focused_output() {
        Some(name) => Target::Output(name),
        None => Target::Screen,
    };
    screenshot::take(Request::from_config(target, &config()));
}

/// A region the user draws.
///
/// The frozen still the picker was drawn over *is* the capture when there is one — see `picker`. Without one the
/// request goes back to the compositor for the selected rectangle, which is why the picker defers the callback
/// until its own surface is gone.
pub fn screenshot_region() {
    let config = config();
    pick(move |picked: Picked| {
        let request = Request::from_config(Target::Area(picked.area), &config);
        match picked.frozen {
            Some(image) => screenshot::deliver(image, request),
            None => screenshot::take(request),
        }
    });
}

/// The screenshot `[screenshot]` describes for `target`, for a caller that already knows which pixels it wants
/// (an IPC command naming an output).
pub fn screenshot(target: Target) {
    screenshot::take(Request::from_config(target, &config()));
}

pub fn record_screen() {
    recorder::start(Scope::Screen);
}

pub fn record_output() {
    let scope = match surfaces::shell::focused_output() {
        Some(name) => Scope::Output(name),
        None => Scope::Screen,
    };
    recorder::start(scope);
}

/// A region the user draws. The still is discarded: a recording is live pixels, and the only thing the picker
/// contributes is the rectangle.
pub fn record_region() {
    pick(move |picked: Picked| recorder::start(Scope::Area(picked.area)));
}

/// Stops a recording, or starts one of the whole screen. What the bar chip and the utilities toggle both do, so
/// one press is always the useful one.
pub fn toggle_recording() {
    if recorder::is_recording() {
        recorder::stop();
    } else {
        record_screen();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every flow resolves its own request from the config rather than hardcoding one, so a user who turned
    /// saving off gets a clipboard-only capture from the chip, the keybind and the panel alike.
    #[test]
    fn a_flow_takes_its_request_from_the_config() {
        let clipboard_only = ScreenshotConfig {
            save: false,
            copy: true,
            include_cursor: true,
            annotator: String::new(),
            ..ScreenshotConfig::default()
        };
        let request = Request::from_config(Target::Screen, &clipboard_only);
        assert!(!request.save && request.copy);
        assert!(request.cursor, "the cursor setting reaches the request");
        assert!(
            !request.annotate,
            "with no annotator configured there is nothing to hand off to"
        );

        let annotated = ScreenshotConfig {
            annotator: "satty --filename {file}".to_string(),
            ..ScreenshotConfig::default()
        };
        assert!(Request::from_config(Target::Screen, &annotated).annotate);
    }
}
