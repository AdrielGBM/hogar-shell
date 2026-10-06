//! The expression field: one line of the shell's expression language, checked and evaluated as it is typed, inside a popover that previews what it drives.
//!
//! **What it shows.** Under the line, the value the expression has right now and its type, read through the same environment the layer window binds it in — the lock screen's view on the lock layer (TA-8) — for as long as the popover is open and its window on screen. A mistake is underlined where it is, with its message under the line; an evaluation error is underlined too, while the last value it had stays shown.
//!
//! **What it writes.** Only what checks: the field hands its owner the text each time it compiles to the type wanted, or `None` while it does not, which the owner reads as "put back what the layout wrote". So the popover's one transaction never holds a broken expression, and the field keeps the text and says what is wrong with it.
//!
//! **Typing help.** Completions follow the word at the caret: `$` and a name offer the readings — module sources, the layout's sources, variables, events — and a bare word offers functions and constants as well. The arrows walk them, Enter or Tab takes one, Esc closes them. The source browser under the field lists every reading with what it reads now, and a press puts one in at the caret.
//!
//! **Keys.** Enter commits the popover, as it does from any of its controls (F-7). Esc in the field puts its text back as the popover opened it and hands the keyboard back, so the next Esc reverts the popover.

use std::cell::Cell;
use std::rc::Rc;

use telar::{
    AlignItems, Children, Container, Input, Key, LayoutStyle, ModifiersState, NamedKey,
    ReactiveList, RectStyle, RwSignal, SizeDimension, StyledContainer, Text, Underline, box_item,
    effect, focus, memo, signal, use_theme,
};
use telar_expression::{Compiled, Error, Reference, Resolver, Type, Value};

use automation::{Environment, Gate, Local, Readings, ReferenceKind};
use config::theme::{FontRole, NordTheme};
use layout::{LayerKind, Mistake};
use surfaces::layer_window::LayerWindowContext;
use surfaces::rects::{Node, Part};
use ui::descriptor::Built;
use util::report::Message;

/// How many completions are offered at once.
const OFFERED: usize = 8;

/// How much of a long text reading a list shows.
const SHOWN_CHARS: usize = 40;

/// What a field hands the text it checked to: the text where it checks (empty for no expression), `None` where it does not.
pub(crate) type Checks = dyn Fn(Option<&str>);

/// One expression field, and what it is for.
pub(crate) struct Field {
    /// What the layout wrote when the popover opened, which the field starts at and Esc puts back.
    pub(crate) seed: String,
    /// What the expression has to give, read reactively: an instance's binding changes it with its target.
    pub(crate) expected: Rc<dyn Fn() -> Wanted>,
    /// What it reads: the shell's names on its layer, and `$item` and `$index` in a child of a repeated group.
    pub(crate) env: Environment,
    /// What an empty field means where it is: shown always, not bound.
    pub(crate) empty: Rc<dyn Fn() -> String>,
    /// Told each time the text is checked again.
    pub(crate) checked: Rc<Checks>,
}

/// What a layer's expressions read: the running shell's names, and on the lock layer what the lock screen may show (TA-8).
pub(crate) fn environment(layer: LayerKind) -> Environment {
    Environment::running().on_layer(layer == LayerKind::Lock)
}

/// What the bindings of the instance at `node` read: its layer's names and, on a child of a repeated group, the copy's own `$item` and `$index`, typed as `locals` says and read as the screen draws the copy — the one the popover was opened on, or the first one for the template, so what a popover opened on the template shows is what its first copy reads.
pub(crate) fn instance_environment(node: &Node, locals: &layout::Locals) -> Environment {
    let env = environment(node.layer);
    let Part::Instance(group, id) = &node.part else {
        return env.with_locals(Local::typed_all(locals));
    };
    let (group, index) = (node.group(group), id.copy_index().unwrap_or(0));
    let item = move || surfaces::expressions::drawn_item(&group, index);
    let read = locals
        .iter()
        .map(|(name, ty)| {
            let item = item.clone();
            match name {
                layout::ITEM => Local::live(name, ty.clone(), item),
                layout::INDEX => Local::live(name, ty.clone(), move || {
                    item().map(|_| Value::Number(index as f64))
                }),
                _ => Local::typed(name, ty.clone()),
            }
        })
        .collect();
    env.with_locals(read)
}

