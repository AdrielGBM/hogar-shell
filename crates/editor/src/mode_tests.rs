//! What entering and leaving an edit mode must leave behind: every window exactly as it was, whichever mode, whichever screen, and whether the screen stayed or went.
//!
//! At the level of the live windows rather than of a running compositor: what the host asks of the compositor is the layer each window is asked to be on, the keyboard it negotiates and whether it is on screen at all, and those are what the windows answer here.

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::collections::BTreeMap;
    use std::rc::Rc;
    use std::sync::Arc;

    use config::Config;
    use layout::{LayerKind, LayoutStore};
    use platform_wayland::{KeyboardMode, Layer, OutputDescriptor};
    use surfaces::layer_window::Content;
    use surfaces::reconcile::{Desktop, Shell, plan};
    use surfaces::transient;

    use crate::mode::{self, Compositor, Mode};

    const LEFT: &str = "DP-1";
    const RIGHT: &str = "HDMI-A-1";

    fn screen(name: &str) -> OutputDescriptor {
        OutputDescriptor {
            name: Some(name.to_string()),
            logical_size: Some((1920, 1080)),
            position: (0, 0),
            scale: 1,
        }
    }

    fn planned(screens: &[&str]) -> Vec<Desktop> {
        let outputs: Vec<OutputDescriptor> = screens.iter().map(|name| screen(name)).collect();
        plan(
            &util::paths::config_dir().join("config.toml"),
            &Arc::new(Config::default()),
            &layout::built_in(),
            &BTreeMap::new(),
            &outputs,
            &|_| None,
        )
        .0
    }

    /// Two screens up, the transient registry pointed at their windows, and the mode watching them.
    fn shell_on(screens: &[&str]) -> Shell {
        telar::set_locale("en");
        mode::leave();
        let mut shell = Shell::new();
        shell.reconcile(&planned(screens), Content::Rebuild);
        transient::install(shell.windows().holder());
        mode::install();
        shell
    }

    fn compositor(restack: bool) -> Compositor {
        Compositor {
            restack,
            locked: false,
            lockable: Err("this compositor does not implement ext-session-lock-v1".to_string()),
        }
    }

    #[derive(Debug, PartialEq)]
    struct Window {
        layer: Option<Layer>,
        keyboard: Option<KeyboardMode>,
        mapped: bool,
        open: bool,
        concealed: bool,
    }

    /// What every window on `screens` is asking of the compositor right now.
    fn windows(shell: &Shell, screens: &[&str]) -> Vec<(String, LayerKind, Window)> {
        let windows = shell.windows();
        screens
            .iter()
            .flat_map(|output| LayerKind::SESSION.map(|layer| (*output, layer)))
            .map(|(output, layer)| {
                let demands = windows.demands(Some(output), layer);
                (
                    output.to_string(),
                    layer,
                    Window {
                        layer: demands.as_ref().map(|demands| demands.layer()),
                        keyboard: demands.as_ref().map(|demands| demands.keyboard_wanted()),
                        mapped: windows.is_mapped(Some(output), layer),
                        open: windows.is_open(Some(output), layer),
                        concealed: windows.is_concealed(Some(output), layer),
                    },
                )
            })
            .collect()
    }

    /// Enters `layer`'s mode on each screen in turn, with and without a compositor that can raise a window, and leaves it again; every window on both screens must come back exactly as it was each time.
    fn every_window_comes_back_from(layer: LayerKind) {
        let both = [LEFT, RIGHT];
        let shell = shell_on(&both);
        let before = windows(&shell, &both);
        for restack in [true, false] {
            for output in both {
                let entered = mode::enter_as(layer, Some(output), &compositor(restack))
                    .unwrap_or_else(|why| panic!("{layer} on {output}: {why}"));
                assert_eq!(mode::current(), Some(entered.clone()));
                assert!(
                    shell.windows().is_open(Some(output), LayerKind::Overlay),
                    "the host is up in the overlay window of {output}"
                );
                assert!(transient::is_open(&format!("edit:{output}")));
                let held = windows(&shell, &both);
                match layer {
                    LayerKind::Background | LayerKind::Desktop if restack => {
                        let raised = shell.windows().demands(Some(output), layer).unwrap();
                        assert_eq!(raised.layer(), Layer::Top, "{layer} is raised on {output}");
                        assert!(shell.windows().is_mapped(Some(output), layer));
                    }
                    LayerKind::Background | LayerKind::Desktop => assert!(
                        shell.windows().is_concealed(Some(output), layer),
                        "{layer}'s own window sets its areas aside while the host stands in for it"
                    ),
                    LayerKind::Top | LayerKind::Overlay => {
                        assert!(shell.windows().is_mapped(Some(output), layer));
                    }
                    LayerKind::Lock => {}
                }
                assert!(
                    held.iter()
                        .filter(|(on, _, _)| on != output)
                        .all(|held| before.contains(held)),
                    "the other screen is untouched while {output} is edited"
                );

                assert_eq!(mode::leave(), Some(entered));
                assert_eq!(mode::current(), None);
                assert!(!transient::is_open(&format!("edit:{output}")));
                assert_eq!(
                    windows(&shell, &both),
                    before,
                    "leaving {layer} on {output} (restack: {restack}) put every window back"
                );
            }
        }
    }

    #[test]
    fn background_mode_leaves_every_window_as_it_found_it() {
        every_window_comes_back_from(LayerKind::Background);
    }

    #[test]
    fn desktop_mode_leaves_every_window_as_it_found_it() {
        every_window_comes_back_from(LayerKind::Desktop);
    }

    #[test]
    fn top_mode_leaves_every_window_as_it_found_it() {
        every_window_comes_back_from(LayerKind::Top);
    }

    #[test]
    fn overlay_mode_leaves_every_window_as_it_found_it() {
        every_window_comes_back_from(LayerKind::Overlay);
    }

    #[test]
    fn lock_preview_leaves_every_window_as_it_found_it() {
        every_window_comes_back_from(LayerKind::Lock);
    }

    /// One mode at a time (DEC-2): a switch gives back what the mode it left held before the next takes anything, so what is left after the last one ends is what was there before the first.
    #[test]
    fn switching_modes_holds_only_what_the_last_one_asked_for() {
        let shell = shell_on(&[LEFT]);
        let before = windows(&shell, &[LEFT]);
        mode::enter_as(LayerKind::Background, Some(LEFT), &compositor(true)).unwrap();
        mode::enter_as(LayerKind::Desktop, Some(LEFT), &compositor(true)).unwrap();
        let windows_now = shell.windows();
        assert_eq!(
            windows_now
                .demands(Some(LEFT), LayerKind::Background)
                .unwrap()
                .layer(),
            Layer::Background,
            "the background went back down when its mode was left"
        );
        assert_eq!(
            windows_now
                .demands(Some(LEFT), LayerKind::Desktop)
                .unwrap()
                .layer(),
            Layer::Top
        );
        assert_eq!(
            mode::current().map(|mode| mode.layer),
            Some(LayerKind::Desktop)
        );
        mode::leave();
        assert_eq!(windows(&shell, &[LEFT]), before);
    }

    /// The host closing — which is what Esc does once nothing opened after it is left on the dismiss stack — ends the mode it belongs to and gives back everything it held.
    #[test]
    fn the_host_closing_ends_its_mode() {
        let shell = shell_on(&[LEFT]);
        let before = windows(&shell, &[LEFT]);
        mode::enter_as(LayerKind::Desktop, Some(LEFT), &compositor(true)).unwrap();
        transient::close(&format!("edit:{LEFT}"));
        assert_eq!(mode::current(), None);
        assert_eq!(windows(&shell, &[LEFT]), before);
    }

    /// A screen unplugged mid-edit takes its windows and the host with it, without the host ever closing; the mode still ends, nothing panics on the way, and the screen that stayed is exactly as it was.
    #[test]
    fn unplugging_the_edited_screen_ends_its_mode() {
        let mut shell = shell_on(&[LEFT, RIGHT]);
        let before = windows(&shell, &[LEFT]);
        for (layer, restack) in [
            (LayerKind::Desktop, true),
            (LayerKind::Background, false),
            (LayerKind::Lock, true),
        ] {
            shell.reconcile(&planned(&[LEFT, RIGHT]), Content::Changed);
            mode::enter_as(layer, Some(RIGHT), &compositor(restack)).unwrap();

            shell.reconcile(&planned(&[LEFT]), Content::Changed);

            assert_eq!(mode::current(), None, "{layer} ended with its screen");
            assert!(!transient::is_open(&format!("edit:{RIGHT}")));
            assert_eq!(windows(&shell, &[LEFT]), before);
        }
    }

    #[test]
    fn a_screen_plugged_in_elsewhere_leaves_the_mode_alone() {
        let mut shell = shell_on(&[LEFT]);
        let entered = mode::enter_as(LayerKind::Top, Some(LEFT), &compositor(true)).unwrap();
        shell.reconcile(&planned(&[LEFT, RIGHT]), Content::Changed);
        assert_eq!(mode::current(), Some(entered));
        mode::leave();
    }

    /// TA-8: lock mode is a preview of an unlocked session, so it is refused outright while the session is locked — and nothing is held for the attempt.
    #[test]
    fn lock_mode_is_refused_while_the_session_is_locked() {
        let shell = shell_on(&[LEFT]);
        let before = windows(&shell, &[LEFT]);
        let locked = Compositor {
            locked: true,
            ..compositor(true)
        };
        let refused =
            mode::enter_as(LayerKind::Lock, Some(LEFT), &locked).expect_err("refused while locked");
        assert!(refused.contains("unlocked"), "{refused}");
        assert_eq!(mode::current(), None);
        assert!(!transient::is_open(&format!("edit:{LEFT}")));
        assert_eq!(windows(&shell, &[LEFT]), before);
        assert!(
            mode::enter_as(LayerKind::Desktop, Some(LEFT), &locked).is_ok(),
            "the session's own layers are not the lock's to refuse"
        );
        mode::leave();
    }

    /// Entering and leaving lock mode never asks for a session lock: the opener the lock service would call is never called, and no lock is held when it is done. On a machine that cannot lock, the mode still opens and carries the reason instead of tools.
    #[test]
    fn lock_mode_opens_no_lock_session() {
        let _shell = shell_on(&[LEFT]);
        let asked = Rc::new(Cell::new(0));
        services::lock::set_session_opener({
            let asked = Rc::clone(&asked);
            move |_| {
                asked.set(asked.get() + 1);
                panic!("lock mode asked for a session lock")
            }
        });

        let entered = mode::enter_as(LayerKind::Lock, Some(LEFT), &compositor(true)).unwrap();
        assert!(
            entered
                .refused
                .as_deref()
                .is_some_and(|why| why.contains("ext-session-lock-v1")),
            "{entered:?}"
        );
        mode::leave();

        let lockable = Compositor {
            lockable: Ok(()),
            ..compositor(true)
        };
        let entered = mode::enter_as(LayerKind::Lock, Some(LEFT), &lockable).unwrap();
        assert_eq!(entered.refused, None, "a machine that can lock gets tools");
        mode::leave();

        assert_eq!(asked.get(), 0);
        assert!(!platform_wayland::session_is_locked());
        assert!(!services::lock::is_locked());
    }

    /// F-10.35: `--safe-layout` refuses every edit, and a mode is an edit waiting to happen.
    #[test]
    fn no_mode_opens_under_the_safe_layout() {
        let shell = shell_on(&[LEFT]);
        let before = windows(&shell, &[LEFT]);
        let dir = util::paths::isolated_root()
            .expect("a test process resolves under its scratch root")
            .join("editor-safe-layout");
        surfaces::layouts::install(
            Rc::new(RefCell::new(LayoutStore::safe(dir))),
            Rc::new(|| {}),
        );
        for layer in LayerKind::ALL {
            let refused = mode::enter_as(layer, Some(LEFT), &compositor(true))
                .expect_err("the recovery flag refuses every mode");
            assert!(refused.contains("--safe-layout"), "{refused}");
        }
        assert_eq!(mode::current(), None);
        assert_eq!(windows(&shell, &[LEFT]), before);
    }

    #[test]
    fn a_screen_the_shell_does_not_draw_on_is_refused_by_name() {
        let _shell = shell_on(&[LEFT]);
        let refused = mode::enter_as(LayerKind::Top, Some("VGA-9"), &compositor(true))
            .expect_err("not a screen");
        assert!(
            refused.contains("VGA-9") && refused.contains(LEFT),
            "{refused}"
        );
    }

    /// Entering the mode that is already up is not a new session: nothing is closed and reopened under the user, and a tool reading the mode is not told it changed.
    #[test]
    fn entering_the_mode_already_up_changes_nothing() {
        let _shell = shell_on(&[LEFT]);
        let entered = mode::enter_as(LayerKind::Top, Some(LEFT), &compositor(true)).unwrap();
        let seen: Rc<RefCell<Vec<Option<Mode>>>> = Rc::default();
        let _reading = telar::effect({
            let seen = Rc::clone(&seen);
            move || seen.borrow_mut().push(mode::active().get())
        });
        let again = mode::enter_as(LayerKind::Top, Some(LEFT), &compositor(true)).unwrap();
        assert_eq!(again, entered);
        assert_eq!(*seen.borrow(), vec![Some(entered)]);
        mode::leave();
        assert_eq!(seen.borrow().last(), Some(&None));
    }
}
