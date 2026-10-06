//! The Actions block of an area's and an instance's popover. A line is written only once it passes the check IPC makes of it ([`layout::grants_trust`], the command table), so a refused line leaves the chain as it was.

use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use telar::{LayoutStyle, ReactiveList, RwSignal, SizeDimension, effect, signal};

use layout::{Action, LayerKind, Trigger};
use ui::descriptor::Built;

use super::origin::{self, Provenance};
use super::rows::{self, Rows, label};

/// A row added for a gesture is the popover's own until its chain runs something: writing an empty chain would bind the gesture to nothing and take it from whatever answers it now.
#[derive(Clone)]
pub struct Actions {
    written: RwSignal<BTreeMap<Trigger, Action>>,
    unwritten: RwSignal<BTreeSet<Trigger>>,
    drawn: Rc<dyn Fn() -> BTreeMap<Trigger, Action>>,
    provenance: Rc<dyn Fn(&str) -> Provenance>,
}

impl Actions {
    pub(crate) fn new(
        written: RwSignal<BTreeMap<Trigger, Action>>,
        unwritten: RwSignal<BTreeSet<Trigger>>,
        drawn: Rc<dyn Fn() -> BTreeMap<Trigger, Action>>,
        provenance: Rc<dyn Fn(&str) -> Provenance>,
    ) -> Self {
        Self {
            written,
            unwritten,
            drawn,
            provenance,
        }
    }

    pub fn bound(&self) -> Vec<Trigger> {
        let mut bound: BTreeSet<Trigger> = (self.drawn)().into_keys().collect();
        self.written.with(|written| bound.extend(written.keys()));
        self.unwritten.with(|unwritten| bound.extend(unwritten));
        bound.into_iter().collect()
    }

    pub fn text(&self, trigger: Trigger) -> String {
        self.chain(trigger)
            .map(|action| action.0.join("; "))
            .unwrap_or_default()
    }

    pub fn writes(&self, trigger: Trigger) -> bool {
        self.written.with(|written| written.contains_key(&trigger))
    }

    pub fn provenance(&self, trigger: Trigger) -> Provenance {
        (self.provenance)(&key_of(trigger))
    }

    pub fn add(&self) -> Result<Trigger, String> {
        let bound = self.bound();
        let free = Trigger::ALL
            .into_iter()
            .find(|trigger| !bound.contains(trigger))
            .ok_or_else(|| telar::t!("editor.actions.all_bound"))?;
        self.unwritten.update(|unwritten| {
            unwritten.insert(free);
        });
        Ok(free)
    }

    /// An empty chain takes the gesture's binding off here and leaves its row, so the line can be typed again.
    pub fn set(&self, trigger: Trigger, text: &str) -> Result<(), String> {
        let chain = chain_of(text);
        if let Some(why) = chain.iter().find_map(|line| refusal(line)) {
            return Err(why);
        }
        if chain.is_empty() {
            self.unbind(trigger);
            if !self
                .unwritten
                .peek_with(|unwritten| unwritten.contains(&trigger))
            {
                self.unwritten.update(|unwritten| {
                    unwritten.insert(trigger);
                });
            }
            return Ok(());
        }
        self.forget(trigger);
        let action = Action(chain);
        if self
            .written
            .peek_with(|written| written.get(&trigger) == Some(&action))
        {
            return Ok(());
        }
        self.written.update(|written| {
            written.insert(trigger, action);
        });
        Ok(())
    }

    pub fn retrigger(&self, from: Trigger, to: Trigger) -> Result<(), String> {
        if from == to {
            return Ok(());
        }
        if self.bound().contains(&to) {
            return Err(telar::t!(
                "editor.actions.taken",
                gesture = crate::context::trigger_name(to)
            ));
        }
        match self.chain(from).filter(|action| !action.0.is_empty()) {
            Some(action) => {
                self.forget(from);
                self.written.update(|written| {
                    written.remove(&from);
                    written.insert(to, action);
                });
            }
            None => {
                self.unbind(from);
                self.unwritten.update(|unwritten| {
                    unwritten.remove(&from);
                    unwritten.insert(to);
                });
            }
        }
        Ok(())
    }

    /// A gesture a broader level also binds runs what that level binds once this is taken off; there is no `unset` for actions.
    pub fn remove(&self, trigger: Trigger) {
        self.unbind(trigger);
        self.forget(trigger);
    }