/// Whether what the field shows is on screen, read reactively: its popover open, in a window that is mapped.
fn on_screen() -> Rc<dyn Fn() -> bool> {
    let open = crate::popover::showing();
    let window = LayerWindowContext::current().map(|window| window.mapped);
    Rc::new(move || {
        open.is_none_or(|open| open.is_alive() && open.get())
            && window.is_none_or(|mapped| mapped.get())
    })
}

/// What an expression has to give.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Wanted {
    Exactly(Type),
    /// A list of anything: what a group repeats over.
    AnyList,
}

/// What a text is as an expression giving what is wanted.
pub(crate) enum Checked {
    Empty,
    Valid(Compiled),
    /// It reads a variable not set yet, and is fine otherwise as far as can be told: written, as `layout check` only warns of it, and checked again once the variable is set.
    Awaiting(Vec<Mistake>),
    Invalid(Vec<Mistake>),
}

/// `text` checked against `env` to give what is wanted. Checked inside an effect or a memo, it is checked again as a variable it names appears, goes or changes type.
pub(crate) fn check(env: &Environment, text: &str, wanted: &Wanted) -> Checked {
    if text.trim().is_empty() {
        return Checked::Empty;
    }
    let compiled = match env.compile(text) {
        Ok(compiled) => compiled,
        Err(errors) => {
            let waits = errors.iter().all(|error| env.awaits_variable(text, error));
            let mistakes = errors.iter().map(mistake).collect();
            return match waits {
                true => Checked::Awaiting(mistakes),
                false => Checked::Invalid(mistakes),
            };
        }
    };
    let fits = match wanted {
        Wanted::Exactly(ty) => compiled.require(ty).map_err(|error| mistake(&error)),
        Wanted::AnyList => match layout::repeat_item(compiled.ty()) {
            Ok(_) => Ok(compiled),
            Err(why) => Err(Mistake {
                kind: telar_expression::ErrorKind::Type,
                span: telar_expression::Span::new(0, text.len()),
                message: why,
            }),
        },
    };
    match fits {
        Ok(compiled) => Checked::Valid(compiled),
        Err(error) => Checked::Invalid(vec![error]),
    }
}

/// What `error` is, in the shell's words, at the bytes it is about.
fn mistake(error: &Error) -> Mistake {
    Mistake {
        kind: error.kind,
        span: error.span,
        message: automation::env::describe(&error.code),
    }
}

/// What a completion offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Kind {
    /// What a copy of a repeated group's child reads of its own: `$item`, `$index`.
    Local,
    Source,
    Var,
    Event,
    Function,
    Constant,
}

impl Kind {
    fn reads(self) -> bool {
        matches!(self, Kind::Local | Kind::Source | Kind::Var | Kind::Event)
    }

    fn said(self) -> String {
        match self {
            Kind::Local => telar::t!("editor.expr.kind.local"),
            Kind::Source => telar::t!("editor.expr.kind.source"),
            Kind::Var => telar::t!("editor.expr.kind.var"),
            Kind::Event => telar::t!("editor.expr.kind.event"),
            Kind::Function => telar::t!("editor.expr.kind.function"),
            Kind::Constant => telar::t!("editor.expr.kind.constant"),
        }
    }
}

/// Something an expression can name.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Candidate {
    pub(crate) kind: Kind,
    /// What is matched against the word typed: `battery.level`, `len`, `pi`.
    pub(crate) name: String,
    /// Its type, or a function's signatures.
    pub(crate) detail: String,
    pub(crate) summary: String,
    pub(crate) reference: Option<Reference>,
}

impl Candidate {
    /// What taking it writes in place of the word.
    pub(crate) fn written(&self) -> String {
        match self.kind {
            Kind::Local | Kind::Source | Kind::Var | Kind::Event => format!("${}", self.name),
            Kind::Function => format!("{}(", self.name),
            Kind::Constant => self.name.clone(),
        }
    }
}

