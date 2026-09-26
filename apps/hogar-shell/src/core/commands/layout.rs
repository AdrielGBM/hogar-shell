//! `hogar-shell layout` — reading the layouts on disk, and choosing which one the shell draws.
//!
//! `list`, `show` and `check` answer in the CLI process, because a layout that stops the shell from starting is exactly the one a user needs to be able to look at (F-10.5). `use` goes to the shell, which is what owns the store — it writes the active name to machine state and asks for a reload, and the shell reads that name back on its next pass.
//!
//! The verbs that *change* a layout — `edit`, `undo`, `redo`, `set`, `add`, `remove`, `move`, `reset` — are T-3.7 and are not here yet.

use layout::{LayoutId, LayoutStore, NOMINAL_OUTPUT, Representation};
use util::paths;

use super::{Command, Target};

pub(crate) const LAYOUT: Target = Target {
    name: "layout",
    commands: &[
        Command {
            name: "list",
            args: "",
            help: "every layout this shell can use",
            run: |_| Ok(list()),
        },
        Command {
            name: "show",
            args: "[name]",
            help: "print a layout as it is stored",
            run: |args| show(args.first().copied()),
        },
        Command {
            name: "check",
            args: "[name]",
            help: "report what is wrong with a layout, without applying it",
            run: |args| check(args.first().copied()),
        },
        Command {
            name: "use",
            args: "<name>",
            help: "draw this layout from now on",
            run: |args| use_layout(args.first().copied()),
        },
    ],
};

/// Makes `name` the layout the shell draws, and asks whatever is running to pick it up.
///
/// Which layout is active is machine state, not a config key (TA-2): it is a choice about this installation rather than a description of one. So this writes `state.json` and reloads, and the running shell reads the name back on the next pass — one direction, no second copy of the answer in the shell's memory to fall out of step.
fn use_layout(name: Option<&str>) -> Result<String, String> {
    let name = name.filter(|name| !name.trim().is_empty()).ok_or_else(|| {
        format!(
            "say which layout to use — `hogar-shell layout list` names them\n{}",
            list()
        )
    })?;
    let (store, _) = LayoutStore::load(layouts_dir());
    let id = LayoutId::new(name);
    if store.get(&id).is_none() {
        return Err(format!("there is no layout called `{name}`\n{}", list()));
    }
    services::state::update(|state| state.layout = Some(name.to_string()));
    config::request_reload();
    Ok(format!("drawing `{name}`"))
}

/// The layouts on disk, with the built-in one marked.
///
/// These three read files and answer from them, so they run in the CLI process the way `config check` does: a layout that stops the shell from starting is exactly the one a user needs to be able to look at.
fn list() -> String {
    let (store, report) = LayoutStore::load(layouts_dir());
    let mut lines: Vec<String> = store
        .names()
        .map(|id| match id.as_str() == layout::BUILT_IN {
            true => format!("{id}\tbuilt-in, read-only"),
            false => id.to_string(),
        })
        .collect();
    if !report.is_clean() {
        lines.push(report.render());
    }
    lines.join("\n")
}

fn show(name: Option<&str>) -> Result<String, String> {
    let (store, _) = LayoutStore::load(layouts_dir());
    let id = LayoutId::new(name.unwrap_or(layout::BUILT_IN));
    let found = store
        .get(&id)
        .ok_or_else(|| format!("there is no layout called '{id}'"))?;
    toml::to_string_pretty(found).map_err(|why| why.to_string())
}

/// Parses, validates and resolves one layout, and says what is wrong with it.
///
/// Resolution is per output, and which outputs exist is a question only a running compositor answers, so this resolves against one nominal output. That catches everything that does not depend on a monitor's name — a missing prompt, an area with no kind, an instance with no module — and leaves the per-monitor half to the running shell's own notice.
fn check(name: Option<&str>) -> Result<String, String> {
    let (store, mut report) = LayoutStore::load(layouts_dir());
    let id = LayoutId::new(name.unwrap_or(layout::BUILT_IN));
    let found = store
        .get(&id)
        .ok_or_else(|| format!("there is no layout called '{id}'"))?;

    let path = store.path_of(&id);
    if let Ok(text) = std::fs::read_to_string(&path) {
        report.merge(layout::check_unknown_keys(&text, &id));
    }
    report.merge(layout::validate(
        found,
        &Descriptors(crate::core::modules::MODULES),
    ));

    let (resolved, resolving) = layout::resolve(found, store.all(), NOMINAL_OUTPUT, None);
    report.merge(resolving);
    report.merge(layout::validate_resolved(
        &resolved,
        &path.display().to_string(),
    ));

    match report.is_clean() {
        true => Ok(report.summary()),
        false => Err(report.render()),
    }
}

