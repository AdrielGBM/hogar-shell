//! How a command's output, or a response's body, becomes a reading: the `parse` key of a declared source.

use std::cmp::Ordering;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::Duration;

use regex::Regex;
use telar_expression::{Type, Value, format_number};
use util::report::Message;

use crate::scalar;

/// A `parse` spec, checked once when the source is declared.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum Parse {
    /// All of it, trimmed.
    #[default]
    Text,
    /// A list with one text per line.
    Lines,
    /// The value at a path into a JSON document.
    Json(Vec<Step>),
    /// The first match, or its first group when the pattern has one.
    Regex(Pattern),
}

/// One step of a `json:` path: a key of an object, or a position in an array.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Step {
    Key(String),
    Index(usize),
}

/// A compiled pattern, compared and ordered by its text so a spec can key a producer.
#[derive(Clone, Debug)]
pub struct Pattern(Arc<Regex>);

impl Pattern {
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl PartialEq for Pattern {
    fn eq(&self, other: &Self) -> bool {
        self.as_str() == other.as_str()
    }
}

impl Eq for Pattern {}

impl PartialOrd for Pattern {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Pattern {
    fn cmp(&self, other: &Self) -> Ordering {
        self.as_str().cmp(other.as_str())
    }
}

impl Hash for Pattern {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.as_str().hash(state);
    }
}

impl Parse {
    /// Reads a `parse` key as written; absent is [`Parse::Text`].
    pub fn from_spec(spec: Option<&str>) -> Result<Self, Message> {
        let Some(spec) = spec.map(str::trim) else {
            return Ok(Parse::Text);
        };
        match spec {
            "" | "text" => Ok(Parse::Text),
            "lines" => Ok(Parse::Lines),
            _ => {
                if let Some(path) = spec.strip_prefix("json:") {
                    return json_path(path).map(Parse::Json);
                }
                if let Some(pattern) = spec.strip_prefix("regex:") {
                    return Regex::new(pattern)
                        .map(|regex| Parse::Regex(Pattern(Arc::new(regex))))
                        .map_err(|why| {
                            util::message!(
                                "finding.not_a_pattern",
                                pattern = pattern,
                                why = Message::verbatim(why.to_string())
                            )
                        });
                }
                Err(util::message!("finding.not_a_parse", spec = spec))
            }
        }
    }

    /// The type a reading has before it is read as the source's own: a list for `lines`, text otherwise.
    pub fn yields_list(&self) -> bool {
        matches!(self, Parse::Lines)
    }

    /// Reads `output` into a raw reading: text, a list of texts, or for JSON whatever the document holds at the path.
    pub fn read(&self, output: &str) -> Result<Value, Message> {
        match self {
            Parse::Text => Ok(Value::text(output.trim())),
            Parse::Lines => Ok(Value::list(output.lines().map(Value::from))),
            Parse::Json(path) => {
                let document: serde_json::Value = serde_json::from_str(output).map_err(|why| {
                    util::message!("finding.not_json", why = Message::verbatim(why.to_string()))
                })?;
                let mut at = &document;
                for step in path {
                    at = match (step, at) {
                        (Step::Key(key), serde_json::Value::Object(object)) => object.get(key),
                        (Step::Index(index), serde_json::Value::Array(items)) => items.get(*index),
                        _ => None,
                    }
                    .ok_or_else(|| {
                        util::message!("finding.json_nothing_at", path = path_text(path))
                    })?;
                }
                json_value(at).ok_or_else(|| {
                    util::message!("finding.json_not_a_value", path = path_text(path))
                })
            }
            Parse::Regex(Pattern(regex)) => {
                let captures = regex
                    .captures(output)
                    .ok_or_else(|| util::message!("finding.no_match", pattern = regex.as_str()))?;
                let found = captures.get(1).or_else(|| captures.get(0));
                Ok(Value::text(found.map_or("", |m| m.as_str())))
            }
        }
    }
}

impl fmt::Display for Parse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Parse::Text => f.write_str("text"),
            Parse::Lines => f.write_str("lines"),
            Parse::Json(path) => write!(f, "json:{}", path_text(path)),
            Parse::Regex(pattern) => write!(f, "regex:{}", pattern.as_str()),
        }
    }
}

fn json_value(value: &serde_json::Value) -> Option<Value> {
    Some(match value {
        serde_json::Value::String(text) => Value::text(text.as_str()),
        serde_json::Value::Number(number) => Value::Number(number.as_f64()?),
        serde_json::Value::Bool(b) => Value::Bool(*b),
        serde_json::Value::Array(items) => {
            Value::list(items.iter().map(json_value).collect::<Option<Vec<_>>>()?)
        }
        serde_json::Value::Null | serde_json::Value::Object(_) => return None,
    })
}

