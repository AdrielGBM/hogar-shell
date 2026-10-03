//! What is wrong with the rules a config writes, found without running any of them, and each rule made ready to run when nothing that stops it is wrong.

use std::collections::BTreeSet;
use std::ops::Range;
use std::time::Duration;

use config::{AutomationConfig, RuleConfig, RuleStore, RuleTrigger};
use services::events::EventKind;
use telar_expression::{Compiled, Reference, Type};
use util::report::Message;

use super::schedule::Schedule;
use crate::env::{Environment, describe};
use crate::sources::clamp_interval;
use crate::vars;

/// One thing wrong with a rule.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Problem {
    /// Where in `config.toml` the mistake is written, as `rules[2].trigger.edge`.
    pub key: String,
    /// The bytes of the value at `key` the mistake is about — the part of an expression or a schedule — or `None` for the whole value.
    pub within: Option<Range<usize>>,
    pub message: Message,
    /// A mistake the shell works around — an interval shorter than the limit runs at the limit — rather than one that keeps the rule from running.
    pub warning: bool,
}

impl Problem {
    fn error(key: String, within: Option<Range<usize>>, message: Message) -> Self {
        Self {
            key,
            within,
            message,
            warning: false,
        }
    }
}

/// What sets a rule off, read and checked.
#[derive(Clone, Debug)]
pub enum Trigger {
    Event(EventKind),
    Edge(Compiled),
    Schedule(Schedule),
    Every(Duration),
}

/// A rule ready to run: its trigger, `when` and `store` compiled, its interval no shorter than the limit.
#[derive(Clone, Debug)]
pub(crate) struct Prepared {
    pub(crate) trigger: Trigger,
    pub(crate) when: Option<Compiled>,
    pub(crate) store: Option<(String, Compiled)>,
    /// What every name the rule's expressions read means now: a rule whose config and meanings are both unchanged by a reload is the same rule, and keeps running as it was.
    pub(crate) meanings: Vec<(Reference, Type)>,
}

/// Everything wrong with `rules`, in the order they are written. `resolves` answers whether a command line names a command the shell has, without running it.
pub fn check(
    rules: &[RuleConfig],
    env: &Environment,
    resolves: &dyn Fn(&str) -> bool,
    automation: &AutomationConfig,
) -> Vec<Problem> {
    prepare_all(rules, env, resolves, automation)
        .into_iter()
        .flat_map(|(_, problems)| problems)
        .collect()
}

/// Each rule of `rules` ready to run, or `None` where something keeps it from running, with what is wrong with it either way.
pub(crate) fn prepare_all(
    rules: &[RuleConfig],
    env: &Environment,
    resolves: &dyn Fn(&str) -> bool,
    automation: &AutomationConfig,
) -> Vec<(Option<Prepared>, Vec<Problem>)> {
    let mut taken = BTreeSet::new();
    rules
        .iter()
        .enumerate()
        .map(|(index, rule)| {
            let mut problems = prepare(index, rule, env, resolves, automation);
            if !rule.id.is_empty() && !taken.insert(rule.id.as_str()) {
                problems.1.push(Problem::error(
                    format!("rules[{index}].id"),
                    None,
                    util::message!("finding.rule_name_taken", id = &rule.id),
                ));
            }
            let stops = problems.1.iter().any(|problem| !problem.warning);
            (problems.0.filter(|_| !stops), problems.1)
        })
        .collect()
}

fn prepare(
    index: usize,
    rule: &RuleConfig,
    env: &Environment,
    resolves: &dyn Fn(&str) -> bool,
    automation: &AutomationConfig,
) -> (Option<Prepared>, Vec<Problem>) {
    let at = |key: &str| format!("rules[{index}].{key}");
    let mut problems = Vec::new();

    if let Err(why) = check_id(&rule.id) {
        problems.push(Problem::error(at("id"), None, why));
    }

    let trigger = trigger(rule, &at, env, automation, &mut problems);

    let when = rule
        .when
        .as_deref()
        .and_then(|source| expression(source, &Type::Bool, &at("when"), env, &mut problems));

    for (line_index, line) in rule.run.iter().enumerate() {
        let key = at(&format!("run[{line_index}]"));
        if runs_a_rule(line) {
            problems.push(Problem::error(
                key,
                None,
                util::message!("finding.rule_runs_rule", line = line),
            ));
        } else if !resolves(line) {
            problems.push(Problem::error(
                key,
                None,
                util::message!("finding.rule_unknown_command", line = line),
            ));
        }
    }

    let store = rule.store.as_ref().and_then(|store| {
        let mut valid = true;
        if let Err(why) = vars::check_name(&store.var) {
            problems.push(Problem::error(at("store.var"), None, why));
            valid = false;
        }
        let value = stored_value(&store.value, &at("store.value"), env, &mut problems)?;
        warn_of_private_readings(store, &value, &at("store.value"), env, &mut problems);
        valid.then(|| (store.var.clone(), value))
    });

    if rule.run.is_empty() && rule.store.is_none() {
        problems.push(Problem {
            warning: true,
            ..Problem::error(at("run"), None, util::message!("finding.rule_does_nothing"))
        });
    }

    let prepared = trigger.map(|trigger| {
        let compiled = match &trigger {
            Trigger::Edge(edge) => Some(edge),
            _ => None,
        }
        .into_iter()
        .chain(&when)
        .chain(store.iter().map(|(_, value)| value));
        let meanings = compiled
            .flat_map(|compiled| compiled.references())
            .map(|reference| {
                let ty =
                    telar_expression::Environment::reference(env, reference).unwrap_or(Type::Never);
                (reference.clone(), ty)
            })
            .collect();
        Prepared {
            trigger,
            when,
            store,
            meanings,
        }
    });
    (prepared, problems)
}