fn layouts_dir() -> std::path::PathBuf {
    paths::config_dir().join("layouts")
}

/// What the layout model has to ask the module table.
///
/// It lives here rather than in `crates/layout` because that crate deliberately knows nothing about modules or the IPC table — the whole point of `Catalogue` is that the model can be validated by a test that states its own three modules. This is the real answer, wired to the descriptors this binary ships.
///
/// **It carries the table rather than reading the installed one.** `ui::descriptor::install` runs in `setup_shell`, and these verbs answer in the CLI process where nothing has run it, so an installed-table lookup answers `None` for every module the binary has — which came out as `layout check` calling `clock` an unknown module. `config check` already takes the table as an argument for the same reason (`check::command`).
struct Descriptors(&'static [ui::descriptor::ModuleDescriptor]);

impl Descriptors {
    fn find(&self, module: &str) -> Option<&'static ui::descriptor::ModuleDescriptor> {
        ui::descriptor::lookup(self.0, module)
    }
}

impl layout::Catalogue for Descriptors {
    fn knows_module(&self, module: &str) -> bool {
        self.find(module).is_some()
    }

    fn has_representation(&self, module: &str, representation: Representation) -> bool {
        self.find(module)
            .is_some_and(|found| found.input(drawn_as(representation)).is_some())
    }

    fn is_read_only(&self, module: &str, representation: Representation) -> bool {
        self.find(module)
            .and_then(|found| found.input(drawn_as(representation)))
            .is_some_and(|input| input == ui::descriptor::Input::ReadOnly)
    }

    fn command_resolves(&self, line: &str) -> bool {
        super::resolves(line)
    }
}

/// The layout model's five placeable sizes as the descriptor table names them. The table has two more, `Panel` and `Popout`, which are opened rather than placed and so have no way to appear in a layout.
fn drawn_as(representation: Representation) -> ui::host::Representation {
    use ui::host::{Representation as Drawn, WidgetSize};
    match representation {
        Representation::Chip => Drawn::Chip,
        Representation::WidgetS => Drawn::Widget(WidgetSize::S),
        Representation::WidgetM => Drawn::Widget(WidgetSize::M),
        Representation::WidgetL => Drawn::Widget(WidgetSize::L),
        Representation::Card => Drawn::Card,
    }
}

/// The name an import writes under when it is not given one.
#[cfg(test)]
mod tests {
    use layout::Layout;

    use super::*;

    #[test]
    fn the_built_in_layout_is_listed_and_marked_read_only() {
        let listed = list();
        assert!(
            listed.lines().any(|line| line.starts_with("default\t")),
            "{listed}"
        );
        assert!(listed.contains("read-only"), "{listed}");
    }

    #[test]
    fn showing_a_layout_that_is_not_there_says_so_rather_than_printing_nothing() {
        let refused = show(Some("no-such-layout")).expect_err("it refuses");
        assert!(refused.contains("no-such-layout"), "{refused}");
    }

    #[test]
    fn showing_the_built_in_layout_prints_something_that_parses_back() {
        let printed = show(None).expect("it prints");
        let again: Layout = toml::from_str(&printed).expect("and it parses back");
        assert_eq!(again.outputs.len(), 1);
    }

    /// The check has to pass on the layout the shell falls back to, or the fallback is not one.
    ///
    /// **Deliberately without installing the module table**, which is the process this verb actually runs in: `ui::descriptor::install` happens in `setup_shell`, and a check answers in the CLI. Installing it here is what hid the bug — every module in a real user's layout came back "there is no module called `clock`", because the catalogue looked in an empty table while the binary's own table sat one argument away.
    #[test]
    fn the_built_in_layout_checks_clean_without_the_module_table_installed() {
        telar::set_locale("en");
        assert!(
            ui::descriptor::installed().is_empty(),
            "this test is only worth anything while nothing installed the table on this thread"
        );
        let verdict = check(None);
        assert!(verdict.is_ok(), "{}", verdict.unwrap_err());
    }

    /// `check` answers from the files rather than from the shell, so it must refuse a name that is not there instead of reporting a clean layout it never read.
    #[test]
    fn checking_a_layout_that_is_not_there_is_an_error_not_a_clean_report() {
        telar::set_locale("en");
        ui::descriptor::install(crate::core::modules::MODULES);
        let refused = check(Some("no-such-layout")).expect_err("it refuses");
        assert!(refused.contains("no-such-layout"), "{refused}");
    }
}