/// `.current.temp`, `items[0].name`, `[2]`: keys separated by dots, positions in brackets.
fn json_path(path: &str) -> Result<Vec<Step>, Message> {
    let mut steps = Vec::new();
    let mut rest = path.trim();
    while !rest.is_empty() {
        if let Some(after) = rest.strip_prefix('[') {
            let (index, tail) = after
                .split_once(']')
                .ok_or_else(|| util::message!("finding.path_unclosed", path = path))?;
            let index = index.trim().parse().map_err(|_| {
                util::message!("finding.path_not_a_position", index = index, path = path)
            })?;
            steps.push(Step::Index(index));
            rest = tail;
            continue;
        }
        rest = rest.strip_prefix('.').unwrap_or(rest);
        let end = rest.find(['.', '[']).unwrap_or(rest.len());
        let key = &rest[..end];
        if key.is_empty() {
            return Err(util::message!("finding.path_empty_key", path = path));
        }
        steps.push(Step::Key(key.to_string()));
        rest = &rest[end..];
    }
    Ok(steps)
}

fn path_text(path: &[Step]) -> String {
    let mut text = String::new();
    for step in path {
        match step {
            Step::Key(key) => {
                text.push('.');
                text.push_str(key);
            }
            Step::Index(index) => text.push_str(&format!("[{index}]")),
        }
    }
    if text.is_empty() {
        text.push('.');
    }
    text
}

/// Why a raw reading is not one of the type its source was declared with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Unreadable {
    /// A text that spells no number, as it was written, cut short.
    NotNumber(String),
    /// A text that is neither true nor false, as it was written, cut short.
    NotTruth(String),
    /// A value of a type the declared one is never read from.
    OtherType,
}

/// A raw reading taken as `ty`, the type the source was declared with: a text that spells a number is that number, `"true"` is a bool, and anything can be written as text.
pub fn coerce(raw: &Value, ty: &Type) -> Result<Value, Unreadable> {
    match (ty, raw) {
        (Type::Text, Value::Text(_)) => Ok(raw.clone()),
        (Type::Text, Value::Number(n)) => Ok(Value::text(format_number(*n))),
        (Type::Text, other) => Ok(Value::text(other.to_string())),
        (Type::Number, Value::Number(_)) => Ok(raw.clone()),
        (Type::Number, Value::Text(text)) => scalar::number(text)
            .map(Value::Number)
            .map_err(|_| Unreadable::NotNumber(scalar::shown(text))),
        (Type::Bool, Value::Bool(_)) => Ok(raw.clone()),
        (Type::Bool, Value::Number(n)) => Ok(Value::Bool(*n != 0.0)),
        (Type::Bool, Value::Text(text)) => scalar::truth(text)
            .map(Value::Bool)
            .map_err(|_| Unreadable::NotTruth(scalar::shown(text))),
        (Type::List(element), Value::List(items)) => items
            .iter()
            .map(|item| coerce(item, element))
            .collect::<Result<Vec<_>, _>>()
            .map(Value::list),
        (Type::List(element), Value::Text(text)) => text
            .lines()
            .map(|line| coerce(&Value::text(line), element))
            .collect::<Result<Vec<_>, _>>()
            .map(Value::list),
        _ => Err(Unreadable::OtherType),
    }
}

/// `500ms`, `5s`, `2m` or `1h`: a whole number and a unit.
pub fn interval(text: &str) -> Result<Duration, Message> {
    let text = text.trim();
    let split = text
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(text.len());
    let (amount, unit) = text.split_at(split);
    let amount: u64 = amount
        .parse()
        .map_err(|_| util::message!("finding.interval_not_a_number", text = text))?;
    let duration = match unit.trim() {
        "ms" => Duration::from_millis(amount),
        "s" => Duration::from_secs(amount),
        "m" => Duration::from_secs(amount.saturating_mul(60)),
        "h" => Duration::from_secs(amount.saturating_mul(3600)),
        _ => {
            return Err(util::message!("finding.interval_unit", text = text));
        }
    };
    Ok(duration)
}

/// Reads an interval as [`interval`] does, raised to `min` when it is shorter — with the warning that says so, since what runs is then not what was written. `Err` when it is not an interval at all.
///
/// The one reading of an interval under `[automation] min_interval_seconds`, for a source's `every` and a rule's alike: running at the minimum is what both do, so both say it the same way.
pub fn clamp_interval(text: &str, min: Duration) -> Result<(Duration, Option<Message>), Message> {
    let every = interval(text)?;
    if every >= min {
        return Ok((every, None));
    }
    Ok((
        min,
        Some(util::message!(
            "finding.interval_raised",
            written = interval_text(every),
            least = interval_text(min)
        )),
    ))
}

/// An interval as the shortest spelling that says it, so `60s` and `1m` read the same in a report.
pub fn interval_text(every: Duration) -> String {
    let millis = every.as_millis();
    match millis {
        n if n % 3_600_000 == 0 && n > 0 => format!("{}h", n / 3_600_000),
        n if n % 60_000 == 0 && n > 0 => format!("{}m", n / 60_000),
        n if n % 1000 == 0 => format!("{}s", n / 1000),
        n => format!("{n}ms"),
    }
}