/// Everything an expression can name in `env`: its readings, as what each one reads there, then its functions and constants.
pub(crate) fn candidates(env: &Environment) -> Vec<Candidate> {
    let mut all: Vec<Candidate> = env
        .references()
        .into_iter()
        .map(|(reference, ty, reads)| {
            let kind = match reads {
                ReferenceKind::Local => Kind::Local,
                ReferenceKind::Source | ReferenceKind::Field | ReferenceKind::Theme => Kind::Source,
                ReferenceKind::Var => Kind::Var,
                ReferenceKind::Event => Kind::Event,
            };
            Candidate {
                kind,
                name: reference.dotted(),
                detail: ty.to_string(),
                summary: String::new(),
                reference: Some(reference),
            }
        })
        .collect();
    let registry = env.registry();
    for function in registry.functions() {
        let form = format!("{}{}", function.name(), function.signature());
        match all
            .iter_mut()
            .find(|held| held.kind == Kind::Function && held.name == function.name())
        {
            Some(held) => {
                held.detail = format!("{}  {form}", held.detail);
                if held.summary.is_empty() {
                    held.summary = function.summary().to_string();
                }
            }
            None => all.push(Candidate {
                kind: Kind::Function,
                name: function.name().to_string(),
                detail: form,
                summary: function.summary().to_string(),
                reference: None,
            }),
        }
    }
    all.extend(registry.constants().map(|constant| Candidate {
        kind: Kind::Constant,
        name: constant.name.clone(),
        detail: constant.value.type_of().to_string(),
        summary: constant.summary.clone(),
        reference: None,
    }));
    all
}

/// The word a completion would replace: from its start to the end of the name the caret is in, and what is typed of it up to the caret.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Word {
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) typed: String,
    /// Whether it is a reading's, written after `$`.
    pub(crate) reading: bool,
}

fn is_name(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '.'
}

/// The word at `caret` in `text`, if one is being written there: not inside a text literal, a colour or a number.
pub(crate) fn word_at(text: &str, caret: usize) -> Option<Word> {
    let mut caret = caret.min(text.len());
    while !text.is_char_boundary(caret) {
        caret -= 1;
    }
    let start = text[..caret]
        .char_indices()
        .rev()
        .take_while(|(_, c)| is_name(*c))
        .last()
        .map_or(caret, |(at, _)| at);
    let end = text[caret..]
        .char_indices()
        .find(|(_, c)| !is_name(*c))
        .map_or(text.len(), |(at, _)| caret + at);
    let before = text[..start].chars().next_back();
    if before == Some('#') || in_text(&text[..start]) {
        return None;
    }
    let typed = text[start..caret].to_string();
    let reading = before == Some('$');
    if !reading && (typed.is_empty() || typed.starts_with(|c: char| c.is_ascii_digit())) {
        return None;
    }
    let start = if reading { start - 1 } else { start };
    Some(Word {
        start,
        end,
        typed,
        reading,
    })
}

/// Whether `before` leaves a text literal open.
fn in_text(before: &str) -> bool {
    let mut open: Option<char> = None;
    let mut escaped = false;
    for c in before.chars() {
        match open {
            Some(_) if escaped => escaped = false,
            Some(_) if c == '\\' => escaped = true,
            Some(quote) if c == quote => open = None,
            Some(_) => {}
            None if c == '"' || c == '\'' => open = Some(c),
            None => {}
        }
    }
    open.is_some()
}

/// How well `name` matches what is typed: from its start, from the start of one of its dotted parts, or somewhere inside; `None` when not at all. Case is ignored, as function names ignore it.
fn closeness(name: &str, typed: &str) -> Option<u8> {
    let (name, typed) = (name.to_lowercase(), typed.to_lowercase());
    if name.starts_with(&typed) {
        return Some(0);
    }
    if name.split('.').skip(1).any(|part| part.starts_with(&typed)) {
        return Some(1);
    }
    name.contains(&typed).then_some(2)
}

/// The candidates that match `word`, best first: a match from the start beats one from a dotted part, which beats one inside; then a copy's own `$item` and `$index`, readings, variables, events, functions and constants, in that order; then the shorter name, then the alphabet. After `$` only readings are offered.
pub(crate) fn ranked<'a>(all: &'a [Candidate], word: &Word) -> Vec<&'a Candidate> {
    let mut matched: Vec<(u8, &Candidate)> = all
        .iter()
        .filter(|candidate| !word.reading || candidate.kind.reads())
        .filter_map(|candidate| Some((closeness(&candidate.name, &word.typed)?, candidate)))
        .collect();
    matched.sort_by(|(a, x), (b, y)| {
        (a, x.kind, x.name.len(), &x.name).cmp(&(b, y.kind, y.name.len(), &y.name))
    });
    matched
        .into_iter()
        .map(|(_, candidate)| candidate)
        .collect()
}

