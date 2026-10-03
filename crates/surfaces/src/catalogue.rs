//! What the layout model has to ask the module table: whether a module exists, what it can be drawn as, whether that only reads, whether a command line resolves, and what an expression's names read.
//!
//! It lives here rather than in `crates/layout` because that crate deliberately knows nothing about modules or the IPC table — the whole point of [`layout::Catalogue`] is that the model can be validated by a test that states its own modules.

use std::cell::OnceCell;
use std::sync::Arc;

use automation::bindings::Target;
use automation::{Environment, UserSources};
use layout::Representation;
use telar_expression::{Compiled, ErrorCode, Errors, Type};
use ui::descriptor::{Input, ModuleDescriptor};
use util::report::Message;

/// The module table and the command table validation asks, carried rather than read from what is installed: the CLI answers `layout check` in a process where nothing installed either, and an installed-table lookup there would call every module unknown.
#[derive(Clone)]
pub struct Descriptors {
    modules: &'static [ModuleDescriptor],
    resolves: fn(&str) -> bool,
    sources: Arc<UserSources>,
    /// What expressions read under these tables, made on the first expression checked.
    environment: OnceCell<Environment>,
}

impl Descriptors {
    /// The tables to ask, reading expressions against the sources the running layout declares.
    pub fn new(modules: &'static [ModuleDescriptor], resolves: fn(&str) -> bool) -> Self {
        Self {
            modules,
            resolves,
            sources: automation::sources::declared_sources(),
            environment: OnceCell::new(),
        }
    }

    /// The same tables, reading `$name` in an expression as one of `sources` — the layout being checked, where it is not the one running.
    pub fn with_sources(mut self, sources: UserSources) -> Self {
        self.sources = Arc::new(sources);
        self.environment = OnceCell::new();
        self
    }

    /// The tables the running shell installed, for a check made inside it.
    pub fn installed() -> Self {
        Self::new(ui::descriptor::installed(), services::command::resolves)
    }

    fn find(&self, module: &str) -> Option<&'static ModuleDescriptor> {
        ui::descriptor::lookup(self.modules, module)
    }

    /// What an expression on the lock layer (`on_lock`) or any other reads.
    pub fn environment(&self, on_lock: bool) -> Environment {
        self.environment
            .get_or_init(|| Environment::of_modules(self.modules, Arc::clone(&self.sources)))
            .clone()
            .on_layer(on_lock)
    }

    /// What a binding at `path` on an instance of `module` drives.
    pub fn target(&self, module: &str, path: &str) -> Result<Target, Message> {
        let found = self
            .find(module)
            .ok_or_else(|| util::message!("finding.unknown_module", module = module))?;
        Target::of(&found.option_fields(), path)
            .map_err(|why| util::message!("finding.of_module", module = module, why = why))
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

    fn option_problems(&self, module: &str, options: &toml::Table) -> Vec<(String, Message)> {
        self.find(module)
            .map_or_else(Vec::new, |found| found.option_problems(options))
    }

    fn is_service_source(&self, name: &str) -> bool {
        name == automation::EVENT
            || name == automation::theme::THEME_SOURCE
            || self
                .modules
                .iter()
                .flat_map(|module| module.sources.iter())
                .any(|source| source.id == name)
    }

    fn compile_with(
        &self,
        source: &str,
        on_lock: bool,
        locals: &layout::Locals,
    ) -> Result<Compiled, Errors> {
        self.environment(on_lock)
            .with_locals(automation::Local::typed_all(locals))
            .compile(source)
    }

    fn awaits_variable(&self, source: &str, error: &telar_expression::Error) -> bool {
        self.environment(false).awaits_variable(source, error)
    }

    fn binding_type(&self, module: &str, path: &str) -> Result<Type, Message> {
        self.target(module, path).map(|target| target.ty())
    }

    fn describe(&self, code: &ErrorCode) -> Message {
        automation::env::describe(code)
    }
}

#[cfg(test)]
mod tests {
    use layout::Catalogue;
    use ui::descriptor::{
        Category, ChipDef, FieldDef, FieldType, OptionsType, Privacy, Representations, Sink,
        SourceDef,
    };

    use super::*;

    fn unbuilt(_: &ui::host::Host) -> ui::descriptor::Built {
        Err(telar::LayoutError::Engine("never built".to_string()))
    }

    fn silent(_: Sink) {}

