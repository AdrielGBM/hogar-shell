//! The desktop's reduced-motion setting, as `[animation] reduced = "auto"` reads it.

use std::sync::atomic::{AtomicBool, Ordering};

/// Process-global rather than a field on `Config` for the reason the dynamic scheme is: every surface build derives its durations synchronously, while the setting arrives from the desktop whenever it changes.
static DESKTOP_REDUCED: AtomicBool = AtomicBool::new(false);

pub fn desktop_prefers_reduced() -> bool {
    DESKTOP_REDUCED.load(Ordering::Relaxed)
}

/// Records the desktop's setting, answering whether it changed. The startup path records it before the first build, so the portal's first delivery is that same answer and no change.
pub fn set_desktop_reduced(reduced: bool) -> bool {
    DESKTOP_REDUCED.swap(reduced, Ordering::Relaxed) != reduced
}

/// Records the desktop's setting and, when it changed, draws the surfaces that follow it again, since their transitions are derived as they are built.
pub fn follow_desktop(reduced: bool) {
    if set_desktop_reduced(reduced) {
        restyle_followers();
    }
}

/// Nothing is drawn again when no running config follows the desktop — every one says `on`, `off` or `enabled = false` — since no surface would look any different.
fn restyle_followers() {
    if crate::live::any_running(|config| config.animation.follows_desktop()) {
        crate::live::request_restyle();
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;
    use std::sync::Arc;

    use super::*;
    use crate::{AnimationConfig, Config, ReducedMotion};

    fn running(animation: AnimationConfig) -> Arc<Config> {
        Arc::new(Config {
            animation,
            ..Config::default()
        })
    }

    fn counting_restyles() -> Rc<Cell<usize>> {
        let restyles = Rc::new(Cell::new(0));
        crate::live::set_restyle_hook({
            let restyles = Rc::clone(&restyles);
            move || restyles.set(restyles.get() + 1)
        });
        restyles
    }

    #[test]
    fn only_a_running_config_that_follows_the_desktop_is_drawn_again() {
        let restyles = counting_restyles();
        let following = AnimationConfig::default();
        assert!(following.follows_desktop());

        crate::live::set_config(running(following.clone()));
        restyle_followers();
        assert_eq!(restyles.get(), 1);

        for decided in [
            AnimationConfig {
                reduced: ReducedMotion::On,
                ..following.clone()
            },
            AnimationConfig {
                reduced: ReducedMotion::Off,
                ..following.clone()
            },
            AnimationConfig {
                enabled: false,
                ..following.clone()
            },
        ] {
            crate::live::set_config(running(decided.clone()));
            restyle_followers();
            assert_eq!(restyles.get(), 1, "{decided:?} cannot look any different");
        }

        crate::live::set_output_config("restyle-DP-1", running(following));
        restyle_followers();
        assert_eq!(
            restyles.get(),
            2,
            "a screen whose override follows the desktop is drawn again"
        );
        crate::live::clear_config();
    }

    #[test]
    fn the_setting_the_shell_started_with_is_no_change() {
        let restyles = counting_restyles();
        crate::live::set_config(running(AnimationConfig::default()));

        follow_desktop(desktop_prefers_reduced());

        assert_eq!(
            restyles.get(),
            0,
            "the portal's first delivery is what startup already recorded, so nothing is drawn again"
        );
        crate::live::clear_config();
    }
}
