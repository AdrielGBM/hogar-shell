//! Which settings live on which page, and how a search finds them.
//!
//! Each form owns one `[toml]` section and saves it on its own, whether it is an `.rsx` component or one of
//! the handful still written in Rust. What this file adds is the *shape* of the application over them: a page
//! is a nav entry and the ordered list of sections it shows, so grouping is a table rather than the order of a
//! forty-item `Vec`.
//!
//! **Search is answered from the schema, not from the widgets.** Every field a form draws is a key on a config
//! struct, and `build.rs` already lifts the doc comment off each one for `hogar-shell config schema`. Matching a
//! query against *that* means the search finds `beat_sensitivity` — a key whose label says "Beat sensitivity"
//! and whose explanation says "how far above its recent average the bass has to jump" — without every field
//! having to register itself twice, and without the index going stale when a form gains a row.

use config::schema;

/// A catalogue lookup for a key assembled at runtime.
///
/// `t!` validates its key while it compiles, which it can only do for a literal — and the nav's labels come out
/// of a table. The check is moved rather than lost: `every_label_has_a_translation` asks the catalogue for all
/// of them, and a missing one fails the suite instead of showing a user a raw key.
pub fn label(prefix: &str, name: &str) -> String {
    telar::i18n::translate(
        &crate::__rsx_i18n::CATALOG,
        &format!("{prefix}.{name}"),
        &[],
    )
}

/// A section's builder. It takes nothing: the file a form edits is ambient (`form::source`), and so is the
/// theme it draws in — they were parameters only because the panel had them in hand when it called down, and
/// carrying them made every section a shape no `.rsx` component can have.
pub type Build = fn() -> Result<Box<dyn telar::LayoutItem>, telar::LayoutError>;

/// One form on a page.
pub struct Section {
    /// Key under `settings.section`, which is also the heading the form draws for itself.
    pub label: &'static str,
    /// The `[toml]` sections this form edits. Drives search, and is what makes a form findable by the name of
    /// a key rather than only by the words on its own label.
    pub keys: &'static [&'static str],
    pub build: Build,
}

/// One nav entry.
pub struct Page {
    /// Key under `settings.page`.
    pub label: &'static str,
    /// Iconify name for the nav row.
    pub icon: &'static str,
    pub sections: &'static [Section],
}

impl Page {
    /// Whether anything on this page answers `query` — what dims a nav entry during a search rather than
    /// letting a user click through to a page with nothing on it.
    pub fn matches(&self, query: &str) -> bool {
        let query = query.trim().to_lowercase();
        query.is_empty()
            || label("settings.page", self.label)
                .to_lowercase()
                .contains(&query)
            || self.sections.iter().any(|section| section.matches(&query))
    }
}

impl Section {
    /// `query` is already trimmed and lowercased.
    fn matches(&self, query: &str) -> bool {
        if label("settings.section", self.label)
            .to_lowercase()
            .contains(query)
        {
            return true;
        }
        self.keys
            .iter()
            .any(|key| key.contains(query) || schema::section_mentions(key, query))
    }
}

/// Which forms the page area shows.
///
/// A search deliberately leaves the nav behind and looks everywhere. The alternative — narrowing only the
/// selected page — makes a user who types `beat` and is on the wrong page see nothing at all, and there is no
/// way for them to tell that from "no such setting". Searching is asking the application a question; the nav
/// is for browsing it, and it stays lit so they can see where the answers live.
pub fn visible(selected: usize, query: &str) -> Vec<&'static Section> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return page(selected).sections.iter().collect();
    }
    PAGES
        .iter()
        .flat_map(|page| page.sections.iter())
        .filter(|section| section.matches(&query))
        .collect()
}

macro_rules! section {
    ($label:literal, [$($key:literal),* $(,)?], $build:path) => {
        Section {
            label: $label,
            keys: &[$($key),*],
            build: $build,
        }
    };
}

