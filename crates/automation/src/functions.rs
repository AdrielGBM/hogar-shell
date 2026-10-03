//! The shell's own function families, added to `telar-expression`'s standard ones: what an expression needs that only this shell knows how to do.
//!
//! - **`df`** writes a moment as the clock does, with the same `strftime` code (`services::clock::format`): `df("%H:%M", $clock.now)`.
//! - **`tr`** picks the text for the language the shell is speaking: `tr("Battery", "es", "Batería", "de", "Akku")`.
//! - **`bytes`** and **`rate`** write a byte count and a byte rate the way the system modules do (KLWP's `si`).
//!
//! KLWP's `bi`, `mi`, `wi` and `nc` are not here: each would read a value that `$battery`, `$media`, `$weather` and `$notifications` already give (R-7). Nor are colour helpers beyond `mix`, `alpha` and `contrast`, which cover what `config::scheme` offers — readable text is `contrast(a, b) >= 4.5`.

use services::clock::Unwritable;
use telar_expression::{CallError, ErrorCode, Pattern, Registry, Signature, Type, Value};
use util::report::Message;

/// The standard families and the shell's: what every expression in the shell is compiled against.
pub fn registry() -> Registry {
    Registry::standard().with(date).with(language).with(system)
}

/// A function's failure at its argument `index`, in this crate's words.
fn refused(index: usize, message: Message) -> CallError {
    CallError::argument(index, ErrorCode::Host(message.host_error()))
}

fn text() -> Pattern {
    Pattern::Exact(Type::Text)
}

fn number() -> Pattern {
    Pattern::Exact(Type::Number)
}

/// `df(format, moment)`: a moment in seconds since the Unix epoch, written with a `strftime` pattern in local time.
pub fn date(registry: &mut Registry) {
    registry.function(
        "df",
        Signature::new([text(), number()], Type::Text),
        "Writes a moment, in seconds since the epoch, with a strftime pattern",
        |a| {
            let (pattern, seconds) = (a.text(0)?, a.number(1)?);
            services::clock::format_at(seconds, pattern)
                .map(Value::text)
                .map_err(|why| match why {
                    Unwritable::Pattern => refused(
                        0,
                        util::message!("expression.not_a_date_pattern", pattern = pattern),
                    ),
                    Unwritable::Moment => refused(
                        1,
                        util::message!("expression.not_a_moment", seconds = seconds),
                    ),
                })
        },
    );
}

/// `tr(text, language, text, …)`: the text given for the language the shell speaks, the first one where none is given for it. A language matches exactly (`es-MX`) before by its first part (`es`).
pub fn language(registry: &mut Registry) {
    registry.function(
        "tr",
        Signature::new([text()], Type::Text).rest(text()),
        "The text for the language the shell is speaking",
        |a| {
            let pairs = a.rest(1);
            if pairs.len() % 2 != 0 {
                return Err(refused(
                    a.len() - 1,
                    util::message!("expression.language_needs_text"),
                ));
            }
            let fallback = a.text(0)?;
            let Some(active) = telar::i18n::use_locale() else {
                return Ok(Value::text(fallback));
            };
            let translations: Vec<(&str, &str)> = pairs
                .chunks(2)
                .filter_map(|pair| Some((pair[0].as_text()?, pair[1].as_text()?)))
                .collect();
            Ok(Value::text(
                pick(&active, &translations).unwrap_or(fallback),
            ))
        },
    );
}

fn pick<'a>(active: &str, translations: &[(&str, &'a str)]) -> Option<&'a str> {
    let primary = |tag: &str| {
        tag.split(['-', '_'])
            .next()
            .unwrap_or(tag)
            .to_ascii_lowercase()
    };
    translations
        .iter()
        .find(|(tag, _)| tag.eq_ignore_ascii_case(active))
        .or_else(|| {
            translations
                .iter()
                .find(|(tag, _)| primary(tag) == primary(active))
        })
        .map(|(_, text)| *text)
}

/// `bytes(n)` and `rate(n)`: a byte count in binary units (`1.5 GiB`), a rate in bytes a second in decimal ones (`1.2 MB/s`), as `$memory` and `$netspeed` read.
pub fn system(registry: &mut Registry) {
    registry
        .function(
            "bytes",
            Signature::new([number()], Type::Text),
            "A byte count in binary units, as 1.5 GiB",
            |a| {
                let n = a.number(0)?;
                if n < 0.0 {
                    return Err(refused(0, util::message!("expression.negative_bytes")));
                }
                Ok(Value::text(services::resources::format_bytes(n as u64)))
            },
        )
        .function(
            "rate",
            Signature::new([number()], Type::Text),
            "A rate in bytes a second, as 1.2 MB/s",
            |a| {
                let n = a.number(0)?;
                if n < 0.0 {
                    return Err(refused(0, util::message!("expression.negative_rate")));
                }
                Ok(Value::text(services::netspeed::format_rate(n)))
            },
        );
}

#[cfg(test)]
mod tests {
    use telar_expression::{Closed, Errors, compile};

    use super::*;

    fn evaluate(source: &str) -> Result<Value, String> {
        let registry = registry();
        let compiled =
            compile(source, &Closed, &registry).map_err(|errors: Errors| errors.to_string())?;
        compiled
            .evaluate(&Closed)
            .map_err(|error| error.to_string())
    }

    #[test]
    fn every_family_is_registered_beside_the_standard_ones() {
        let registry = registry();
        let names: Vec<&str> = registry
            .functions()
            .map(|function| function.name())
            .collect();
        for name in ["df", "tr", "bytes", "rate", "fmt", "round", "mix", "join"] {
            assert!(names.contains(&name), "`{name}` is missing: {names:?}");
        }
    }

    #[test]
    fn df_writes_a_moment_the_way_the_clock_does() {
        assert_eq!(
            evaluate("df('%s', 1700000000)"),
            Ok(Value::text("1700000000"))
        );
        assert!(
            evaluate("df('%Q', 0)").unwrap_err().contains("%Q"),
            "a pattern chrono cannot write is an error, never a panic"
        );
    }

    #[test]
    fn tr_picks_the_text_for_the_language_the_shell_speaks() {
        telar::set_locale("es-MX");
        assert_eq!(
            evaluate("tr('Battery', 'de', 'Akku', 'es', 'Batería')"),
            Ok(Value::text("Batería")),
            "`es` answers for `es-MX` when nothing says `es-MX` itself"
        );
        assert_eq!(
            evaluate("tr('Color', 'es', 'Color', 'es-MX', 'Colorcito')"),
            Ok(Value::text("Colorcito")),
            "an exact match wins over the language's first part"
        );
        assert_eq!(
            evaluate("tr('Battery', 'de', 'Akku')"),
            Ok(Value::text("Battery"))
        );
        assert!(evaluate("tr('Battery', 'de')").is_err());
        telar::set_locale("en");
    }

    #[test]
    fn bytes_and_rate_read_as_the_system_modules_write_them() {
        assert_eq!(evaluate("bytes(1536)"), Ok(Value::text("1.5 KiB")));
        assert_eq!(evaluate("rate(1200000)"), Ok(Value::text("1.2 MB/s")));
        assert!(evaluate("bytes(-1)").is_err());
    }
}
