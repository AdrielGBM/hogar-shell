#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use crate::bundle;
    use crate::templates::{self, TemplateError};
    use crate::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = util::paths::isolated_root()
            .expect("a test resolves under its scratch root")
            .join(format!("template-tests-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        dir
    }

    fn store_of(name: &str, mine: &Layout) -> (LayoutStore, PathBuf) {
        let dir = scratch(name);
        std::fs::write(
            dir.join("mine.toml"),
            toml::to_string_pretty(mine).expect("the layout serializes"),
        )
        .expect("a layout of the user's own");
        let (mut store, report) = LayoutStore::load(&dir);
        assert!(report.is_clean(), "{}", report.render());
        store.use_layout(&LayoutId::new("mine")).expect("mine");
        (store, dir)
    }

    #[test]
    fn every_shipped_template_reads_as_a_bundle_of_one_layout_named_after_it() {
        let shipped = templates::shipped();
        assert!(!shipped.is_empty());
        for (name, read) in shipped {
            let template = read.unwrap_or_else(|report| panic!("{name}: {}", report.render()));
            assert_eq!(template.name(), name);
            assert!(template.layout().is_some(), "{name} holds its own layout");
            assert_eq!(template.bundle.layouts.len(), 1, "{name}");
            assert!(template.bundle.komponents.is_empty(), "{name}");
            assert!(template.bundle.assets.is_empty(), "{name}");
            assert_eq!(
                template.bundle.manifest.description.as_deref(),
                Some(template.description.english().as_str()),
                "{name}: the manifest says what the gallery says"
            );
            for said in [&template.title, &template.description] {
                assert!(said.is_translated_in("es"), "{name}: {}", said.english());
            }
        }
    }

    #[test]
    fn the_showcase_resolves_cleanly_on_its_own() {
        let template = templates::named("showcase").expect("the showcase ships");
        let layout = template.layout().expect("its layout");
        assert!(layout.extends.is_none());
        let library = Library::of_layouts([built_in(), layout.clone()]);
        for output in [NOMINAL_OUTPUT, "DP-1", "HDMI-A-1"] {
            let (resolved, report) = resolve(layout, &library, output, None);
            assert!(report.is_clean(), "{output}: {}", report.render());
            let lock = &resolved.layer(LayerKind::Lock).expect("a lock layer").areas;
            assert!(
                lock.iter()
                    .any(|area| matches!(area.kind, ResolvedAreaKind::Prompt { .. })),
                "{output}: the lock keeps its prompt"
            );
        }
    }

    #[test]
    fn the_showcase_places_what_it_promises() {
        let template = templates::named("showcase").expect("the showcase ships");
        let layout = template.layout().expect("its layout");
        let library = Library::of_layouts([built_in(), layout.clone()]);
        let (resolved, _) = resolve(layout, &library, "DP-1", None);
        let modules = |layer: LayerKind| -> Vec<String> {
            resolved
                .areas()
                .filter(|(on, _)| *on == layer)
                .map(|(_, area)| area)
                .flat_map(|area| area.groups.iter())
                .flat_map(|group| group.children.iter())
                .map(|instance| instance.module.clone())
                .collect()
        };
        assert_eq!(
            modules(LayerKind::Desktop),
            ["weather", "media", "cpu", "clock", "visualiser"]
        );
        assert_eq!(
            modules(LayerKind::Top),
            [
                "workspaces",
                "windows",
                "clock",
                "notes",
                "tray",
                "network",
                "volume",
                "battery"
            ]
        );
        assert!(
            resolved
                .areas()
                .any(|(layer, area)| layer == LayerKind::Overlay
                    && matches!(area.kind, ResolvedAreaKind::Stack { .. }))
        );
        assert_eq!(
            modules(LayerKind::Lock),
            ["clock", "user", "media", "notifications"]
        );
    }

    #[test]
    fn using_a_template_adds_a_layout_and_leaves_the_current_one_alone() {
        let mut mine = built_in();
        mine.id = LayoutId::new("mine");
        let (mut store, dir) = store_of("use", &mine);
        let before = store.get(&LayoutId::new("mine")).cloned();

        let made = templates::put(&mut store, "showcase", None).expect("it is made");
        assert_eq!(made.as_str(), "showcase");
        let again = templates::put(&mut store, "showcase", None).expect("and again");
        assert_eq!(again.as_str(), "showcase-2", "a taken name is numbered");
        let named = templates::put(&mut store, "showcase", Some("work")).expect("named");
        assert_eq!(named.as_str(), "work");
        assert_eq!(store.get(&named).map(|it| it.name.as_str()), Some("work"));

        let shipped = templates::named("showcase").expect("the showcase ships");
        let copied = store.get(&made).cloned().expect("the copy is in the store");
        assert_eq!(
            copied.outputs,
            shipped.layout().expect("its layout").outputs
        );
        assert_eq!(store.get(&LayoutId::new("mine")).cloned(), before);
        assert_eq!(store.active_id().as_str(), "mine", "using is the caller's");
        assert!(
            store.history().is_empty(),
            "nothing to take back in any layout"
        );

        assert!(store.flush().is_clean());
        let written = std::fs::read_to_string(dir.join("showcase.toml")).expect("written");
        let read: Layout = toml::from_str(&written).expect("it parses back");
        assert_eq!(read.outputs, copied.outputs);
    }

    #[test]
    fn a_name_that_is_taken_or_cannot_name_a_file_is_refused() {
        let mut mine = built_in();
        mine.id = LayoutId::new("mine");
        let (_, dir) = store_of("refused", &mine);
        std::fs::write(dir.join("broken.toml"), "this is = = not toml").unwrap();
        let (mut store, report) = LayoutStore::load(&dir);
        assert!(!report.is_clean(), "the broken file is reported");

        let refused = |store: &mut LayoutStore, called: &str| {
            templates::put(store, "showcase", Some(called)).expect_err(called)
        };
        assert!(matches!(
            refused(&mut store, "mine"),
            TemplateError::Taken(_)
        ));
        assert!(matches!(
            refused(&mut store, BUILT_IN),
            TemplateError::Taken(_)
        ));
        assert!(
            matches!(refused(&mut store, "broken"), TemplateError::Taken(_)),
            "a file that does not parse is still the user's"
        );
        assert!(matches!(
            refused(&mut store, "Not a name"),
            TemplateError::BadName(_)
        ));
        let unknown = templates::put(&mut store, "nothing-like-it", None).expect_err("unknown");
        assert!(unknown.message().english().contains("`showcase`"));
        assert_eq!(
            refused(&mut store, "mine").message().english(),
            "a layout called `mine` exists already: give the new one another name"
        );
        assert!(store.get(&LayoutId::new("showcase")).is_none());
    }

    #[test]
    fn the_safe_store_takes_no_template() {
        let mut store = LayoutStore::safe(scratch("safe"));
        assert!(matches!(
            templates::put(&mut store, "showcase", None),
            Err(TemplateError::Store(StoreError::Safe))
        ));
    }

    #[test]
    fn a_bundle_held_in_memory_is_read_by_the_rules_a_directory_is() {
        let manifest = "name = \"mine\"\n";
        let picture = r#"
[[outputs]]
match = "*"

[[outputs.layers.background.areas]]
id = "background"
kind = "wallpaper_region"
source = "assets/sky.png"
"#;
        let refused = bundle::of_texts(Path::new("here"), manifest, &[("mine", picture)], &[])
            .expect_err("it carries no pictures");
        assert!(
            refused
                .errors
                .iter()
                .any(|finding| finding.key.ends_with("areas.background.source")),
            "{}",
            refused.render()
        );
        let refused = bundle::of_texts(Path::new("here"), manifest, &[(BUILT_IN, "")], &[])
            .expect_err("the built-in name");
        assert!(
            refused.render().contains("built-in"),
            "{}",
            refused.render()
        );
        let refused = bundle::of_texts(Path::new("here"), manifest, &[("Mine", "")], &[])
            .expect_err("a name that is no file's");
        assert!(refused.render().contains("Mine"), "{}", refused.render());
        let read = bundle::of_texts(Path::new("here"), manifest, &[("mine", "")], &[])
            .expect("an empty layout reads");
        assert_eq!(read.layouts.len(), 1);
    }
}