/// What is offered for `word`: nothing when the one match is what is typed already, so Enter is left to commit.
fn offered<'a>(all: &'a [Candidate], word: &Word) -> Vec<&'a Candidate> {
    let mut ranked = ranked(all, word);
    if let [only] = ranked.as_slice()
        && only.name.eq_ignore_ascii_case(&word.typed)
    {
        return Vec::new();
    }
    ranked.truncate(OFFERED);
    ranked
}

/// `text` with `candidate` written in place of `word`, and where the caret goes after it: past the opening bracket of a call, reusing one already there.
pub(crate) fn complete(text: &str, word: &Word, candidate: &Candidate) -> (String, usize) {
    let mut written = candidate.written();
    let bracket_follows = text[word.end..].starts_with('(');
    if candidate.kind == Kind::Function && bracket_follows {
        written.pop();
    }
    let mut after = String::with_capacity(text.len() + written.len());
    after.push_str(&text[..word.start]);
    after.push_str(&written);
    after.push_str(&text[word.end..]);
    let mut caret = word.start + written.len();
    if candidate.kind == Kind::Function && bracket_follows {
        caret += 1;
    }
    (after, caret)
}

/// A value as the field shows it: a text quoted and cut short, anything else as the language prints it.
pub(crate) fn shown(value: &Value) -> String {
    match value {
        Value::Text(text) => format!("\"{}\"", util::text::clipped(text, SHOWN_CHARS)),
        other => other.to_string(),
    }
}

/// What `reading` reads now, as a list shows it.
fn reading_of(readings: &Readings, reference: &Reference) -> String {
    match readings.read(reference) {
        Ok(value) => shown(&value),
        Err(_) => "—".to_string(),
    }
}

