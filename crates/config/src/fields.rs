//! Every option a section declares, typed: the control an inspector draws for it, what it means and what it is when unset.
//!
//! Read off the same declaration the schema, the man page and the validator read (F-1.8). The field's Rust type says which control it is, the enum's own variants what it may be, the doc comment what it means, and a `Range: <min> to <max>[, step <step>].` line in that comment how far a number goes, so nothing here is a second description of a key that could drift from the first.

use std::collections::HashMap;
use std::sync::OnceLock;

use crate::Config;
use crate::schema::{CONFIG_FIELD_RUST, CONFIG_FIELDS, CONFIG_VARIANTS, doc_for, struct_for};
use crate::sections::ModuleOverride;
use crate::theme::NordTheme;

/// The struct behind `[modules.<id>]`, whose keys every instance may also set on itself.
const PRESENTATION: &str = "ModuleOverride";

/// One option as an inspector shows it and a validator checks it.
#[derive(Clone, Debug, PartialEq)]
pub struct OptionField {
    /// Where the value is written, dotted from the table that holds it: `show_date`, `face.scale`.
    pub key: String,
    pub control: Control,
    /// Whether leaving it unset means something of its own, rather than taking `default`.
    pub optional: bool,
    pub doc: Option<&'static str>,
    /// What the shell uses when the key is absent; `None` for an optional key with nothing to print.
    pub default: Option<toml::Value>,
}

/// How an option is edited.
#[derive(Clone, Debug, PartialEq)]
pub enum Control {
    Bool,
    /// One of these, spelled as the file writes them.
    Enum(Vec<&'static str>),
    Number(Number),
    /// One of the theme's accents, by name.
    Colour,
    Text,
    List(Box<Control>),
    /// Names of the caller's choosing, each holding a value of this kind.
    Map(Box<Control>),
    /// A table of options of its own — one element of a list of tables.
    Table(Vec<OptionField>),
    /// A type nothing here knows how to edit, by the name it was declared with.
    Unknown(&'static str),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Number {
    pub integer: bool,
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub step: Option<f64>,
}

impl OptionField {
    /// What the option is called where it is edited: the catalogue's `option.<name>` for its last part in the active locale where there is one, else that part as words — `drawer_max_height` reads "Drawer max height". Read inside a build, it follows a change of language.
    pub fn label(&self) -> String {
        let name = self.key.rsplit('.').next().unwrap_or(&self.key);
        let catalog = &crate::__rsx_i18n::CATALOG;
        let active = telar::use_locale();
        let locale = active.as_deref().unwrap_or(catalog.default_locale);
        match catalog.message(&format!("option.{name}"), locale) {
            Some(message) => message.select(locale, &[]).render(&[]),
            None => words(name),
        }
    }
}

fn words(name: &str) -> String {
    let name = name.replace('_', " ");
    let mut chars = name.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().collect::<String>() + chars.as_str())
        .unwrap_or_default()
}

impl Control {
    /// Whether `value` is one this control takes, and what it takes when it is not.
    pub fn accepts(&self, value: &toml::Value) -> Result<(), String> {
        use toml::Value;
        match (self, value) {
            (Control::Unknown(_), _) | (Control::Bool, Value::Boolean(_)) => Ok(()),
            (Control::Text, Value::String(_)) => Ok(()),
            (Control::Enum(variants), Value::String(text)) if variants.contains(&text.as_str()) => {
                Ok(())
            }
            (Control::Colour, Value::String(name)) if NordTheme::has_accent(name) => Ok(()),
            (Control::Number(number), Value::Integer(whole)) => number.holds(*whole as f64),
            (Control::Number(number), Value::Float(value)) if !number.integer => {
                number.holds(*value)
            }
            (Control::List(element), Value::Array(items)) => {
                items.iter().try_for_each(|item| element.accepts(item))
            }
            (Control::Map(element), Value::Table(entries)) => {
                entries.values().try_for_each(|item| element.accepts(item))
            }
            (Control::Table(fields), Value::Table(entries)) => match check(fields, entries).first()
            {
                Some((key, why)) => Err(format!("`{key}` {why}")),
                None => Ok(()),
            },
            _ => Err(format!("takes {}", self.describe())),
        }
    }

