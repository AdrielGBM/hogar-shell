//! The shell's live context and its open-surface registry.
//!
//! Two things every entry point needs and no single bar owns. **The context** is the config the shell is currently running (kept in step with the reload watcher) plus the compositor's focused monitor, so code reached from outside a surface — an IPC call, a keybind — can still answer "which config? which screen?". **The registry** is what is open right now, so a panel toggled from a bar chip, from `hogar-shell panel toggle`, and from a keybind are all the *same* surface rather than three stacked copies.
//!
//! What is in the registry is what the *user* opened. The surfaces the *config* describes — the bars, their reservation strips, the wallpaper, its widgets, the frame — are [`crate::reconcile`]'s, and the split is what makes a reload safe to run at every keystroke: one side is reconciled against the file, the other is left alone.
//!
//! Both live on the driver thread, which is the one UI thread every surface shares, so they are plain thread-locals rather than locks.

use std::cell::RefCell;
use std::collections::HashMap;

use telar::SurfaceToken;

use config::Edge;
use config::SurfaceEnv;
use config::fingerprint::{Fingerprint, Reload, Stamp};
use ui::panels;

thread_local! {
    static OPEN: RefCell<OpenSurfaces> = RefCell::new(OpenSurfaces::default());
}

/// What is on screen beyond the bars. A drawer is single-slot — two of them would be two cards hanging off one bar, each catching the presses meant for the other — while floats and overlays are independent windows, each keyed by its own id.
#[derive(Default)]
struct OpenSurfaces {
    drawer: Option<(String, Open)>,
    windows: HashMap<String, Open>,
}

/// One surface the user opened, and the config content it was last built from.
///
/// The stamp lives on the entry rather than in a table beside it so that it goes when the surface does: a window closed and opened again is a new build from whatever the file held by then, and inheriting the stamp of the one it replaced would have a reload pass it by for content it never showed.
struct Open {
    token: SurfaceToken,
    stamp: Stamp,
}

impl From<SurfaceToken> for Open {
    /// Opened with an empty stamp: the registry did not see what it was built from, and an empty stamp is the one that cannot suppress a reload it should not.
    fn from(token: SurfaceToken) -> Self {
        Self {
            token,
            stamp: Stamp::default(),
        }
    }
}

/// The monitor a surface opened from outside a bar should land on: whichever Hyprland reports as focused, else the compositor's default. Queried per call rather than cached — the focused monitor is exactly the thing that changes between one keypress and the next.
pub fn focused_output() -> Option<String> {
    let dir = services::hyprland::socket_dir()?;
    services::hyprland::focused_monitor(&dir)
}

/// The environment a module's panel should open with when there is no bar surface in scope — an IPC call or a keybind rather than a chip click. Anchors the panel to the bar the module actually sits on (so its drawer hangs off the right edge and aligns to the right zone), falling back to the top edge for a module that is configured nowhere.
pub fn env_for_module(module_id: &str) -> Option<SurfaceEnv> {
    config::config()?;
    let output = focused_output();
    // The screen it will open on decides which config it resolves against, so a panel opened by a keybind reads the same per-monitor overrides as one opened from that screen's bar.
    let config = config::config_for(output.as_deref());
    let edge = Edge::ALL
        .into_iter()
        .find(|edge| config.zone_of(*edge, module_id).is_some())
        .unwrap_or(Edge::Top);
    Some(SurfaceEnv {
        edge,
        bar_size: config.bars.get(edge).size,
        output,
        config,
    })
}

/// Whether `id`'s drawer is the one currently showing.
pub fn drawer_is_open(id: &str) -> bool {
    OPEN.with(|open| {
        open.borrow()
            .drawer
            .as_ref()
            .is_some_and(|(open_id, open)| open_id == id && !open.token.is_closing())
    })
}

/// Closes whatever drawer is up and opens `id`'s, unless it was already the one showing — in which case this is a close. Dropping the previous token is what tears the old drawer down.
pub fn toggle_drawer(id: &str, open: impl FnOnce() -> SurfaceToken) {
    let already_open = drawer_is_open(id);
    OPEN.with(|surfaces| surfaces.borrow_mut().drawer = None);
    if !already_open {
        let token = open();
        OPEN.with(|surfaces| {
            surfaces.borrow_mut().drawer = Some((id.to_string(), token.into()));
        });
    }
}

