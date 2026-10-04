//! A sentence carried as the catalogue entry it is and the values it is built from, and rendered where it is shown (DEC-27): in English by the command line, in the user's language by the running shell.
//!
//! The entry lives in the catalogue of the crate that says it, so whoever knows what went wrong keeps saying it, and the place that shows it only picks the language. [`crate::message!`] makes one and checks its key and arguments against that catalogue when the crate builds, as `t!` does.

use std::borrow::Cow;
use std::fmt;
use std::hash::{Hash, Hasher};

use telar::i18n::Catalog;
use telar_expression::HostError;

/// The language the command line answers in, and the one every catalogue falls back to.
pub const ENGLISH: &str = "en";

/// A sentence for a person to read, not yet in any language.
#[derive(Clone)]
pub struct Message(Body);

#[derive(Clone)]
enum Body {
    Keyed {
        catalog: &'static Catalog,
        key: Cow<'static, str>,
        args: Vec<(Cow<'static, str>, Arg)>,
    },
    /// Words someone else already wrote — a parser's, the system's — shown as they are.
    Verbatim(String),
}

/// What fills one placeholder: a value, or a sentence of its own rendered in the same language.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Arg {
    Text(String),
    Message(Message),
}

impl Arg {
    fn render_in(&self, locale: &str) -> String {
        match self {
            Arg::Text(text) => text.clone(),
            Arg::Message(message) => message.render_in(locale),
        }
    }

    /// As it fills a placeholder: a value is often somebody else's text — an id a bundle chose, a command it runs — so it is written out where it could disguise the sentence around it ([`crate::text::shown`]).
    fn shown_in(&self, locale: &str) -> String {
        match self {
            Arg::Text(text) => crate::text::shown(text),
            Arg::Message(message) => message.render_in(locale),
        }
    }
}

/// What a message's argument can be made from: anything that prints, or another message.
pub trait IntoArg {
    fn into_arg(self) -> Arg;
}

impl<T: fmt::Display> IntoArg for T {
    fn into_arg(self) -> Arg {
        Arg::Text(self.to_string())
    }
}

impl IntoArg for Message {
    fn into_arg(self) -> Arg {
        Arg::Message(self)
    }
}

impl IntoArg for Arg {
    fn into_arg(self) -> Arg {
        self
    }
}

impl IntoArg for &Message {
    fn into_arg(self) -> Arg {
        Arg::Message(self.clone())
    }
}

impl IntoArg for &Arg {
    fn into_arg(self) -> Arg {
        self.clone()
    }
}

impl Message {
    /// The entry `key` of `catalog`, with no arguments yet. [`crate::message!`] is the checked way to write one; this is for a key known only at run time.
    pub fn new(catalog: &'static Catalog, key: impl Into<Cow<'static, str>>) -> Self {
        Self(Body::Keyed {
            catalog,
            key: key.into(),
            args: Vec::new(),
        })
    }

    /// Words that are not the shell's to translate: a parser's own report, an error the system gave.
    pub fn verbatim(text: impl Into<String>) -> Self {
        Self(Body::Verbatim(text.into()))
    }

    /// This message with `value` filling the placeholder `name`.
    pub fn arg(mut self, name: impl Into<Cow<'static, str>>, value: impl IntoArg) -> Self {
        if let Body::Keyed { args, .. } = &mut self.0 {
            args.push((name.into(), value.into_arg()));
        }
        self
    }

    /// The catalogue entry, or `None` for words shown as they are.
    pub fn key(&self) -> Option<&str> {
        match &self.0 {
            Body::Keyed { key, .. } => Some(key),
            Body::Verbatim(_) => None,
        }
    }

    /// What fills the placeholder `name`.
    pub fn argument(&self, name: &str) -> Option<&Arg> {
        match &self.0 {
            Body::Keyed { args, .. } => args
                .iter()
                .find(|(held, _)| held == name)
                .map(|(_, arg)| arg),
            Body::Verbatim(_) => None,
        }
    }

    /// In the language this thread speaks, read reactively: inside a widget's content it follows a language switch, as `t!` does.
    pub fn render(&self) -> String {
        let active = telar::i18n::use_locale();
        self.render_in(active.as_deref().unwrap_or(ENGLISH))
    }

    /// In `locale`, or the catalogue's own language where it has no text for this entry in it. An entry the catalogue lacks is shown as its key, which is visible rather than silent.
    ///
    /// What no catalogue wrote is written out where it could disguise what is around it: a value filling a placeholder as [`crate::text::shown`] does, words shown as they are as [`crate::text::shown_lines`] does, which keeps a parser's report on its lines.
    pub fn render_in(&self, locale: &str) -> String {
        match &self.0 {
            Body::Verbatim(text) => crate::text::shown_lines(text),
            Body::Keyed { catalog, key, args } => {
                let rendered: Vec<(&str, String)> = args
                    .iter()
                    .map(|(name, arg)| (name.as_ref(), arg.shown_in(locale)))
                    .collect();
                let pairs: Vec<(&str, &str)> = rendered
                    .iter()
                    .map(|(name, value)| (*name, value.as_str()))
                    .collect();
                catalog
                    .message(key, locale)
                    .map(|message| message.select(locale, &pairs).render(&pairs))
                    .unwrap_or_else(|| key.to_string())
            }
        }
    }

    /// What the command line says.
    pub fn english(&self) -> String {
        self.render_in(ENGLISH)
    }