    fn describe(&self) -> String {
        match self {
            Control::Bool => "true or false".to_string(),
            Control::Enum(variants) => format!(
                "one of {}",
                variants
                    .iter()
                    .map(|variant| format!("`{variant}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Control::Number(number) if number.integer => "a whole number".to_string(),
            Control::Number(_) => "a number".to_string(),
            Control::Colour => format!(
                "one of the theme's accents ({})",
                crate::theme::ACCENTS.join(", ")
            ),
            Control::Text => "text".to_string(),
            Control::List(element) => format!("a list, each {}", element.describe()),
            Control::Map(element) => format!("a table, each value {}", element.describe()),
            Control::Table(_) => "a table".to_string(),
            Control::Unknown(declared) => format!("a `{declared}`"),
        }
    }
}

impl Number {
    fn holds(&self, value: f64) -> Result<(), String> {
        let below = self.min.is_some_and(|min| value < min);
        let above = self.max.is_some_and(|max| value > max);
        match (self.min, self.max) {
            (Some(min), Some(max)) if below || above => Err(format!("is between {min} and {max}")),
            (Some(min), None) if below => Err(format!("is at least {min}")),
            _ => Ok(()),
        }
    }
}

/// Every option of `[section]`, a nested table's keys dotted under its name; `None` for a section the schema does not have.
pub fn section(section: &str) -> Option<Vec<OptionField>> {
    let structure = struct_for(section)?;
    let defaults = starter().get(section).and_then(toml::Value::as_table);
    Some(fields_of(structure, "", defaults))
}

/// Every option of `[modules.<id>]` — how any module is presented — which an instance of any module may also set on itself.
pub fn presentation() -> Vec<OptionField> {
    let defaults = toml::Value::try_from(ModuleOverride::default()).ok();
    fields_of(
        PRESENTATION,
        "",
        defaults.as_ref().and_then(toml::Value::as_table),
    )
}

/// What is wrong with `options` against `fields`, as `(key, why)`: a key none of them is, or a value its control does not take.
pub fn check(fields: &[OptionField], options: &toml::Table) -> Vec<(String, String)> {
    let mut problems = Vec::new();
    walk(fields, options, "", &mut problems);
    problems
}

fn walk(
    fields: &[OptionField],
    table: &toml::Table,
    prefix: &str,
    problems: &mut Vec<(String, String)>,
) {
    for (key, value) in table {
        let path = format!("{prefix}{key}");
        let nested = format!("{path}.");
        if let Some(field) = fields.iter().find(|field| field.key == path) {
            if let Err(why) = field.control.accepts(value) {
                problems.push((path, why));
            }
        } else if let toml::Value::Table(inner) = value
            && fields.iter().any(|field| field.key.starts_with(&nested))
        {
            walk(fields, inner, &nested, problems);
        } else {
            problems.push((path, "is not one of its options".to_string()));
        }
    }
}

/// The top-level keys of `[section]`'s struct, which is what an instance's options are read against for that section.
pub(crate) fn keys_of(section: &str) -> &'static [&'static str] {
    static KEYS: OnceLock<HashMap<&'static str, Vec<&'static str>>> = OnceLock::new();
    KEYS.get_or_init(|| {
        crate::schema::section_names()
            .into_iter()
            .filter_map(|section| Some((section, fields(struct_for(section)?))))
            .collect()
    })
    .get(section)
    .map_or(&[], Vec::as_slice)
}

/// The keys of `[modules.<id>]`.
pub(crate) fn presentation_keys() -> &'static [&'static str] {
    static KEYS: OnceLock<Vec<&'static str>> = OnceLock::new();
    KEYS.get_or_init(|| fields(PRESENTATION))
}

fn fields(structure: &str) -> Vec<&'static str> {
    CONFIG_FIELDS
        .iter()
        .filter(|(owner, _)| *owner == structure)
        .map(|(_, field)| *field)
        .collect()
}

/// The defaults a fresh install writes, as the table the schema prints them from. Built once: it is the same every time.
fn starter() -> &'static toml::Table {
    static STARTER: OnceLock<toml::Table> = OnceLock::new();
    STARTER.get_or_init(|| match toml::Value::try_from(Config::starter()) {
        Ok(toml::Value::Table(table)) => table,
        _ => toml::Table::new(),
    })
}

