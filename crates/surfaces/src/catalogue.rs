//! What the layout model has to ask the module table: whether a module exists, what it can be drawn as, whether that only reads, and whether a command line resolves.
//!
//! It lives here rather than in `crates/layout` because that crate deliberately knows nothing about modules or the IPC table — the whole point of [`layout::Catalogue`] is that the model can be validated by a test that states its own modules.

use layout::Representation;
use ui::descriptor::{Input, ModuleDescriptor};

/// The module table and the command table validation asks, carried rather than read from what is installed: the CLI answers `layout check` in a process where nothing installed either, and an installed-table lookup there would call every module unknown.
#[derive(Clone, Copy)]
pub struct Descriptors {
    modules: &'static [ModuleDescriptor],
    resolves: fn(&str) -> bool,
}

impl Descriptors {
    pub fn new(modules: &'static [ModuleDescriptor], resolves: fn(&str) -> bool) -> Self {
        Self { modules, resolves }
    }

    /// The tables the running shell installed, for a check made inside it.
    pub fn installed() -> Self {
        Self::new(ui::descriptor::installed(), services::command::resolves)
    }

    fn find(&self, module: &str) -> Option<&'static ModuleDescriptor> {
        ui::descriptor::lookup(self.modules, module)
    }
}

impl layout::Catalogue for Descriptors {
    fn knows_module(&self, module: &str) -> bool {
        self.find(module).is_some()
    }

    fn has_representation(&self, module: &str, representation: Representation) -> bool {
        self.find(module).is_some_and(|found| {
            found
                .input(crate::area::representation(representation))
                .is_some()
        })
    }

    fn is_read_only(&self, module: &str, representation: Representation) -> bool {
        self.find(module)
            .and_then(|found| found.input(crate::area::representation(representation)))
            .is_some_and(|input| input == Input::ReadOnly)
    }

    fn command_resolves(&self, line: &str) -> bool {
        (self.resolves)(line)
    }

    fn option_problems(&self, module: &str, options: &toml::Table) -> Vec<(String, String)> {
        self.find(module)
            .map_or_else(Vec::new, |found| found.option_problems(options))
    }
}
