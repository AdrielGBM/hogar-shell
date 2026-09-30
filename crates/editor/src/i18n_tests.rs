//! That the editor's catalogue resolves, and that switching the locale changes what it answers.

#[cfg(test)]
mod tests {
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
}