/// The field: the line, what it gives now, the completions for the word at the caret, and the source browser.
pub(crate) fn field(spec: Field) -> Built {
    let Field {
        seed,
        expected,
        env,
        empty,
        checked,
    } = spec;
    let theme = use_theme::<NordTheme>();
    let text = signal(seed.clone());
    let compile_errors: RwSignal<Vec<Mistake>> = signal(Vec::new());
    let awaiting: RwSignal<Vec<Mistake>> = signal(Vec::new());
    let evaluation_error: RwSignal<Option<Error>> = signal(None);
    let closed_at: RwSignal<Option<String>> = signal(None);
    let selected = signal(0usize);
    let browsing = signal(false);
    let all = Rc::new(candidates(&env));
    let seen = on_screen();

    {
        let (env, expected) = (env.clone(), Rc::clone(&expected));
        let seeded = Cell::new(false);
        effect(move || {
            let now = text.get();
            let checking = check(&env, &now, &expected());
            let (errors, waiting) = match &checking {
                Checked::Invalid(errors) => (errors.clone(), Vec::new()),
                Checked::Awaiting(errors) => (Vec::new(), errors.clone()),
                _ => (Vec::new(), Vec::new()),
            };
            if compile_errors.peek_with(|held| *held != errors) {
                compile_errors.set(errors);
            }
            if awaiting.peek_with(|held| *held != waiting) {
                awaiting.set(waiting);
            }
            if !seeded.replace(true) {
                return;
            }
            selected.set(0);
            match checking {
                Checked::Invalid(_) => checked(None),
                Checked::Empty => checked(Some("")),
                Checked::Valid(_) | Checked::Awaiting(_) => checked(Some(&now)),
            }
        });
    }

    let input = Input::declaring(
        text,
        LayoutStyle::new().height(theme.font(FontRole::Body) * 1.4),
        move |inherited| {
            inherited
                .with_font_size(theme.font(FontRole::Body))
                .with_color(theme.text)
        },
    )?
    .placeholder(telar::t!("editor.expr.placeholder"))
    .underline(move || {
        let compiled = compile_errors.with(|errors| {
            errors
                .iter()
                .map(|error| Underline {
                    range: error.span.range(),
                    color: theme.error,
                })
                .collect::<Vec<_>>()
        });
        let evaluated = evaluation_error.with(|error| error.as_ref().map(|error| error.span));
        let warned = awaiting.with(|errors| {
            errors
                .iter()
                .map(|error| error.span)
                .chain(evaluated)
                .map(|span| Underline {
                    range: span.range(),
                    color: theme.warning,
                })
                .collect::<Vec<_>>()
        });
        compiled.into_iter().chain(warned).collect()
    });
    let caret = input.caret();
    let id = input.focus_id();

    let offers = {
        let all = Rc::clone(&all);
        memo(move || {
            let now = text.get();
            let closed = closed_at.with(|closed| closed.as_deref() == Some(now.as_str()));
            if closed || !focus::is_focused(id) {
                return Vec::new();
            }
            let Some(word) = word_at(&now, caret.get()) else {
                return Vec::new();
            };
            offered(&all, &word)
                .into_iter()
                .filter_map(|candidate| all.iter().position(|held| held == candidate))
                .collect::<Vec<usize>>()
        })
    };
    let take: Rc<dyn Fn(usize)> = {
        let all = Rc::clone(&all);
        Rc::new(move |index: usize| {
            let Some(candidate) = all.get(index) else {
                return;
            };
            let now = text.peek();
            let Some(word) = word_at(&now, caret.peek()) else {
                return;
            };
            let (after, at) = complete(&now, &word, candidate);
            closed_at.set(Some(after.clone()));
            text.set(after);
            caret.set(at);
            focus::request(id);
        })
    };
    let insert: Rc<dyn Fn(&str)> = Rc::new(move |written: &str| {
        let now = text.peek();
        let mut at = caret.peek().min(now.len());
        while !now.is_char_boundary(at) {
            at -= 1;
        }
        let mut after = now.clone();
        after.insert_str(at, written);
        closed_at.set(Some(after.clone()));
        text.set(after);
        caret.set(at + written.len());
        focus::request(id);
    });

    let taking = Rc::clone(&take);
    let seeded_text = seed.clone();
    let input = input
        .on_key(move |key: &Key, _: ModifiersState| {
            let showing = offers.with(|offers| offers.len());
            if showing == 0 {
                return false;
            }
            match key {
                Key::Named(NamedKey::ArrowDown) => selected.set((selected.peek() + 1) % showing),
                Key::Named(NamedKey::ArrowUp) => {
                    selected.set((selected.peek() + showing - 1) % showing)
                }
                Key::Named(NamedKey::Enter | NamedKey::Tab) => {
                    let at = selected.peek().min(showing - 1);
                    if let Some(index) = offers.with(|offers| offers.get(at).copied()) {
                        taking(index);
                    }
                }
                Key::Named(NamedKey::Escape) => closed_at.set(Some(text.peek())),
                _ => return false,
            }
            true
        })
        .on_submit(|| {
            telar::confirm_top();
        })
        .on_cancel(move || {
            if text.peek_with(|now| *now != seeded_text) {
                text.set(seeded_text.clone());
            }
        });

    let line = StyledContainer::new(
        LayoutStyle::new()
            .flex_column()
            .width(SizeDimension::Percent(1.0))
            .padding_horizontal(ui::scale::space::md())
            .padding_vertical(ui::scale::space::sm()),
        move |_| {
            let stroke = match compile_errors.with(Vec::is_empty) {
                true => theme.base,
                false => theme.error,
            };
            RectStyle::filled(theme.base, ui::scale::corner::md())
                .with_border(telar::Border::uniform(stroke, 1.0))
        },
        vec![box_item(input)],
    )?;

    let live = live_line(
        &env,
        text,
        Rc::clone(&expected),
        Gate::new({
            let seen = Rc::clone(&seen);
            move || seen()
        }),
        empty,
        evaluation_error,
    )?;
    let readings = Rc::new(env.readings(Gate::new(move || {
        seen() && (browsing.get() || offers.with(|offers| !offers.is_empty()))
    })));
    let completions = completions(
        Rc::clone(&all),
        offers,
        selected,
        take,
        Rc::clone(&readings),
    )?;
    let browser = browser(all, browsing, insert, readings)?;

    Ok(box_item(Container::new(
        LayoutStyle::new()
            .flex_column()
            .gap(ui::scale::space::xs())
            .width(SizeDimension::Percent(1.0)),
        vec![box_item(line), live, completions, browser],
    )?))
}