    /// Whether this entry, and every entry filling one of its placeholders, has text of its own in `locale` rather than falling back. Words shown as they are have none of their own; filling a placeholder, they are a value like any other.
    pub fn is_translated_in(&self, locale: &str) -> bool {
        match &self.0 {
            Body::Verbatim(_) => false,
            Body::Keyed { catalog, key, args } => {
                has_own_text(catalog, key, locale)
                    && args.iter().all(|(_, arg)| match arg {
                        Arg::Message(message) if message.key().is_some() => {
                            message.is_translated_in(locale)
                        }
                        _ => true,
                    })
            }
        }
    }

    /// This message as the failure a host hands `telar-expression`: the key, the arguments in the order the catalogue's own language writes its placeholders, and the English. [`Message::from_host_error`] reads it back.
    ///
    /// A host error carries text, so a message filling a placeholder travels as its English: what a host reports this way is names and values, not sentences.
    pub fn host_error(&self) -> HostError {
        match &self.0 {
            Body::Verbatim(text) => HostError::new("", text.clone()),
            Body::Keyed { catalog, key, .. } => placeholders(catalog, key).into_iter().fold(
                HostError::new(key.to_string(), self.english()),
                |error, name| {
                    error.arg(
                        self.argument(name)
                            .map(|arg| arg.render_in(ENGLISH))
                            .unwrap_or_default(),
                    )
                },
            ),
        }
    }

    /// The message a host error made by [`Message::host_error`] from an entry of `catalog` carries; its English words where `catalog` has no such entry.
    pub fn from_host_error(catalog: &'static Catalog, error: &HostError) -> Self {
        if !catalog.contains(&error.key) {
            return Self::verbatim(error.message.clone());
        }
        placeholders(catalog, &error.key)
            .into_iter()
            .zip(&error.args)
            .fold(
                Self::new(catalog, error.key.clone()),
                |message, (name, value)| message.arg(name, value.as_str()),
            )
    }
}

/// The placeholders of `key` in the catalogue's own language, each once, in the order it writes them.
fn placeholders(catalog: &Catalog, key: &str) -> Vec<&'static str> {
    let mut names: Vec<&'static str> = Vec::new();
    if let Some(message) = catalog.message(key, catalog.default_locale) {
        for name in message.arg_names() {
            if !names.contains(&name) {
                names.push(name);
            }
        }
    }
    names
}

fn has_own_text(catalog: &Catalog, key: &str, locale: &str) -> bool {
    catalog
        .entries
        .iter()
        .find(|entry| entry.key == key)
        .is_some_and(|entry| entry.messages.iter().any(|(held, _)| *held == locale))
}

/// The keys under any of `prefixes` that `catalog` has no text for in one of its languages, each with that language: what a test asserts is empty, so a message the shell shows is never English in a Spanish session.
pub fn untranslated(catalog: &Catalog, prefixes: &[&str]) -> Vec<String> {
    catalog
        .entries
        .iter()
        .filter(|entry| prefixes.iter().any(|prefix| entry.key.starts_with(prefix)))
        .flat_map(|entry| {
            catalog
                .locales
                .iter()
                .filter(|locale| !entry.messages.iter().any(|(held, _)| held == *locale))
                .map(|locale| format!("{} ({locale})", entry.key))
        })
        .collect()
}

impl fmt::Debug for Message {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0 {
            Body::Verbatim(text) => write!(f, "{text:?}"),
            Body::Keyed { key, args, .. } => {
                let mut tuple = f.debug_tuple(key);
                for (name, arg) in args {
                    tuple.field(&format_args!("{name} = {arg:?}"));
                }
                tuple.finish()
            }
        }
    }
}

impl PartialEq for Message {
    fn eq(&self, other: &Self) -> bool {
        match (&self.0, &other.0) {
            (Body::Verbatim(one), Body::Verbatim(other)) => one == other,
            (
                Body::Keyed { catalog, key, args },
                Body::Keyed {
                    catalog: other_catalog,
                    key: other_key,
                    args: other_args,
                },
            ) => std::ptr::eq(*catalog, *other_catalog) && key == other_key && args == other_args,
            _ => false,
        }
    }
}

impl Eq for Message {}

impl Hash for Message {
    fn hash<H: Hasher>(&self, state: &mut H) {
        match &self.0 {
            Body::Verbatim(text) => text.hash(state),
            Body::Keyed { catalog, key, args } => {
                std::ptr::hash(*catalog, state);
                key.hash(state);
                args.hash(state);
            }
        }
    }
}

/// A [`Message`] from an entry of the calling crate's catalogue: `message!("finding.unknown_tab", id = id)`.
///
/// The key and the argument names are checked against that catalogue when the crate builds, as `t!` checks them, so a misspelt key is a build error rather than a key shown to the user. An argument is anything that prints, or another `Message`.
// `crate` is the calling crate on purpose: the entry is in its catalogue, not in this one.
#[allow(clippy::crate_in_macro_def)]
#[macro_export]
macro_rules! message {
    ($key:literal $(, $name:ident = $value:expr)* $(,)?) => {{
        // `t!` is the build-time check of the key and the names; it is never run.
        if false {
            let _ = ::telar::t!($key $(, $name = "")*);
        }
        $crate::report::Message::new(&crate::__rsx_i18n::CATALOG, $key)
            $(.arg(stringify!($name), $value))*
    }};
}