    static NOTES: SourceDef = SourceDef {
        id: "notes",
        fields: &[
            FieldDef {
                name: "count",
                privacy: Privacy::Public,
                ty: FieldType::Number,
            },
            FieldDef {
                name: "summary",
                privacy: Privacy::Private,
                ty: FieldType::Text,
            },
        ],
        feed: silent,
    };

    const TABLE: &[ModuleDescriptor] = &[ModuleDescriptor {
        id: "clock",
        name: "Clock",
        icon: "clock",
        category: Category::Info,
        options: &[OptionsType::of::<config::ClockConfig>()],
        representations: Representations {
            chip: Some(ChipDef::new(unbuilt, Input::ReadOnly)),
            ..Representations::NONE
        },
        actions: &[],
        sources: &[NOTES],
    }];

    fn descriptors() -> Descriptors {
        let sources: std::collections::BTreeMap<String, layout::Source> = toml::from_str(
            "[uptime]\nkind = 'poll'\ncmd = 'uptime'\n[load]\nkind = 'poll'\ncmd = 'cat /proc/loadavg'\nlock_safe = true",
        )
        .expect("the sources parse");
        let (sources, report) = UserSources::of(
            "layouts/t.toml",
            &sources,
            &config::AutomationConfig::default(),
        );
        assert!(report.is_clean(), "{}", report.render());
        Descriptors::new(TABLE, |_| true).with_sources(sources)
    }

    fn ty(catalogue: &Descriptors, source: &str, on_lock: bool) -> Result<Type, Errors> {
        catalogue
            .compile(source, on_lock)
            .map(|compiled| compiled.ty().clone())
    }

    /// A variable is machine state the shell sets as it runs, so naming one not set yet waits for it, and checks as its type once it is; a name that could never be a variable does not wait.
    #[test]
    fn a_variable_not_set_yet_is_waited_for_and_checks_once_it_is_set() {
        automation::vars::remove("catalogue_test_later");
        let catalogue = descriptors();
        let source = "$catalogue_test_later && true";
        let errors = catalogue.compile(source, false).expect_err("not set yet");
        assert!(
            errors
                .iter()
                .all(|error| catalogue.awaits_variable(source, error)),
            "{errors}"
        );
        let typo = "$notes.cuont";
        let errors = catalogue.compile(typo, false).expect_err("no such field");
        assert!(
            !errors
                .iter()
                .any(|error| catalogue.awaits_variable(typo, error))
        );

        automation::vars::set("catalogue_test_later", services::state::Var::Bool(true)).unwrap();
        assert_eq!(ty(&catalogue, source, false), Ok(Type::Bool));
        automation::vars::remove("catalogue_test_later");
    }

    /// TA-8 at load: on the lock layer a private field still checks — it reads as its type's empty value — while a command source that is not `lock_safe` is refused there and only there.
    #[test]
    fn the_lock_layer_reads_private_fields_as_empty_and_refuses_a_command_that_is_not_lock_safe() {
        let catalogue = descriptors();
        assert_eq!(ty(&catalogue, "$notes.summary", true), Ok(Type::Text));
        assert_eq!(ty(&catalogue, "$uptime", false), Ok(Type::Text));
        let refused = catalogue
            .compile("$uptime", true)
            .expect_err("the lock screen refuses it");
        assert!(refused.to_string().contains("lock_safe"), "{refused}");
        assert_eq!(ty(&catalogue, "$load", true), Ok(Type::Text));
        assert_eq!(ty(&catalogue, "$theme.accent", true), Ok(Type::Color));
    }

    #[test]
    fn a_binding_takes_the_type_of_the_option_it_drives_or_a_colour_for_accent() {
        let catalogue = descriptors();
        assert_eq!(catalogue.binding_type("clock", "show_date"), Ok(Type::Bool));
        assert_eq!(
            catalogue.binding_type("clock", "date_format"),
            Ok(Type::Text)
        );
        assert_eq!(catalogue.binding_type("clock", "accent"), Ok(Type::Color));
        assert!(catalogue.binding_type("clock", "colour").is_err());
        assert!(catalogue.is_service_source("theme"));
        assert!(catalogue.is_service_source("notes"));
    }

    #[test]
    fn every_finding_has_words_in_every_language_the_shell_speaks() {
        assert_eq!(
            util::report::untranslated(&crate::__rsx_i18n::CATALOG, &["finding."]),
            Vec::<String>::new()
        );
    }
}
