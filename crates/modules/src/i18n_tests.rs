//! That the modules' baked catalogs resolve, and that switching the locale changes what they answer.
//!
//! One test rather than one per module: the catalogs are scanned and merged per crate, so a key that resolves here proves the whole scan — and the same reactive `t!` calls back every label, so a live locale switch re-renders them.

#[cfg(test)]
mod tests {
    #[test]
    fn catalog_translates_and_switches() {
        telar::set_locale("en");
        // The crate's own catalog, and a per-module one — the scan merges every `i18n/` under `src`, so one of each is what proves the whole tree was picked up rather than only the root.
        assert_eq!(telar::t!("common.on"), "On");
        assert_eq!(telar::t!("battery.remaining", time = "5m"), "5m remaining");
        telar::set_locale("es");
        assert_eq!(telar::t!("common.on"), "Sí");
        assert_eq!(telar::t!("battery.remaining", time = "5m"), "5m restante");
    }
}