/// A rule's name has to be one word `rule run` can take.
fn check_id(id: &str) -> Result<(), Message> {
    if id.is_empty() {
        return Err(util::message!("finding.rule_needs_id"));
    }
    match id
        .chars()
        .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
    {
        true => Ok(()),
        false => Err(util::message!("finding.rule_id", id = id)),
    }
}

fn trigger(
    rule: &RuleConfig,
    at: &dyn Fn(&str) -> String,
    env: &Environment,
    automation: &AutomationConfig,
    problems: &mut Vec<Problem>,
) -> Option<Trigger> {
    let keys = RuleTrigger::KEYS.map(|key| format!("`{key}`"));
    let keys = util::message!(
        "finding.either",
        first = keys[..keys.len() - 1].join(", "),
        last = &keys[keys.len() - 1]
    );
    let (key, value) = match rule.trigger.written().as_slice() {
        [] => {
            problems.push(Problem::error(
                at("trigger"),
                None,
                util::message!("finding.rule_needs_trigger", keys = keys),
            ));
            return None;
        }
        [one] => *one,
        [first, rest @ ..] => {
            for (key, _) in rest {
                problems.push(Problem::error(
                    at(&format!("trigger.{key}")),
                    None,
                    util::message!("finding.rule_one_trigger", keys = &keys, first = first.0),
                ));
            }
            return None;
        }
    };
    let key_at = at(&format!("trigger.{key}"));
    match key {
        "event" => match EventKind::from_name(value.trim()) {
            Some(kind) => Some(Trigger::Event(kind)),
            None => {
                problems.push(Problem::error(
                    key_at,
                    None,
                    util::message!(
                        "finding.not_an_event",
                        value = value,
                        events = EventKind::names()
                            .map(|name| format!("`{name}`"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                ));
                None
            }
        },
        "edge" => expression(value, &Type::Bool, &key_at, env, problems).map(Trigger::Edge),
        "schedule" => match Schedule::parse(value) {
            Ok(schedule) => Some(Trigger::Schedule(schedule)),
            Err(error) => {
                problems.push(Problem::error(key_at, Some(error.span), error.message));
                None
            }
        },
        _ => match clamp_interval(value, automation.min_interval()) {
            Ok((every, warning)) => {
                if let Some(warning) = warning {
                    problems.push(Problem {
                        warning: true,
                        ..Problem::error(key_at, None, warning)
                    });
                }
                Some(Trigger::Every(every))
            }
            Err(why) => {
                problems.push(Problem::error(key_at, None, why));
                None
            }
        },
    }
}

/// `source` compiled against `env`, required to give `expected`; each mistake is a problem at `key`, placed in the expression.
fn expression(
    source: &str,
    expected: &Type,
    key: &str,
    env: &Environment,
    problems: &mut Vec<Problem>,
) -> Option<Compiled> {
    let compiled = compiled(source, key, env, problems)?;
    match compiled.require(expected) {
        Ok(compiled) => Some(compiled),
        Err(error) => {
            problems.push(Problem::error(
                key.to_string(),
                Some(error.span.range()),
                describe(&error.code),
            ));
            None
        }
    }
}

/// A `store` value: any type a variable can hold.
fn stored_value(
    source: &str,
    key: &str,
    env: &Environment,
    problems: &mut Vec<Problem>,
) -> Option<Compiled> {
    let compiled = compiled(source, key, env, problems)?;
    if let Err(why) = vars::var_type_of(compiled.ty()) {
        problems.push(Problem::error(key.to_string(), Some(0..source.len()), why));
        return None;
    }
    Some(compiled)
}

fn compiled(
    source: &str,
    key: &str,
    env: &Environment,
    problems: &mut Vec<Problem>,
) -> Option<Compiled> {
    match env.compile(source) {
        Ok(compiled) => Some(compiled),
        Err(errors) => {
            for error in errors {
                problems.push(Problem::error(
                    key.to_string(),
                    Some(error.span.range()),
                    describe(&error.code),
                ));
            }
            None
        }
    }
}

/// Whether `line` is `rule run …`: a rule that runs a rule could run itself, directly or through another, until the shell's stack gives out.
fn runs_a_rule(line: &str) -> bool {
    let mut words = line.split_whitespace();
    words.next() == Some("rule") && words.next() == Some("run")
}

/// Warns where `store` keeps something the lock screen hides in a variable, which the lock screen reads (TA-8).
fn warn_of_private_readings(
    store: &RuleStore,
    value: &Compiled,
    key: &str,
    env: &Environment,
    problems: &mut Vec<Problem>,
) {
    for reference in value.references() {
        if !env.hidden_on_lock(reference) {
            continue;
        }
        let written = format!("${}", reference.dotted());
        let within = store
            .value
            .find(&written)
            .map(|start| start..start + written.len());
        problems.push(Problem {
            warning: true,
            ..Problem::error(
                key.to_string(),
                within,
                util::message!(
                    "finding.store_shows_hidden",
                    written = &written,
                    var = &store.var
                ),
            )
        });
    }
}