/// Whether the independent surface `id` (a float, a launcher, a session menu) is up.
pub fn window_is_open(id: &str) -> bool {
    OPEN.with(|open| {
        open.borrow()
            .windows
            .get(id)
            .is_some_and(|open| !open.token.is_closing())
    })
}

/// Opens or closes `id`, a window the user opens deliberately and closes deliberately: the launcher, a module's float, the notification centre. Whatever drawer was up goes first — see [`close_drawer`].
///
/// **What it does not touch is another standing window.** A float is the presentation you pick when you want a panel to stay put; the notification centre is where a morning's notifications are worked through. Closing either because a second window opened would be taking away something still in use — and a standing window can be closed by hand, which is the whole difference between it and a glance.
///
/// Closing one is only that: a second press on the settings chip is "put this away", and taking the drawer with it would close something the user never asked about.
pub fn toggle_standing_window(id: &str, open: impl FnOnce() -> SurfaceToken) {
    if !window_is_open(id) {
        close_drawer();
    }
    toggle_window(id, open);
}

/// Closes the drawer, if one is up, telling its module that the user is done with it.
///
/// **The one surface another window takes away.** A drawer is a glance: it hangs off the chip you pressed and a press outside it dismisses it. Its surface also covers the whole usable area — that is how the press outside reaches it — so a window opening *under* one is a window that is painted, unreachable, and dismissed rather than used by the first press that goes near it.
///
/// Nothing else goes. A toast, a notification popup and the OSD are pinned to an edge and say something the user did not open a window to be told. A float and the notification centre were opened deliberately, and each other's arrival is not a reason to take one away. The region picker takes the whole screen and still does not come through here — it is drawn over a still taken the instant before it mapped, so closing a drawer first would take out of the capture exactly what the user opened the picker to photograph.
pub fn close_drawer() {
    let open = OPEN.with(|surfaces| surfaces.borrow().drawer.as_ref().map(|(id, _)| id.clone()));
    if let Some(id) = open {
        panels::closed(&id);
        close(&id);
    }
}

/// Opens or closes the independent surface `id`, leaving every other one alone.
pub fn toggle_window(id: &str, open: impl FnOnce() -> SurfaceToken) {
    let already_open = window_is_open(id);
    OPEN.with(|surfaces| surfaces.borrow_mut().windows.remove(id));
    if !already_open {
        let token = open();
        OPEN.with(|surfaces| {
            surfaces
                .borrow_mut()
                .windows
                .insert(id.to_string(), token.into());
        });
    }
}

/// Closes `id` whether it is the open drawer or an independent surface. A close of something already closed is a no-op, so `hogar-shell panel close x` is safe to call blind.
pub fn close(id: &str) {
    OPEN.with(|surfaces| {
        let mut surfaces = surfaces.borrow_mut();
        if surfaces.drawer.as_ref().is_some_and(|(open, _)| open == id) {
            surfaces.drawer = None;
        }
        surfaces.windows.remove(id);
    });
}

/// Every surface currently up, for `hogar-shell panel list`.
pub fn open_ids() -> Vec<String> {
    OPEN.with(|surfaces| {
        let surfaces = surfaces.borrow();
        let drawer = surfaces
            .drawer
            .iter()
            .filter(|(_, open)| !open.token.is_closing())
            .map(|(id, _)| id.clone());
        let windows = surfaces
            .windows
            .iter()
            .filter(|(_, open)| !open.token.is_closing())
            .map(|(id, _)| id.clone());
        let mut ids: Vec<String> = drawer.chain(windows).collect();
        ids.sort();
        ids
    })
}

