//! What a binding in the layout drives: the type an instance's option takes as an expression, and how an evaluated value is written back as that option.
//!
//! **Targets.** A binding's path is an option its module declares (`config::fields`, through `ModuleDescriptor::option_fields`), `accent`, or one of the instance's own style keys (`style.fill`, `style.opacity`, `style.border.color`). An option takes the type its control does — a bool, a number, a text, a list of one of those; one of a fixed set of words is a text, checked against the set when it is written. `accent` takes a colour and paints the instance itself, rather than naming one of the theme's accents as the written option does; a style key paints the instance's box the way its `style` does.
//!
//! **Writing.** A value is written in the control's own spelling — a whole number for an integer, a word for one of a set — and then checked as a written value would be, so a binding can never put an option somewhere `layout set` could not.

use config::fields::{Control, OptionField};
use layout::StyleBinding;
use telar::Color;
use telar_expression::{Type, Value};
use util::report::Message;

/// The path that binds the colour an instance is painted with.
pub const ACCENT: &str = "accent";

/// What one binding drives.
#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    /// The colour the instance is painted with, in place of the accent its options name.
    Accent,
    /// A key of the instance's own `style`.
    Style(StyleBinding),
    Option(OptionField),
}

/// A value a binding evaluated to, written as what it drives.
#[derive(Clone, Debug, PartialEq)]
pub enum Written {
    Accent(Color),
    Fill(Color),
    /// Already within 0 to 1.
    Opacity(f32),
    BorderColor(Color),
    Option(toml::Value),
}

impl Target {
    /// What a binding at `path` drives on a module declaring `fields`, or why nothing can be bound there.
    pub fn of(fields: &[OptionField], path: &str) -> Result<Self, Message> {
        if path == ACCENT {
            return Ok(Target::Accent);
        }
        if let Some(key) = StyleBinding::from_path(path) {
            return Ok(Target::Style(key));
        }
        let field = fields
            .iter()
            .find(|field| field.key == path)
            .ok_or_else(|| util::message!("finding.no_option_to_bind", path = path))?;
        type_of(&field.control)
            .map_err(|why| util::message!("finding.target", path = path, why = why))?;
        Ok(Target::Option(field.clone()))
    }

    /// The type an expression bound here has to give.
    pub fn ty(&self) -> Type {
        match self {
            Target::Accent => Type::Color,
            Target::Style(key) => key.ty(),
            Target::Option(field) => type_of(&field.control).unwrap_or(Type::Never),
        }
    }

    /// `value` written as what this target takes, or why it cannot be.
    pub fn write(&self, value: &Value) -> Result<Written, Message> {
        match (self, value) {
            (Target::Accent, Value::Color(color)) => Ok(Written::Accent(*color)),
            (Target::Accent, other) => Err(util::message!(
                "finding.accent_takes_colour",
                found = Message::type_name(&other.type_of())
            )),
            (Target::Style(StyleBinding::Fill), Value::Color(color)) => Ok(Written::Fill(*color)),
            (Target::Style(StyleBinding::BorderColor), Value::Color(color)) => {
                Ok(Written::BorderColor(*color))
            }
            (Target::Style(StyleBinding::Opacity), Value::Number(n)) => {
                Ok(Written::Opacity(n.clamp(0.0, 1.0) as f32))
            }
            (Target::Style(key), other) => Err(util::message!(
                "finding.style_takes",
                path = key.path(),
                wanted = Message::type_name(&key.ty()),
                found = Message::type_name(&other.type_of())
            )),
            (Target::Option(field), value) => {
                let written = option_value(&field.control, value)?;
                field.control.accepts(&written).map_err(|why| {
                    util::message!("finding.target", path = &field.key, why = why)
                })?;
                Ok(Written::Option(written))
            }
        }
    }
}

fn type_of(control: &Control) -> Result<Type, Message> {
    Ok(match control {
        Control::Bool => Type::Bool,
        Control::Number(_) => Type::Number,
        Control::Text | Control::Enum(_) | Control::Colour => Type::Text,
        Control::List(element) => Type::list(type_of(element)?),
        Control::Map(_) | Control::Table(_) => {
            return Err(util::message!("finding.gives_no_table"));
        }
        Control::Unknown(declared) => {
            return Err(util::message!(
                "finding.gives_no_declared",
                declared = declared
            ));
        }
    })
}