fn fields_of(structure: &str, prefix: &str, defaults: Option<&toml::Table>) -> Vec<OptionField> {
    let mut found = Vec::new();
    for (owner, name, declared) in CONFIG_FIELD_RUST {
        if *owner != structure {
            continue;
        }
        let key = format!("{prefix}{name}");
        let default = defaults.and_then(|table| table.get(*name));
        let (optional, inner) = match wrapped(declared, "Option") {
            Some(inner) => (true, inner),
            None => (false, *declared),
        };
        if is_struct(inner) {
            found.extend(fields_of(
                inner,
                &format!("{key}."),
                default.and_then(toml::Value::as_table),
            ));
            continue;
        }
        let doc = doc_for(structure, name);
        found.push(OptionField {
            control: ranged(control_of(inner), doc),
            key,
            optional,
            doc,
            default: default.cloned(),
        });
    }
    found
}

fn control_of(declared: &'static str) -> Control {
    if let Some(element) = wrapped(declared, "Vec") {
        return Control::List(Box::new(control_of(element)));
    }
    if let Some(entries) = wrapped(declared, "HashMap").or_else(|| wrapped(declared, "BTreeMap"))
        && let Some((_, value)) = entries.split_once(',')
    {
        return Control::Map(Box::new(control_of(value.trim())));
    }
    let number = |integer: bool, min: Option<f64>| {
        Control::Number(Number {
            integer,
            min,
            max: None,
            step: None,
        })
    };
    match declared {
        "bool" => Control::Bool,
        "f32" | "f64" => number(false, None),
        "u8" | "u16" | "u32" | "u64" | "usize" => number(true, Some(0.0)),
        "i8" | "i16" | "i32" | "i64" | "isize" => number(true, None),
        "String" | "PathBuf" => Control::Text,
        "AccentName" => Control::Colour,
        name if is_struct(name) => Control::Table(fields_of(name, "", None)),
        name => {
            let variants: Vec<&'static str> = CONFIG_VARIANTS
                .iter()
                .filter(|(owner, _)| *owner == name)
                .map(|(_, variant)| *variant)
                .collect();
            if variants.is_empty() {
                Control::Unknown(declared)
            } else {
                Control::Enum(variants)
            }
        }
    }
}

/// `control` with the range its doc comment declares, when it is a number and the comment declares one.
fn ranged(control: Control, doc: Option<&str>) -> Control {
    let Control::Number(mut number) = control else {
        return control;
    };
    if let Some((min, max, step)) = doc.and_then(range_in) {
        number.min = Some(min);
        number.max = Some(max);
        number.step = step;
    }
    Control::Number(number)
}

/// `Range: 140 to 900, step 4.` → `(140, 900, Some(4))`.
fn range_in(doc: &str) -> Option<(f64, f64, Option<f64>)> {
    let line = doc
        .lines()
        .find_map(|line| line.trim().strip_prefix("Range: "))?;
    let line = line.trim_end_matches('.');
    let (range, step) = match line.split_once(", step ") {
        Some((range, step)) => (range, Some(step.trim().parse().ok()?)),
        None => (line, None),
    };
    let (min, max) = range.split_once(" to ")?;
    Some((min.trim().parse().ok()?, max.trim().parse().ok()?, step))
}

fn wrapped<'a>(declared: &'a str, outer: &str) -> Option<&'a str> {
    declared
        .strip_prefix(outer)?
        .strip_prefix('<')?
        .strip_suffix('>')
        .map(str::trim)
}