/// Records that `id` already shows `content`, so a reload of exactly that content passes it by. A no-op for a surface that is not open.
///
/// **A surface is not rebuilt by a change it already shows.** The settings window is the case that matters: it applies a form a moment after the last keystroke, and rebuilding the field being typed into would put the caret back at the start of it — the whole reason live editing was unusable. A stamp of *content* rather than of authorship, because "this window shows these bytes" is the actual rule: an edit made anywhere else — `hogar-shell`, a text editor, a second write landing before the reload — is different content, and reaches the window like any other surface however close behind its own write it lands.
///
/// Called once the bytes are on disk, with the fingerprint of those very bytes rather than of a second look at the file: a look before the write names the content the write replaced, and one after it can take in an edit landing behind the write — either way a stamp for something the surface does not show.
pub fn stamp(id: &str, content: Fingerprint) {
    OPEN.with(|surfaces| {
        let mut surfaces = surfaces.borrow_mut();
        let OpenSurfaces { drawer, windows } = &mut *surfaces;
        let drawer = drawer
            .as_mut()
            .filter(|(open_id, _)| open_id == id)
            .map(|(_, open)| open);
        for open in drawer.into_iter().chain(windows.get_mut(id)) {
            open.stamp.record(content.clone());
        }
    });
}

/// Asks every open surface not already showing `content` to build again — what a config reload does to this registry — and stamps each with it.
///
/// Nothing is closed and nothing is reopened: each surface stays where it is, at the size it is, and builds its content from the config as it now stands. What the user was in the middle of survives because it is not in the tree being replaced — a panel keeps its search, its page and its transition in [`util::state`], which belongs to the surface rather than to any one build of it.
///
/// [`Reload::Always`] passes nobody by. It is a reload somebody asked for — `hogar-shell shell reload`, a palette that moved under an unchanged file — and what it exists to deliver is exactly what a fingerprint of the config cannot see.
pub fn rebuild_all(content: &Fingerprint, reload: Reload) {
    OPEN.with(|surfaces| {
        let mut surfaces = surfaces.borrow_mut();
        let OpenSurfaces { drawer, windows } = &mut *surfaces;
        let open = drawer
            .iter_mut()
            .map(|(_, open)| open)
            .chain(windows.values_mut());
        for open in open {
            if open.stamp.needs(reload, content) {
                open.token.rebuild();
            }
            open.stamp.record(content.clone());
        }
    });
}

