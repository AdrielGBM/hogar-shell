//! The desktop's reduced-motion setting, as `[animation] reduced = "auto"` reads it.

use std::sync::atomic::{AtomicBool, Ordering};

/// Process-global rather than a field on `Config` for the reason the dynamic scheme is: every surface build derives its durations synchronously, while the setting arrives from the desktop whenever it changes.
static DESKTOP_REDUCED: AtomicBool = AtomicBool::new(false);

pub fn desktop_prefers_reduced() -> bool {
    DESKTOP_REDUCED.load(Ordering::Relaxed)
}

/// Records the desktop's setting, answering whether it changed.
pub fn set_desktop_reduced(reduced: bool) -> bool {
    DESKTOP_REDUCED.swap(reduced, Ordering::Relaxed) != reduced
}

/// Records the desktop's setting and rebuilds the surfaces when it changed, since their transitions are derived as they are built.
pub fn follow_desktop(reduced: bool) {
    if set_desktop_reduced(reduced) {
        crate::live::request_reload();
    }
}