fn is_struct(name: &str) -> bool {
    CONFIG_FIELDS.iter().any(|(owner, _)| *owner == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn every_module_section() -> impl Iterator<Item = (&'static str, Vec<OptionField>)> {
        crate::options::MODULE_OPTIONS.iter().map(|(section, _)| {
            (
                *section,
                super::section(section).unwrap_or_else(|| panic!("[{section}] has no schema")),
            )
        })
    }

    fn unknown(fields: &[OptionField]) -> Vec<String> {
        let mut found = Vec::new();
        for field in fields {
            match &field.control {
                Control::Unknown(declared) => found.push(format!("{} ({declared})", field.key)),
                Control::Table(inner) => found.extend(unknown(inner)),
                Control::List(element) | Control::Map(element) => {
                    if let Control::Unknown(declared) = element.as_ref() {
                        found.push(format!("{} ({declared})", field.key));
                    }
                }
                _ => {}
            }
        }
        found
    }

    /// The inspector draws a control per option, so an option whose type no control answers is one it could only show as raw text.
    #[test]
    fn every_module_option_and_every_presentation_key_has_a_control() {
        let mut missing = Vec::new();
        for (section, fields) in every_module_section() {
            assert!(!fields.is_empty(), "[{section}] declares no options");
            missing.extend(
                unknown(&fields)
                    .into_iter()
                    .map(|key| format!("{section}.{key}")),
            );
        }
        missing.extend(
            unknown(&presentation())
                .into_iter()
                .map(|key| format!("modules.<id>.{key}")),
        );
        assert!(missing.is_empty(), "no control for: {missing:?}");
    }

    /// `key = value` under `[section]`, the dotted key opened into the tables it names.
    fn written(section: &str, key: &str, value: toml::Value) -> toml::Table {
        let mut parts: Vec<&str> = key.split('.').collect();
        let mut table = toml::Table::from_iter([(parts.pop().unwrap_or(key).to_string(), value)]);
        while let Some(part) = parts.pop() {
            table = toml::Table::from_iter([(part.to_string(), toml::Value::Table(table))]);
        }
        toml::Table::from_iter([(section.to_string(), toml::Value::Table(table))])
    }

    fn read_back<'a>(table: &'a toml::Table, section: &str, key: &str) -> Option<&'a toml::Value> {
        let mut at = table.get(section)?;
        for part in key.split('.') {
            at = at.as_table()?.get(part)?;
        }
        Some(at)
    }

    /// An enum's variants are read off the source, so each one is written into a config file and read back through `Config` itself: a variant the scanner spelled differently from serde would not parse.
    #[test]
    fn every_enum_option_lists_the_variants_serde_accepts() {
        let mut enums = 0;
        for (section, fields) in every_module_section() {
            for field in fields {
                let Control::Enum(variants) = &field.control else {
                    continue;
                };
                assert!(!variants.is_empty(), "{section}.{}", field.key);
                for variant in variants {
                    enums += 1;
                    let value = toml::Value::String(variant.to_string());
                    let text = toml::to_string(&written(section, &field.key, value.clone()))
                        .expect("a table prints");
                    let config: Config = toml::from_str(&text).unwrap_or_else(|why| {
                        panic!(
                            "[{section}] {} = \"{variant}\" does not parse: {why}",
                            field.key
                        )
                    });
                    let back = toml::Table::try_from(&config).expect("the config prints");
                    assert_eq!(
                        read_back(&back, section, &field.key),
                        Some(&value),
                        "[{section}] {} = \"{variant}\" did not survive serde",
                        field.key
                    );
                }
            }
        }
        assert!(enums > 10, "only {enums} variants were checked");
        let open = presentation()
            .into_iter()
            .find(|field| field.key == "open")
            .expect("`open` is a presentation key");
        assert_eq!(open.control, Control::Enum(vec!["drawer", "float"]));
        assert!(open.optional);
    }

    #[test]
    fn a_documented_range_bounds_its_number() {
        let fields = presentation();
        let popout = fields
            .iter()
            .find(|field| field.key == "popout_width")
            .expect("popout_width");
        assert_eq!(
            popout.control,
            Control::Number(Number {
                integer: false,
                min: Some(140.0),
                max: Some(900.0),
                step: None,
            })
        );
        assert_eq!(popout.default, Some(toml::Value::Float(264.0)));
        assert_eq!(popout.label(), "Popout width");
        telar::set_locale("es");
        assert_eq!(popout.label(), "Ancho de la tarjeta emergente");
        telar::set_locale("en");
        let (min, max) = ModuleOverride {
            popout_width: 1.0,
            popout_max_height: 1.0e6,
            ..ModuleOverride::default()
        }
        .popout_size();
        assert_eq!(
            (min, max),
            (140.0, 1200.0),
            "the accessor clamps to what the doc declares"
        );
        assert_eq!(
            range_in("A width.\nRange: 0 to 1, step 0.05."),
            Some((0.0, 1.0, Some(0.05)))
        );
    }

    #[test]
    fn a_nested_table_s_options_are_dotted_under_its_name() {
        let clock = super::section("clock").expect("[clock]");
        let scale = clock
            .iter()
            .find(|field| field.key == "face.scale")
            .expect("`face.scale` is one of the clock's options");
        assert!(matches!(
            scale.control,
            Control::Number(Number { integer: false, .. })
        ));
        assert_eq!(scale.default, Some(toml::Value::Float(3.0)));
        let format = clock
            .iter()
            .find(|field| field.key == "format")
            .expect("format");
        assert!(format.optional && format.default.is_none() && format.control == Control::Text);
    }

    #[test]
    fn a_value_is_checked_against_its_control() {
        let clock = super::section("clock").expect("[clock]");
        let options: toml::Table = toml::from_str(
            "show_date = \"yes\"\nnope = 1\nface = { scale = 2.0, colour = \"red\" }\ntwelve_hour = true\n",
        )
        .expect("toml");
        let problems = check(&clock, &options);
        assert_eq!(
            problems,
            [
                (
                    "face.colour".to_string(),
                    "is not one of its options".to_string()
                ),
                ("nope".to_string(), "is not one of its options".to_string()),
                ("show_date".to_string(), "takes true or false".to_string()),
            ]
        );
        let accent = presentation();
        let options: toml::Table =
            toml::from_str("accent = \"mauve\"\npopout_width = 20\n").expect("toml");
        assert_eq!(
            check(&accent, &options)
                .into_iter()
                .map(|(key, _)| key)
                .collect::<Vec<_>>(),
            ["accent", "popout_width"],
            "an accent the theme does not name, and a width under the range"
        );
    }

    /// An instance's own value wins, a key it does not set falls back to the section, and a value of the wrong type costs only itself.
    #[test]
    fn an_instance_s_options_are_laid_over_its_section() {
        use crate::ModuleOptions;
        let config: Config =
            toml::from_str("[clock]\nshow_date = true\ndate_format = \"%d\"\n").expect("config");
        let options: toml::Table = toml::from_str(
            "date_format = \"%A\"\ntwelve_hour = \"no\"\nface = { scale = 5.0 }\naccent = \"red\"\n",
        )
        .expect("toml");
        let clock = crate::ClockConfig::with_options(&config, &options);
        assert_eq!(clock.date_format, "%A", "the instance's value wins");
        assert!(clock.show_date, "what it leaves unset is the section's");
        assert!(!clock.twelve_hour, "a value of the wrong type is skipped");
        assert_eq!(
            clock.face.scale, 5.0,
            "a nested table merges into the section's"
        );
        assert!(clock.face.shadow, "and keeps what it does not name");

        let presented = config.presentation("clock", &options);
        assert_eq!(presented.accent.as_deref(), Some("red"));
        assert_eq!(presented.drawer_width, 320.0);
    }
}
