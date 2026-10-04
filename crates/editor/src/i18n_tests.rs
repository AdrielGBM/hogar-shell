//! That the editor's catalogue resolves, that switching the locale changes what it answers, and that it holds exactly the messages the editor asks for, in every language.

#[cfg(test)]
mod tests {
    use std::path::Path;

    #[test]
    fn catalog_translates_and_switches() {
        telar::set_locale("en");
        assert_eq!(telar::t!("editor.done"), "Done");
        assert_eq!(crate::mode::name_of(layout::LayerKind::Lock), "Lock screen");
        telar::set_locale("es");
        assert_eq!(telar::t!("editor.done"), "Listo");
        assert_eq!(
            crate::mode::name_of(layout::LayerKind::Lock),
            "Pantalla de bloqueo"
        );
        telar::set_locale("en");
    }

    /// A count is said in the plural its language gives it.
    #[test]
    fn a_count_takes_its_languages_plural() {
        let said =
            |count: usize| telar::t!("editor.palette.komponent", name = "pill", count = count);
        telar::set_locale("en");
        assert_eq!(said(1), "pill · 1 parameter");
        assert_eq!(said(2), "pill · 2 parameters");
        assert_eq!(said(0), "pill · 0 parameters");
        telar::set_locale("es");
        assert_eq!(said(1), "pill · 1 parámetro");
        assert_eq!(said(3), "pill · 3 parámetros");
        telar::set_locale("en");
    }

    fn sources(dir: &Path, into: &mut String) {
        for entry in std::fs::read_dir(dir).expect("the sources") {
            let path = entry.expect("an entry").path();
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("");
            if path.is_dir() {
                if name != ".telar" && name != "i18n" {
                    sources(&path, into);
                }
            } else if name.ends_with(".rs") || name.ends_with(".rsx") {
                into.push_str(&std::fs::read_to_string(&path).expect("a source"));
            }
        }
    }

    /// Every message is in every language, and every one is asked for by name — but an action's, which is asked for by the action's id ([`crate::context::action_label`]): a message nothing reads is a label left behind by a row that went. The messages are the catalogue as telar bakes it, where a plural (`one`, `other`, … under one key) is one message asked for by that key, its categories each language's own.
    #[test]
    fn the_catalogue_holds_what_the_editor_asks_for_in_every_language() {
        let catalog = &crate::__rsx_i18n::CATALOG;
        let untranslated: Vec<(&str, &str)> = catalog
            .entries
            .iter()
            .flat_map(|entry| {
                catalog
                    .locales
                    .iter()
                    .filter(|locale| !entry.messages.iter().any(|(held, _)| held == *locale))
                    .map(|locale| (entry.key, *locale))
            })
            .collect();
        assert!(
            untranslated.is_empty(),
            "not in every language: {untranslated:?}"
        );
        let mut said = String::new();
        sources(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
            &mut said,
        );
        let unread: Vec<&str> = catalog
            .entries
            .iter()
            .map(|entry| entry.key)
            .filter(|key| !key.starts_with("editor.action."))
            .filter(|key| !said.contains(&format!("\"{key}\"")))
            .collect();
        assert!(unread.is_empty(), "nothing asks for {unread:?}");
    }
}
