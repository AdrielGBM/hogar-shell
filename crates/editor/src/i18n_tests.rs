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

    fn keys_of(table: &toml::Table, prefix: &str, into: &mut Vec<String>) {
        for (name, value) in table {
            let key = match prefix.is_empty() {
                true => name.clone(),
                false => format!("{prefix}.{name}"),
            };
            match value {
                toml::Value::Table(inner) => keys_of(inner, &key, into),
                _ => into.push(key),
            }
        }
    }

    fn catalogue(language: &str) -> Vec<String> {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("src/i18n/{language}.toml"));
        let text = std::fs::read_to_string(path).expect("the catalogue");
        let table: toml::Table = toml::from_str(&text).expect("it parses");
        let mut keys = Vec::new();
        keys_of(&table, "", &mut keys);
        keys.sort();
        keys
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

    /// Every message is in every language, and every one is asked for by name — but an action's, which is asked for by the action's id ([`crate::context::action_label`]): a message nothing reads is a label left behind by a row that went.
    #[test]
    fn the_catalogue_holds_what_the_editor_asks_for_in_every_language() {
        let english = catalogue("en");
        assert_eq!(catalogue("es"), english, "the same messages in Spanish");
        let mut said = String::new();
        sources(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
            &mut said,
        );
        let unread: Vec<&String> = english
            .iter()
            .filter(|key| !key.starts_with("editor.action."))
            .filter(|key| !said.contains(&format!("\"{key}\"")))
            .collect();
        assert!(unread.is_empty(), "nothing asks for {unread:?}");
    }
}