/// The line under the field: what the expression gives now and its type, or what is wrong with it, in the language the shell speaks.
fn live_line(
    env: &Environment,
    text: RwSignal<String>,
    expected: Rc<dyn Fn() -> Wanted>,
    gate: Gate,
    empty: Rc<dyn Fn() -> String>,
    evaluation_error: RwSignal<Option<Error>>,
) -> Built {
    let (env, checking) = (env.clone(), env.clone());
    let theme = use_theme::<NordTheme>();
    let list = ReactiveList::with_style(
        LayoutStyle::new().width(SizeDimension::Percent(1.0)),
        move || {
            let (written, wanted) = (text.get(), expected());
            let valid = matches!(check(&checking, &written, &wanted), Checked::Valid(_));
            vec![(written, wanted, valid)]
        },
        |checked: &(String, Wanted, bool)| checked.clone(),
        move |(written, wanted, _): (String, Wanted, bool)| {
            let gives: RwSignal<Option<String>> = signal(None);
            let wrong: RwSignal<Option<Message>> = signal(None);
            let failing = signal(false);
            let waiting = signal(false);
            match check(&env, &written, &wanted) {
                Checked::Empty => {
                    evaluation_error.set(None);
                    gives.set(Some(empty()));
                }
                Checked::Invalid(errors) => {
                    evaluation_error.set(None);
                    failing.set(true);
                    wrong.set(first_message(&errors));
                }
                Checked::Awaiting(errors) => {
                    evaluation_error.set(None);
                    waiting.set(true);
                    wrong.set(first_message(&errors));
                }
                Checked::Valid(compiled) => {
                    let ty = compiled.ty().clone();
                    let held = env.bind(compiled, gate.clone());
                    effect(move || {
                        let held = held.get();
                        gives.set(held.value.as_ref().map(|value| {
                            telar::t!(
                                "editor.expr.value",
                                value = shown(value),
                                ty = Message::type_name(&ty).render()
                            )
                        }));
                        wrong.set(
                            held.error
                                .as_ref()
                                .map(|error| automation::env::describe(&error.code)),
                        );
                        let waits = held
                            .error
                            .as_ref()
                            .is_some_and(automation::env::awaits_reading);
                        failing.set(held.value.is_none() && held.error.is_some() && !waits);
                        waiting.set(waits);
                        if evaluation_error.peek_with(|now| *now != held.error) {
                            evaluation_error.set(held.error.clone());
                        }
                    });
                }
            }
            Ok(box_item(Text::new(
                move || said(gives.get(), wrong.get()),
                LayoutStyle::new().width(SizeDimension::Percent(1.0)),
                move || {
                    let ink = match (failing.get(), waiting.get()) {
                        (true, _) => theme.error,
                        (false, true) => theme.warning,
                        (false, false) => theme.subtle,
                    };
                    theme.text_style(FontRole::Caption, ink)
                },
            )?))
        },
    )?;
    Ok(Box::new(list))
}

/// The live line's words: what the expression gives, what is wrong with it, or both, with what is wrong rendered here so a language switch reaches it.
fn said(gives: Option<String>, wrong: Option<Message>) -> String {
    match (gives, wrong) {
        (Some(value), None) => value,
        (Some(value), Some(wrong)) => format!("{value} — {}", wrong.render()),
        (None, Some(wrong)) => wrong.render(),
        (None, None) => telar::t!("editor.expr.waiting"),
    }
}

fn first_message(errors: &[Mistake]) -> Option<Message> {
    errors.first().map(|error| error.message.clone())
}

/// The completions for the word at the caret, the chosen one marked.
fn completions(
    all: Rc<Vec<Candidate>>,
    offers: telar::Memo<Vec<usize>>,
    selected: RwSignal<usize>,
    take: Rc<dyn Fn(usize)>,
    readings: Rc<Readings>,
) -> Built {
    let list = ReactiveList::with_style(
        LayoutStyle::new()
            .flex_column()
            .width(SizeDimension::Percent(1.0)),
        move || offers.get(),
        |index: &usize| *index,
        move |index: usize| {
            let candidate = all[index].clone();
            let chosen = move || {
                offers.with(|offers| offers.iter().position(|held| *held == index))
                    == Some(selected.get())
            };
            let take = Rc::clone(&take);
            row(
                &candidate,
                Rc::clone(&readings),
                Rc::new(chosen),
                Rc::new(move || take(index)),
            )
        },
    )?;
    Ok(Box::new(list))
}

