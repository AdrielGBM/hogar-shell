use std::path::PathBuf;

use config::{Config, DockConfig};
use platform_wayland::ManagedToplevel;
use services::apps::App;

pub const MOST_DOTS: usize = 3;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum SlotKey {
    Pinned(String),
    Running(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Slot {
    pub key: SlotKey,
    pub entry: Option<App>,
    pub windows: Vec<ManagedToplevel>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Press {
    Launch(App),
    Activate(ManagedToplevel),
    Nothing,
}

impl SlotKey {
    pub fn is_pinned(&self) -> bool {
        matches!(self, SlotKey::Pinned(_))
    }
}

impl Slot {
    pub fn is_focused(&self) -> bool {
        self.windows.iter().any(|window| window.activated)
    }

    pub fn dots(&self) -> usize {
        self.windows.len().min(MOST_DOTS)
    }

    pub fn pin_id(&self) -> String {
        match (&self.key, &self.entry) {
            (SlotKey::Pinned(id), _) => id.clone(),
            (SlotKey::Running(_), Some(entry)) => entry.id.clone(),
            (SlotKey::Running(app), None) => app.clone(),
        }
    }

    pub fn icon(&self) -> String {
        match (&self.entry, self.windows.first()) {
            (Some(entry), _) if !entry.icon.is_empty() => entry.icon.clone(),
            (_, Some(window)) => window.app_id.clone(),
            _ => self.pin_id(),
        }
    }

    pub fn press(&self) -> Press {
        if self.windows.is_empty() {
            return self.entry.clone().map_or(Press::Nothing, Press::Launch);
        }
        let next = self
            .windows
            .iter()
            .position(|window| window.activated)
            .map_or(0, |focused| (focused + 1) % self.windows.len());
        Press::Activate(self.windows[next].clone())
    }
}

pub fn slots(pinned: &[String], apps: &[App], windows: &[ManagedToplevel]) -> Vec<Slot> {
    let mut left: Vec<&ManagedToplevel> = windows.iter().collect();
    let mut slots: Vec<Slot> = pinned
        .iter()
        .map(|id| {
            let entry = apps
                .iter()
                .find(|app| app.id.eq_ignore_ascii_case(id))
                .cloned();
            let owns = |window: &&ManagedToplevel| match &entry {
                Some(entry) => entry.owns_window(&window.app_id),
                None => window.app_id.eq_ignore_ascii_case(id),
            };
            let (own, rest): (Vec<_>, Vec<_>) = left.iter().copied().partition(owns);
            left = rest;
            Slot {
                key: SlotKey::Pinned(id.clone()),
                entry,
                windows: own.into_iter().cloned().collect(),
            }
        })
        .collect();
    for window in left {
        match slots
            .iter_mut()
            .find(|slot| slot.key == SlotKey::Running(window.app_id.clone()))
        {
            Some(slot) => slot.windows.push(window.clone()),
            None => slots.push(Slot {
                key: SlotKey::Running(window.app_id.clone()),
                entry: apps
                    .iter()
                    .find(|app| app.owns_window(&window.app_id))
                    .cloned(),
                windows: vec![window.clone()],
            }),
        }
    }
    slots
}

pub fn toggled(pinned: &[String], id: &str) -> Vec<String> {
    match pinned.iter().any(|pin| pin.eq_ignore_ascii_case(id)) {
        true => pinned
            .iter()
            .filter(|pin| !pin.eq_ignore_ascii_case(id))
            .cloned()
            .collect(),
        false => pinned.iter().cloned().chain([id.to_string()]).collect(),
    }
}

pub fn moved(pinned: &[String], dragged: &str, onto: &str) -> Vec<String> {
    let (Some(from), Some(to)) = (
        pinned
            .iter()
            .position(|pin| pin.eq_ignore_ascii_case(dragged)),
        pinned.iter().position(|pin| pin.eq_ignore_ascii_case(onto)),
    ) else {
        return pinned.to_vec();
    };
    let mut pinned = pinned.to_vec();
    let pin = pinned.remove(from);
    pinned.insert(to, pin);
    pinned
}

#[derive(Clone, Debug)]
pub struct PinFile {
    path: PathBuf,
}

impl PinFile {
    pub fn at(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn user() -> Self {
        Self::at(Config::default_path())
    }

    pub fn save(&self, pinned: &[String]) {
        let dock = DockConfig {
            pinned: pinned.to_vec(),
            ..Config::load_or_default(&self.path).dock
        };
        if let Err(e) = Config::save_section(&self.path, "dock", &dock) {
            tracing::warn!("dock: could not save the pins: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use platform_wayland::ManagedToplevelId;

    use super::*;

    fn app(id: &str, class: &str) -> App {
        App {
            id: id.to_string(),
            name: id.to_string(),
            exec: id.to_string(),
            icon: format!("{id}-icon"),
            wm_class: class.to_string(),
            ..App::default()
        }
    }

    fn window(id: u32, app: &str, activated: bool) -> ManagedToplevel {
        ManagedToplevel {
            id: ManagedToplevelId::from_raw(id),
            app_id: app.to_string(),
            title: format!("{app} {id}"),
            activated,
            ..ManagedToplevel::default()
        }
    }

    fn pins(ids: &[&str]) -> Vec<String> {
        ids.iter().map(|id| id.to_string()).collect()
    }

    fn keys(slots: &[Slot]) -> Vec<(bool, String, usize)> {
        slots
            .iter()
            .map(|slot| (slot.key.is_pinned(), slot.pin_id(), slot.windows.len()))
            .collect()
    }

    #[test]
    fn pins_come_first_in_their_order_holding_their_windows_and_the_rest_follow_by_application() {
        let apps = [app("firefox", ""), app("code", "Code"), app("kitty", "")];
        let open = [
            window(1, "kitty", false),
            window(2, "Code", true),
            window(3, "kitty", false),
            window(4, "gimp", false),
        ];
        let slots = slots(&pins(&["firefox", "code"]), &apps, &open);
        assert_eq!(
            keys(&slots),
            vec![
                (true, "firefox".to_string(), 0),
                (true, "code".to_string(), 1),
                (false, "kitty".to_string(), 2),
                (false, "gimp".to_string(), 1),
            ]
        );
        assert!(slots[1].is_focused());
        assert_eq!(slots[2].icon(), "kitty-icon");
        assert_eq!(
            slots[3].icon(),
            "gimp",
            "no entry, so the window names its icon"
        );
    }

    #[test]
    fn a_pin_whose_entry_is_gone_still_holds_its_place_and_its_windows() {
        let slots = slots(&pins(&["vanished"]), &[], &[window(1, "Vanished", false)]);
        assert_eq!(keys(&slots), vec![(true, "vanished".to_string(), 1)]);
        assert_eq!(
            slots[0].press(),
            Press::Activate(window(1, "Vanished", false))
        );
    }

    #[test]
    fn indicators_count_the_windows_up_to_three() {
        let apps = [app("kitty", "")];
        let open: Vec<_> = (1..=5).map(|id| window(id, "kitty", false)).collect();
        for count in 0..=5 {
            let slots = slots(&pins(&["kitty"]), &apps, &open[..count]);
            assert_eq!(slots[0].dots(), count.min(MOST_DOTS), "{count} windows");
        }
    }

    #[test]
    fn a_press_launches_what_is_not_running_and_otherwise_steps_through_its_windows() {
        let apps = [app("kitty", "")];
        let idle = slots(&pins(&["kitty"]), &apps, &[]);
        assert_eq!(idle[0].press(), Press::Launch(app("kitty", "")));

        let open = [window(1, "kitty", false), window(2, "kitty", true)];
        let running = slots(&pins(&["kitty"]), &apps, &open);
        assert_eq!(
            running[0].press(),
            Press::Activate(open[0].clone()),
            "after the focused one, wrapping"
        );

        let unfocused = [window(1, "kitty", false), window(2, "kitty", false)];
        let running = slots(&pins(&["kitty"]), &apps, &unfocused);
        assert_eq!(running[0].press(), Press::Activate(unfocused[0].clone()));

        assert_eq!(
            slots(&pins(&["gone"]), &apps, &[])[0].press(),
            Press::Nothing
        );
    }

    #[test]
    fn pinning_appends_unpinning_removes_and_a_drag_moves_a_pin_onto_another() {
        assert_eq!(toggled(&pins(&["a", "b"]), "c"), pins(&["a", "b", "c"]));
        assert_eq!(toggled(&pins(&["a", "b", "c"]), "b"), pins(&["a", "c"]));
        assert_eq!(
            moved(&pins(&["a", "b", "c"]), "a", "c"),
            pins(&["b", "c", "a"])
        );
        assert_eq!(
            moved(&pins(&["a", "b", "c"]), "c", "a"),
            pins(&["c", "a", "b"])
        );
        assert_eq!(moved(&pins(&["a", "b"]), "x", "a"), pins(&["a", "b"]));
    }

    #[test]
    fn a_pin_is_found_unpinned_and_moved_whatever_its_case() {
        let apps = [app("org.gnome.Nautilus", "")];
        let slots = slots(
            &pins(&["org.gnome.nautilus"]),
            &apps,
            &[window(1, "org.gnome.Nautilus", false)],
        );
        assert_eq!(
            keys(&slots),
            vec![(true, "org.gnome.nautilus".to_string(), 1)],
            "the pin holds its entry's window rather than leaving it to a second slot"
        );
        assert_eq!(slots[0].entry, Some(apps[0].clone()));
        assert_eq!(
            toggled(&pins(&["Firefox", "kitty"]), "firefox"),
            pins(&["kitty"])
        );
        assert_eq!(
            moved(&pins(&["Firefox", "kitty"]), "KITTY", "firefox"),
            pins(&["kitty", "Firefox"])
        );
    }

    #[test]
    fn a_running_entry_is_pinned_by_its_desktop_entry_where_it_has_one() {
        let apps = [app("org.mozilla.firefox", "")];
        let slots = slots(
            &[],
            &apps,
            &[window(1, "firefox", false), window(2, "xterm", false)],
        );
        assert_eq!(slots[0].pin_id(), "org.mozilla.firefox");
        assert_eq!(slots[1].pin_id(), "xterm");
    }

    #[test]
    fn the_pins_are_written_to_the_dock_section_and_the_rest_of_the_file_is_kept() {
        let root = util::paths::isolated_root().expect("a test writes only its own scratch tree");
        let path = root.join("pins").join("config.toml");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            "# mine\n[dock]\nmagnification = 2.0\n\n[clock]\nshow_date = false\n",
        )
        .unwrap();
        let file = PinFile::at(&path);
        file.save(&pins(&["firefox", "kitty"]));
        assert!(
            config::fingerprint::written_by_shell(&config::fingerprint::Fingerprint::read(&path)),
            "the reload a pin brings back is the shell's own write, applied without a toast"
        );
        let written = std::fs::read_to_string(&path).unwrap();
        assert!(written.starts_with("# mine"), "{written}");
        let config = Config::load(&path).expect("it parses back");
        assert_eq!(config.dock.pinned, pins(&["firefox", "kitty"]));
        assert_eq!(config.dock.magnification, 2.0);
        assert!(!config.clock.show_date);
    }
}
