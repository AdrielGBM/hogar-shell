//! `$theme.*`: the colours the shell is painting with, as a reading every expression may name — `mix($theme.accent, #f00, $cpu.usage / 100)`.
//!
//! Every token `[theme.colors]` can set ([`THEME_TOKENS`]), with `fg` and `bg` for the text and the base under it. Read from the config of the area the expression is drawn in where it has one, so a monitor with a theme of its own answers with its colours, else from the running config, and read again each time the palette changes (`colors_changed`): a binding is made again with every rebuild, but a rule outlives them all, so this is what keeps it on the palette being painted. Public: the palette says nothing about the user.

use std::sync::Arc;

use config::Config;
use config::theme::{NordTheme, THEME_TOKENS};
use services::events::EventKind;
use telar_expression::Value;
use ui::descriptor::{FieldDef, FieldType, Privacy, Reading, Sink, SourceDef};

use crate::sources::{self, SourceSpec};

/// The name the theme's colours are read under: `$theme.accent`.
pub const THEME_SOURCE: &str = "theme";

/// Names a theme reading gives a token besides its own, and the token each one reads.
const ALIASES: [(&str, &str); 2] = [("fg", "text"), ("bg", "base")];

const FIELD_COUNT: usize = THEME_TOKENS.len() + ALIASES.len();

const fn colour(name: &'static str) -> FieldDef {
    FieldDef {
        name,
        privacy: Privacy::Public,
        ty: FieldType::Color,
    }
}

const fn fields() -> [FieldDef; FIELD_COUNT] {
    let mut fields = [colour(""); FIELD_COUNT];
    let mut token = 0;
    while token < THEME_TOKENS.len() {
        fields[token] = colour(THEME_TOKENS[token]);
        token += 1;
    }
    let mut alias = 0;
    while alias < ALIASES.len() {
        fields[token + alias] = colour(ALIASES[alias].0);
        alias += 1;
    }
    fields
}

static FIELDS: [FieldDef; FIELD_COUNT] = fields();

/// The theme's colours: every token, then the aliases.
pub static THEME: SourceDef = SourceDef {
    id: THEME_SOURCE,
    fields: &FIELDS,
    feed,
};

fn feed(sink: Sink) {
    let area = crate::env::area_config();
    follow(
        sink,
        move || palette(area.as_deref()),
        |mut changed| {
            platform_wayland::watch(
                |tx| sources::subscribe(SourceSpec::event(EventKind::ColorsChanged), tx),
                move |_: Value| changed(),
            );
        },
    );
}

/// Feeds `sink` the palette `paint` reads now, and again each time `changes` says it moved.
fn follow(
    mut sink: Sink,
    paint: impl Fn() -> NordTheme + 'static,
    changes: impl FnOnce(Box<dyn FnMut()>),
) {
    sink(reading(&paint()));
    changes(Box::new(move || sink(reading(&paint()))));
}

/// The palette `area` paints with, or the running config's where there is none: a rule is drawn in no area, and reads the config every reload replaces.
fn palette(area: Option<&Config>) -> NordTheme {
    area.map(Config::resolve_theme)
        .or_else(|| config::config().map(|config| config.resolve_theme()))
        .unwrap_or_default()
}

/// `theme`'s colours in the order [`THEME`] declares them.
fn reading(theme: &NordTheme) -> Reading {
    FIELDS
        .iter()
        .map(|field| Value::Color(theme.token(token_of(field.name))))
        .collect::<Arc<[Value]>>()
}

fn token_of(field: &str) -> &str {
    ALIASES
        .iter()
        .find(|(alias, _)| *alias == field)
        .map_or(field, |(_, token)| token)
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use super::*;

    #[test]
    fn every_token_the_theme_has_is_a_field_with_fg_and_bg_besides() {
        let names: Vec<&str> = THEME.fields.iter().map(|field| field.name).collect();
        for token in THEME_TOKENS {
            assert!(names.contains(token), "`$theme.{token}` is missing");
        }
        assert!(names.contains(&"highlight_med"));
        assert!(names.contains(&"fg") && names.contains(&"bg"));
        assert_eq!(names.len(), THEME_TOKENS.len() + 2);
    }

    #[test]
    fn each_field_reads_its_token_and_an_alias_reads_the_token_it_names() {
        let theme = NordTheme::default();
        let read = reading(&theme);
        for (field, value) in THEME.fields.iter().zip(read.iter()) {
            assert_eq!(
                *value,
                Value::Color(theme.token(token_of(field.name))),
                "{}",
                field.name
            );
        }
        let at = |name: &str| {
            THEME
                .field(name)
                .map(|(index, _)| read[index].clone())
                .unwrap()
        };
        assert_eq!(at("fg"), Value::Color(theme.text));
        assert_eq!(at("bg"), Value::Color(theme.base));
        assert_eq!(at("highlight_high"), Value::Color(theme.highlight_high));
    }

    type Changed = Box<dyn FnMut()>;

    /// A rule outlives every rebuild, so the reading has to move with the palette rather than be taken once.
    #[test]
    fn the_reading_follows_the_palette_as_it_changes() {
        let painted = Rc::new(RefCell::new(NordTheme::default()));
        let seen: Rc<RefCell<Vec<Reading>>> = Rc::default();
        let changed: Rc<RefCell<Option<Changed>>> = Rc::default();
        let (into, now, told) = (Rc::clone(&seen), Rc::clone(&painted), Rc::clone(&changed));
        follow(
            Box::new(move |reading| into.borrow_mut().push(reading)),
            move || *now.borrow(),
            move |on_change| *told.borrow_mut() = Some(on_change),
        );
        assert_eq!(seen.borrow().len(), 1);

        let light = NordTheme {
            accent: telar::Color::from_hex("#ff8800").unwrap(),
            ..NordTheme::default()
        };
        *painted.borrow_mut() = light;
        (changed.borrow_mut().as_mut().unwrap())();
        let accent = THEME.field("accent").unwrap().0;
        assert_eq!(seen.borrow().len(), 2);
        assert_eq!(seen.borrow()[1][accent], Value::Color(light.accent));
    }
}