/// The nav, in the order it is drawn.
///
/// The order is deliberate rather than alphabetical: the first four pages are what a user opens the settings
/// *for* — how it looks, where the bars are, and the two devices whose panels they already know — and the ones
/// they will each visit once come after.
pub const PAGES: &[Page] = &[
    Page {
        label: "appearance",
        icon: "palette",
        sections: &[
            section!(
                "theme",
                ["theme"],
                crate::sections::appearance::theme_section
            ),
            section!(
                "theme_colors",
                ["theme"],
                crate::sections::appearance::theme_colors_section
            ),
            section!("shape", ["shape"], crate::shape),
            section!("corners", ["corners"], crate::corners),
            section!("icons", ["icons"], crate::icons),
            section!("animation", ["animation"], crate::animation),
        ],
    },
    Page {
        label: "bars",
        icon: "layout-panel-top",
        sections: &[
            section!("bars", ["bars"], crate::sections::bars::bars_section),
            section!(
                "modules",
                ["modules"],
                crate::sections::bars::module_overrides_section
            ),
            section!("panels", ["panels"], crate::panels),
            section!("popouts", ["popouts"], crate::popouts),
            section!("stack", ["stack"], crate::stack),
            section!("clock", ["clock"], crate::clock),
            section!("active_window", ["active_window"], crate::active_window),
            section!("workspaces", ["workspaces"], crate::workspaces),
            section!("status_icons", ["status_icons"], crate::status_icons),
            section!("tray", ["tray"], crate::tray),
            section!("battery", ["battery"], crate::battery),
            section!(
                "battery_warnings",
                ["battery"],
                crate::sections::bars::battery_warnings_section
            ),
            section!("lock_status", ["lock_status"], crate::lock_status),
            section!("temperature", ["temperature"], crate::temperature),
        ],
    },
    Page {
        label: "audio",
        icon: "volume-2",
        sections: &[
            section!("audio", ["audio"], crate::audio),
            section!("visualiser", ["visualiser"], crate::visualiser),
            section!("media", ["media"], crate::media),
            section!(
                "media_aliases",
                ["media"],
                crate::sections::audio::media_aliases_section
            ),
            section!("lyrics", ["lyrics"], crate::lyrics),
        ],
    },
    Page {
        label: "network",
        icon: "wifi",
        sections: &[section!("network", ["network"], crate::network)],
    },
    Page {
        label: "bluetooth",
        icon: "bluetooth",
        sections: &[section!("bluetooth", ["bluetooth"], crate::bluetooth)],
    },
    Page {
        label: "applications",
        icon: "layout-grid",
        sections: &[
            section!(
                "apps",
                ["launcher"],
                crate::sections::applications::apps_section
            ),
            section!("launcher", ["launcher"], crate::launcher),
        ],
    },
    Page {
        label: "notifications",
        icon: "bell",
        sections: &[
            section!("notifications", ["notifications"], crate::notifications),
            section!("toasts", ["toasts"], crate::toasts),
            section!("sidebar", ["sidebar"], crate::sidebar),
        ],
    },
    Page {
        label: "lock",
        icon: "lock",
        sections: &[
            section!("lock", ["lock"], crate::lock),
            section!("idle", ["idle"], crate::idle),
            section!(
                "idle_stages",
                ["idle"],
                crate::sections::lock::idle_stages_section
            ),
        ],
    },
    Page {
        label: "wallpaper",
        icon: "image",
        sections: &[
            section!(
                "library",
                ["wallpaper"],
                crate::sections::wallpaper::wallpaper_browser_section
            ),
            section!(
                "background",
                ["background"],
                crate::sections::wallpaper::background_section
            ),
            section!("wallpaper", ["wallpaper"], crate::wallpaper),
            section!("desktop_clock", ["widgets"], crate::desktop_clock),
            section!("desktop_visualiser", ["widgets"], crate::desktop_visualiser),
        ],
    },
    Page {
        label: "language",
        icon: "languages",
        sections: &[
            section!("general", ["general"], crate::general),
            section!("dashboard", ["dashboard"], crate::dashboard),
        ],
    },
    Page {
        label: "services",
        icon: "server",
        sections: &[
            section!("weather", ["weather"], crate::weather),
            section!("gpu", ["gpu"], crate::gpu),
            section!("brightness", ["brightness"], crate::brightness),
            section!("paths", ["paths"], crate::paths),
            section!("screenshot", ["screenshot"], crate::screenshot),
            section!("recorder", ["recorder"], crate::recorder),
            section!("utilities", ["utilities"], crate::utilities),
            section!("keynav", ["keynav"], crate::keynav),
        ],
    },
    Page {
        label: "about",
        icon: "info",
        sections: &[
            section!("about", [], crate::about),
            section!(
                "dependencies",
                [],
                crate::sections::dependencies::dependencies_section
            ),
        ],
    },
];

/// The page at `index`, clamped — a stored selection outlives the page list it was made against.
pub fn page(index: usize) -> &'static Page {
    &PAGES[index.min(PAGES.len() - 1)]
}