/// Drops every open surface. The shutdown path, and nothing else — a reload rebuilds them instead.
pub fn close_all() {
    OPEN.with(|surfaces| {
        let mut surfaces = surfaces.borrow_mut();
        surfaces.drawer = None;
        surfaces.windows.clear();
    });
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use config::Config;

    fn config_from(toml: &str) -> Arc<Config> {
        Arc::new(toml::from_str(toml).unwrap())
    }

    /// A token over a surface that was never opened, counting what it was asked to do: `open_surface` needs a driver, and these tests are about the registry's bookkeeping rather than about anything on screen.
    fn token() -> SurfaceToken {
        counted().0
    }

    fn counted() -> (SurfaceToken, std::rc::Rc<std::cell::Cell<u32>>) {
        struct Counting(std::rc::Rc<std::cell::Cell<u32>>);
        impl telar::SurfaceControl for Counting {
            fn close(&self) {}
            fn is_closing(&self) -> bool {
                false
            }
            fn rebuild(&self) {
                self.0.set(self.0.get() + 1);
            }
        }
        let rebuilds = std::rc::Rc::new(std::cell::Cell::new(0));
        (
            SurfaceToken::new(Box::new(Counting(std::rc::Rc::clone(&rebuilds)))),
            rebuilds,
        )
    }

    /// Closing a panel releases it rather than hiding it.
    ///
    /// The distinction the residency rule turns on, and the one a surface cannot show from outside: a panel put away with its tree still built and its subscriptions still live looks exactly like one that is gone. The token is the ownership, so the only honest evidence is that dropping it out of the registry is what runs its teardown — counted here from the token itself rather than from anything that tracks it.
    #[test]
    fn closing_a_panel_releases_its_surface_rather_than_hiding_it() {
        struct Dropping(std::rc::Rc<std::cell::Cell<u32>>);
        impl telar::SurfaceControl for Dropping {
            fn close(&self) {}
            fn is_closing(&self) -> bool {
                false
            }
            fn rebuild(&self) {}
        }
        impl Drop for Dropping {
            fn drop(&mut self) {
                self.0.set(self.0.get() + 1);
            }
        }

        let released = std::rc::Rc::new(std::cell::Cell::new(0));
        let token = SurfaceToken::new(Box::new(Dropping(std::rc::Rc::clone(&released))));
        OPEN.with(|surfaces| {
            surfaces
                .borrow_mut()
                .windows
                .insert("mixer".to_string(), token.into())
        });

        assert_eq!(released.get(), 0, "an open panel is still held");
        close("mixer");
        assert_eq!(
            released.get(),
            1,
            "a closed panel is dropped, not parked in the registry"
        );
        assert!(open_ids().is_empty());
    }

    /// **A standing window takes the screen from the drawer, and from nothing else.**
    ///
    /// A drawer's surface covers the whole usable area — that is how a press beside it dismisses it — so a window opening under one is painted, unreachable, and dismissed rather than used by the first press near it. Everything else was opened deliberately: the float the user parked, the notification centre they are working through, the popout the pointer owns.
    #[test]
    fn a_standing_window_closes_the_drawer_and_leaves_every_other_window_up() {
        OPEN.with(|surfaces| {
            let mut surfaces = surfaces.borrow_mut();
            surfaces.drawer = Some(("network".to_string(), token().into()));
            surfaces.windows.insert("mixer".to_string(), token().into());
            surfaces
                .windows
                .insert("sidebar".to_string(), token().into());
            surfaces
                .windows
                .insert("popout".to_string(), token().into());
        });

        toggle_standing_window("launcher", token);

        assert_eq!(open_ids(), vec!["launcher", "mixer", "popout", "sidebar"]);
    }

    /// The same door for a float and for the notification centre, and neither takes the other away: two of them overlap, and both can be closed by hand, which is the trade the user made by opening the second.
    #[test]
    fn two_standing_windows_stay_up_together() {
        OPEN.with(|surfaces| {
            surfaces.borrow_mut().drawer = Some(("network".to_string(), token().into()));
        });

        toggle_standing_window("sidebar", token);
        toggle_standing_window("settings", token);
        assert_eq!(open_ids(), vec!["settings", "sidebar"]);

        // And closing one is only that: a second press on the chip puts that window away and takes nothing with it — not even a drawer opened since.
        OPEN.with(|surfaces| {
            surfaces.borrow_mut().drawer = Some(("network".to_string(), token().into()));
        });
        toggle_standing_window("settings", token);
        assert_eq!(open_ids(), vec!["network", "sidebar"]);
    }

    #[test]
    fn env_anchors_a_panel_to_the_bar_its_module_sits_on() {
        config::set_config(config_from(
            "[bars.top]\ncenter=[\"clock\"]\n[bars.left]\nsize=48\nstart=[\"battery\"]\n",
        ));
        let battery = env_for_module("battery").expect("context is set");
        assert_eq!(battery.edge, Edge::Left, "follows the bar the module is on");
        assert_eq!(battery.bar_size, 48, "and that bar's thickness");

        let stray = env_for_module("notes").expect("context is set");
        assert_eq!(
            stray.edge,
            Edge::Top,
            "a module on no bar still opens somewhere sensible"
        );
    }

    const SAVED: &str = "[clock]\nformat = \"%H:%M\"\n";
    const TYPED: &str = "[clock]\nformat = \"%H:%M:%S\"\n";

    fn scratch_config(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("hogar-shell-reload-{name}-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(&path, SAVED).unwrap();
        path
    }

    /// The settings window, a float and a drawer, each counting its rebuilds, stamped with `content` as a reload of it would leave them.
    fn open_three(content: &Fingerprint) -> [std::rc::Rc<std::cell::Cell<u32>>; 3] {
        let (settings, settings_rebuilds) = counted();
        let (launcher, launcher_rebuilds) = counted();
        let (drawer, drawer_rebuilds) = counted();
        OPEN.with(|surfaces| {
            let mut surfaces = surfaces.borrow_mut();
            surfaces.drawer = Some(("clock".to_string(), drawer.into()));
            surfaces
                .windows
                .insert("settings".to_string(), settings.into());
            surfaces
                .windows
                .insert("launcher".to_string(), launcher.into());
        });
        rebuild_all(content, Reload::Always);
        let rebuilds = [settings_rebuilds, launcher_rebuilds, drawer_rebuilds];
        for counter in &rebuilds {
            counter.set(0);
        }
        rebuilds
    }

    /// **A settings write reaches every surface but the window that made it — and it does reach them.**
    ///
    /// Nothing applies a settings change to the running shell but the file: it is the only way the change reaches the bars, so the reload it causes has to run. What the window is spared is the rebuild — it already shows what it wrote, and rebuilding it would put the caret back at the start of the field being typed into.
    #[test]
    fn a_settings_write_reloads_every_other_surface_and_not_the_window_that_wrote_it() {
        let path = scratch_config("own-write");
        let mut applied = Stamp::default();
        applied.record(Fingerprint::read(&path));
        let [settings, launcher, drawer] = open_three(&Fingerprint::read(&path));

        util::writer::write(&path, TYPED.as_bytes().to_vec()).unwrap();
        stamp("settings", Fingerprint::with_config(&path, Some(TYPED)));

        let seen = Fingerprint::read(&path);
        assert!(
            applied.needs(Reload::IfChanged, &seen),
            "the reload runs: nothing applies a settings change in-process, so skipping it would leave the bars on the old config"
        );
        rebuild_all(&seen, Reload::IfChanged);
        assert_eq!(
            settings.get(),
            0,
            "the window that wrote the change is already showing it"
        );
        assert_eq!(
            (launcher.get(), drawer.get()),
            (1, 1),
            "every other surface takes the new config"
        );
        close_all();
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    /// **An edit landing between a settings write and its reload reaches the settings window too** — the case the authorship flag got wrong. The flag was spent on whichever reload came next and could not tell the window's bytes from anyone else's, so an edit arriving behind a save was suppressed in exactly the window that needed to show it. A stamp of content cannot be fooled that way: the reload carries bytes the window never wrote.
    #[test]
    fn an_edit_landing_between_a_settings_write_and_its_reload_rebuilds_the_settings_window() {
        let path = scratch_config("edit-behind");
        let [settings, ..] = open_three(&Fingerprint::read(&path));

        util::writer::write(&path, TYPED.as_bytes().to_vec()).unwrap();
        stamp("settings", Fingerprint::with_config(&path, Some(TYPED)));
        std::fs::write(&path, "[clock]\nformat = \"%I:%M\"\n").unwrap();

        let seen = Fingerprint::read(&path);
        rebuild_all(&seen, Reload::IfChanged);
        assert_eq!(
            settings.get(),
            1,
            "the reload carries an edit the window never made, so it is rebuilt to show it"
        );

        rebuild_all(&seen, Reload::IfChanged);
        assert_eq!(
            settings.get(),
            1,
            "and the rebuild stamps it, so the same content does not rebuild it twice"
        );
        close_all();
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    /// A reload somebody asked for rebuilds every surface, the one already showing the file included: `hogar-shell shell reload` and a palette that moved under an unchanged file exist to deliver what no fingerprint of the config covers, so a stamp that matches it says nothing about whether the surface is current.
    #[test]
    fn a_requested_reload_rebuilds_even_the_window_already_showing_the_file() {
        let path = scratch_config("requested");
        let [settings, launcher, drawer] = open_three(&Fingerprint::read(&path));
        let content = Fingerprint::read(&path);
        stamp("settings", content.clone());

        rebuild_all(&content, Reload::Always);
        assert_eq!(
            (settings.get(), launcher.get(), drawer.get()),
            (1, 1, 1),
            "an asked-for reload passes nobody by"
        );
        close_all();
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    /// Shutting down takes everything with it, whatever kind of surface it is.
    #[test]
    fn quitting_closes_every_surface() {
        OPEN.with(|surfaces| {
            let mut surfaces = surfaces.borrow_mut();
            surfaces.drawer = Some(("clock".to_string(), token().into()));
            surfaces
                .windows
                .insert("settings".to_string(), token().into());
            surfaces
                .windows
                .insert("launcher".to_string(), token().into());
        });

        close_all();

        assert!(open_ids().is_empty());
    }

    #[test]
    fn env_without_a_context_is_none() {
        config::clear_config();
        assert!(
            env_for_module("clock").is_none(),
            "no config means nothing to open against"
        );
    }
}