    fn chain(&self, trigger: Trigger) -> Option<Action> {
        self.written
            .peek_with(|written| written.get(&trigger).cloned())
            .or_else(|| (self.drawn)().remove(&trigger))
    }

    fn unbind(&self, trigger: Trigger) {
        if self
            .written
            .peek_with(|written| written.contains_key(&trigger))
        {
            self.written.update(|written| {
                written.remove(&trigger);
            });
        }
    }

    fn forget(&self, trigger: Trigger) {
        if self
            .unwritten
            .peek_with(|unwritten| unwritten.contains(&trigger))
        {
            self.unwritten.update(|unwritten| {
                unwritten.remove(&trigger);
            });
        }
    }
}

fn key_of(trigger: Trigger) -> String {
    format!("actions.{}", trigger.as_str())
}

pub fn chain_of(text: &str) -> Vec<String> {
    text.split(';')
        .map(|line| line.trim().to_string())
        .filter(|line| !line.is_empty())
        .collect()
}

/// The same refusal IPC gives: trust is the user's alone to give, never a gesture's.
pub fn refusal(line: &str) -> Option<String> {
    if layout::grants_trust(line) {
        return Some(telar::t!("editor.actions.trust", line = line));
    }
    if !services::command::resolves(line) {
        return Some(telar::t!("editor.actions.unknown", line = line));
    }
    None
}

/// The lock screen holds readings, never controls; behind the windows, and on a picture or a texture, there is nothing to press.
pub fn offered(layer: LayerKind, kind: Option<&str>) -> bool {
    !matches!(layer, LayerKind::Lock | LayerKind::Background)
        && !matches!(kind, Some("wallpaper_region" | "texture"))
}

pub(crate) fn rows(actions: Actions, note: Option<fn() -> String>) -> Rows {
    let mut list = vec![rows::heading(|| telar::t!("editor.actions.heading"))?];
    let (listing, building) = (actions.clone(), actions.clone());
    list.push(Box::new(ReactiveList::with_style(
        LayoutStyle::new()
            .flex_column()
            .gap(ui::scale::space::sm())
            .width(SizeDimension::Percent(1.0)),
        move || listing.bound(),
        |trigger: &Trigger| trigger.as_str(),
        move |trigger: Trigger| row(&building, trigger),
    )?));
    list.push(rows::action(
        || telar::t!("editor.actions.add"),
        move || {
            if let Err(why) = actions.add() {
                crate::mode::refuse(why);
            }
        },
    )?);
    if let Some(note) = note {
        list.push(rows::note(note)?);
    }
    Ok(list)
}

fn row(actions: &Actions, trigger: Trigger) -> Built {
    let gesture = signal(trigger.as_str().to_string());
    let moving = actions.clone();
    following(gesture, move |picked: &String| {
        let Some(to) = Trigger::from_name(picked).filter(|to| *to != trigger) else {
            return;
        };
        if let Err(why) = moving.retrigger(trigger, to) {
            crate::mode::refuse(why);
            gesture.set(trigger.as_str().to_string());
        }
    });
    let text = signal(actions.text(trigger));
    let refused = signal(None::<String>);
    let setting = actions.clone();
    following(text, move |now: &String| {
        let why = setting.set(trigger, now).err();
        if refused.peek() != why {
            refused.set(why);
        }
    });
    let gestures: Rc<[(String, String)]> = Trigger::ALL
        .into_iter()
        .map(|each| {
            (
                each.as_str().to_string(),
                crate::context::trigger_name(each),
            )
        })
        .collect();
    let fields = rows::together(vec![
        rows::listed(label!("editor.actions.gesture"), None, gesture, gestures)?,
        rows::text(label!("editor.actions.runs"), None, text)?,
        rows::note(move || refused.get().unwrap_or_default())?,
    ])?;
    let (standing, writing, removing) = (actions.clone(), actions.clone(), actions.clone());
    origin::noted(
        fields,
        move || standing.provenance(trigger).said(),
        label!("editor.popover.remove"),
        move || writing.writes(trigger),
        move || removing.remove(trigger),
    )
}

fn following<T: Clone + 'static>(value: RwSignal<T>, act: impl Fn(&T) + 'static) {
    let seeded = Cell::new(false);
    effect(move || {
        let now = value.get();
        if seeded.replace(true) {
            act(&now);
        }
    });
}