#[cfg(test)]
mod tests {
    use super::*;
    use config::Config;

    #[test]
    fn every_config_section_is_reachable_from_some_page() {
        // The failure this catches is silent and permanent: a section added to `Config` with a form written for
        // it, and no nav entry, is a form no user can ever open. Nothing else notices — the shell builds, the
        // schema prints it, and the key simply cannot be edited.
        let defaults = toml::Value::try_from(Config::starter()).expect("serializes");
        let placed: Vec<&str> = PAGES
            .iter()
            .flat_map(|page| page.sections.iter().flat_map(|s| s.keys.iter().copied()))
            .collect();
        let missing: Vec<&str> = defaults
            .as_table()
            .expect("a table")
            .keys()
            // `version` is the schema's own bookkeeping, not a form.
            .filter(|key| key.as_str() != "version")
            .filter(|key| !placed.contains(&key.as_str()))
            .map(|key| key.as_str())
            .collect();
        assert!(
            missing.is_empty(),
            "config sections with no page: {missing:?}"
        );
    }

    #[test]
    fn a_page_id_and_a_section_label_are_each_used_once() {
        let mut ids: Vec<&str> = PAGES.iter().map(|page| page.label).collect();
        ids.sort_unstable();
        let unique = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), unique, "two pages share a label");

        let mut labels: Vec<&str> = PAGES
            .iter()
            .flat_map(|page| page.sections.iter().map(|s| s.label))
            .collect();
        labels.sort_unstable();
        let unique = labels.len();
        labels.dedup();
        assert_eq!(labels.len(), unique, "the same form is on two pages");
    }

    fn english() {
        services::locale::attach("en".to_string());
    }

    #[test]
    fn a_search_finds_a_form_by_a_key_it_does_not_display() {
        english();
        // The point of indexing the schema: `beat_sensitivity` is a field on the visualiser form, and nothing
        // about the words "Audio visualiser" would ever match it.
        let found = visible(0, "beat_sensitivity");
        assert_eq!(
            found.iter().map(|s| s.label).collect::<Vec<_>>(),
            vec!["visualiser"]
        );
        let audio = PAGES
            .iter()
            .find(|p| p.label == "audio")
            .expect("the audio page");
        assert!(audio.matches("beat_sensitivity"));
        assert!(
            !audio.matches("ddcutil"),
            "a key belonging to another page does not light this one up"
        );
    }

    #[test]
    fn a_search_leaves_the_selected_page_behind() {
        english();
        // A user who types `wifi` while sitting on Appearance has asked a question, not narrowed a page — and
        // an answer that depends on where they happened to be is indistinguishable from "no such setting".
        let from_appearance = visible(0, "ssid");
        let from_services = visible(PAGES.len() - 1, "ssid");
        assert!(!from_appearance.is_empty());
        assert_eq!(
            from_appearance.iter().map(|s| s.label).collect::<Vec<_>>(),
            from_services.iter().map(|s| s.label).collect::<Vec<_>>(),
        );
    }

    #[test]
    fn an_empty_query_shows_the_selected_pages_own_forms() {
        english();
        for (index, page) in PAGES.iter().enumerate() {
            assert!(page.matches(""));
            assert_eq!(visible(index, "").len(), page.sections.len());
            assert_eq!(visible(index, "   ").len(), page.sections.len());
        }
    }

    #[test]
    fn a_query_that_matches_nothing_shows_nothing_rather_than_everything() {
        english();
        assert!(visible(0, "zzzzz-no-such-setting").is_empty());
    }

    #[test]
    fn every_label_has_a_translation() {
        // The check `t!` would have made, moved: these keys are assembled at runtime from the table above, so
        // a page added without its catalogue entry would draw the literal `settings.page.foo` at a user.
        for locale in ["en", "es"] {
            services::locale::attach(locale.to_string());
            for page in PAGES {
                let key = format!("settings.page.{}", page.label);
                assert_ne!(label("settings.page", page.label), key, "{locale}: {key}");
                for section in page.sections {
                    let key = format!("settings.section.{}", section.label);
                    assert_ne!(
                        label("settings.section", section.label),
                        key,
                        "{locale}: {key}"
                    );
                }
            }
        }
        english();
    }

    #[test]
    fn a_selection_past_the_end_still_names_a_page() {
        assert_eq!(page(0).label, PAGES[0].label);
        assert_eq!(page(usize::MAX).label, PAGES[PAGES.len() - 1].label);
    }
}