/// Every reading there is, with what it reads now, folded away under a heading until opened.
fn browser(
    all: Rc<Vec<Candidate>>,
    browsing: RwSignal<bool>,
    insert: Rc<dyn Fn(&str)>,
    readings: Rc<Readings>,
) -> Built {
    let count = all
        .iter()
        .filter(|candidate| candidate.kind.reads())
        .count();
    let heading = telar::button(
        telar::ButtonProps::props()
            .label(telar::Reactive::of(move || {
                let marker = match browsing.get() {
                    true => "▾",
                    false => "▸",
                };
                format!(
                    "{marker} {}",
                    telar::t!("editor.expr.sources", count = count)
                )
            }))
            .ghost(true)
            .on_press(Rc::new(move || browsing.set(!browsing.peek())))
            .build(),
        Children::default(),
    )?;
    let listed: Vec<usize> = all
        .iter()
        .enumerate()
        .filter(|(_, candidate)| candidate.kind.reads())
        .map(|(index, _)| index)
        .collect();
    let rows = ReactiveList::with_style(
        LayoutStyle::new()
            .flex_column()
            .width(SizeDimension::Percent(1.0)),
        move || match browsing.get() {
            true => listed.clone(),
            false => Vec::new(),
        },
        |index: &usize| *index,
        move |index: usize| {
            let candidate = all[index].clone();
            let written = candidate.written();
            let insert = Rc::clone(&insert);
            row(
                &candidate,
                Rc::clone(&readings),
                Rc::new(|| false),
                Rc::new(move || insert(&written)),
            )
        },
    )?;
    Ok(box_item(Container::new(
        LayoutStyle::new()
            .flex_column()
            .width(SizeDimension::Percent(1.0)),
        vec![heading, Box::new(rows)],
    )?))
}

/// One thing that can be named: what it is called, its type or signature, what it reads now where it is a reading, and what it is for; a press takes it.
fn row(
    candidate: &Candidate,
    readings: Rc<Readings>,
    chosen: Rc<dyn Fn() -> bool>,
    take: Rc<dyn Fn()>,
) -> Built {
    let theme = use_theme::<NordTheme>();
    let now = signal(String::new());
    if let Some(reference) = candidate.reference.clone() {
        effect(move || now.set(reading_of(&readings, &reference)));
    }
    let name = format!("{}  {}", candidate.written(), candidate.kind.said());
    let detail = match candidate.summary.is_empty() {
        true => candidate.detail.clone(),
        false => format!("{} — {}", candidate.detail, candidate.summary),
    };
    let title = Text::new(
        move || name.clone(),
        LayoutStyle::new().flex_grow(1.0),
        move || theme.text_style(FontRole::Caption, theme.text),
    )?;
    let reading = Text::new(
        move || now.get(),
        LayoutStyle::new(),
        move || theme.text_style(FontRole::Caption, theme.accent),
    )?;
    let head = Container::new(
        LayoutStyle::new()
            .flex_row()
            .align_items(AlignItems::CENTER)
            .gap(ui::scale::space::sm())
            .width(SizeDimension::Percent(1.0)),
        vec![box_item(title), box_item(reading)],
    )?;
    let about = Text::new(
        move || detail.clone(),
        LayoutStyle::new().width(SizeDimension::Percent(1.0)),
        move || theme.text_style(FontRole::Caption, theme.subtle),
    )?;
    let radius = ui::scale::corner::xs();
    Ok(box_item(
        StyledContainer::new(
            LayoutStyle::new()
                .flex_column()
                .width(SizeDimension::Percent(1.0))
                .padding_horizontal(ui::scale::space::sm())
                .padding_vertical(ui::scale::space::xs()),
            move |_| match chosen() {
                true => RectStyle::filled(theme.overlay, radius),
                false => RectStyle::filled(telar::Color::TRANSPARENT, radius),
            },
            vec![box_item(head), box_item(about)],
        )?
        .on_press(move || take()),
    ))
}