fn option_value(control: &Control, value: &Value) -> Result<toml::Value, Message> {
    Ok(match (control, value) {
        (Control::Bool, Value::Bool(it)) => toml::Value::Boolean(*it),
        (Control::Number(number), Value::Number(n)) if number.integer => {
            toml::Value::Integer(n.round() as i64)
        }
        (Control::Number(_), Value::Number(n)) => toml::Value::Float(*n),
        (Control::Text | Control::Enum(_) | Control::Colour, Value::Text(text)) => {
            toml::Value::String(text.to_string())
        }
        (Control::List(element), Value::List(items)) => toml::Value::Array(
            items
                .iter()
                .map(|item| option_value(element, item))
                .collect::<Result<_, _>>()?,
        ),
        (control, value) => {
            return Err(util::message!(
                "finding.option_cannot_take",
                found = match type_of(control) {
                    Ok(_) => Message::type_name(&value.type_of()),
                    Err(why) => why,
                }
            ));
        }
    })
}

/// Writes `value` at the dotted `path` of `options`, making the tables on the way.
pub fn overlay(options: &mut toml::Table, path: &str, value: toml::Value) {
    match path.split_once('.') {
        None => {
            options.insert(path.to_string(), value);
        }
        Some((head, rest)) => {
            let inner = options
                .entry(head.to_string())
                .or_insert_with(|| toml::Value::Table(toml::Table::new()));
            if !inner.is_table() {
                *inner = toml::Value::Table(toml::Table::new());
            }
            if let toml::Value::Table(inner) = inner {
                overlay(inner, rest, value);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clock() -> Vec<OptionField> {
        config::fields::section("clock").expect("[clock]")
    }

    #[test]
    fn an_option_takes_the_type_its_control_does_and_accent_takes_a_colour() {
        let fields = clock();
        let ty = |path: &str| Target::of(&fields, path).map(|target| target.ty());
        assert_eq!(ty("show_date"), Ok(Type::Bool));
        assert_eq!(ty("date_format"), Ok(Type::Text));
        assert_eq!(ty(ACCENT), Ok(Type::Color));
        assert!(ty("colour").is_err(), "a key the module does not have");
    }

    #[test]
    fn a_value_is_written_in_the_option_s_own_spelling_and_checked_as_written() {
        let fields = clock();
        let show_date = Target::of(&fields, "show_date").unwrap();
        assert_eq!(
            show_date.write(&Value::Bool(true)),
            Ok(Written::Option(toml::Value::Boolean(true)))
        );
        assert!(show_date.write(&Value::Number(1.0)).is_err());

        let accent = Target::Accent;
        let red = Color::from_hex("#ff0000").unwrap();
        assert_eq!(accent.write(&Value::Color(red)), Ok(Written::Accent(red)));
        assert!(accent.write(&Value::text("red")).is_err());
    }

    #[test]
    fn a_style_key_takes_its_own_type_on_any_module() {
        let ty = |path: &str| Target::of(&[], path).map(|target| target.ty());
        assert_eq!(ty("style.fill"), Ok(Type::Color));
        assert_eq!(ty("style.border.color"), Ok(Type::Color));
        assert_eq!(ty("style.opacity"), Ok(Type::Number));
        assert!(
            ty("style.radius").is_err(),
            "only a colour or a number binds"
        );

        let red = Color::from_hex("#ff0000").unwrap();
        let fill = Target::Style(StyleBinding::Fill);
        assert_eq!(fill.write(&Value::Color(red)), Ok(Written::Fill(red)));
        assert!(fill.write(&Value::Number(1.0)).is_err());
        assert_eq!(
            Target::Style(StyleBinding::Opacity).write(&Value::Number(1.5)),
            Ok(Written::Opacity(1.0)),
            "an opacity is held within 0 to 1"
        );
    }

    #[test]
    fn a_whole_number_option_takes_the_nearest_whole_number_within_its_range() {
        let field = OptionField {
            key: "size".to_string(),
            control: Control::Number(config::fields::Number {
                integer: true,
                min: Some(1.0),
                max: Some(10.0),
                step: None,
            }),
            optional: false,
            doc: None,
            default: None,
        };
        let target = Target::Option(field);
        assert_eq!(
            target.write(&Value::Number(3.6)),
            Ok(Written::Option(toml::Value::Integer(4)))
        );
        assert!(
            target.write(&Value::Number(40.0)).is_err(),
            "out of range is refused, as a written value would be"
        );
    }

    #[test]
    fn a_bound_value_is_laid_over_a_nested_option() {
        let mut options: toml::Table =
            toml::from_str("show_date = false\n[face]\nscale = 1.0").unwrap();
        overlay(&mut options, "face.scale", toml::Value::Float(2.0));
        overlay(&mut options, "show_date", toml::Value::Boolean(true));
        assert_eq!(
            options,
            toml::from_str::<toml::Table>("show_date = true\n[face]\nscale = 2.0").unwrap()
        );
    }
}
