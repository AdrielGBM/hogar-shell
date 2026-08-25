//! Which card a chip shows when the pointer rests on it.
//!
//! The hover surface's counterpart to [`crate::panels`], and registered for the same reason: the popout is one card-shaped surface that opens under whichever chip was hovered, and naming the modules it can show would be the one thing stopping it from being built without them.
//!
//! A builder yields a [`Card`] rather than a finished tree, because the surface owns the frame around it — the panel fill, the bar's radius and the pointer tracking that keeps the popout up — and a card that built its own box could not be told any of that.

use std::cell::RefCell;
use std::collections::HashMap;

use telar::{LayoutError, LayoutItem};

use config::Config;
use config::theme::NordTheme;

use crate::card::Card;

pub type PopoutBuilder = fn(&Config, NordTheme) -> Card;

/// Every module a hover popout is offered for.
#[derive(Default)]
pub struct PopoutRegistry {
    cards: HashMap<String, PopoutBuilder>,
}

impl PopoutRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, id: &str, build: PopoutBuilder) {
        self.cards.insert(id.to_string(), build);
    }

    pub fn has(&self, id: &str) -> bool {
        self.cards.contains_key(id)
    }

    /// Every registered id, sorted. What lets a test walk the cards without a surface to hover.
    pub fn ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.cards.keys().cloned().collect();
        ids.sort();
        ids
    }
}

thread_local! {
    static LIVE: RefCell<Option<PopoutRegistry>> = const { RefCell::new(None) };
}

/// Publishes the registry the popout surface resolves against. Set once at startup.
pub fn install(registry: PopoutRegistry) {
    LIVE.with(|live| *live.borrow_mut() = Some(registry));
}

/// Whether `module` has a card. What the bar wires its chips' hover from, so no chip is given a hover target it would open empty.
pub fn has_popout(module: &str) -> bool {
    LIVE.with(|live| {
        live.borrow()
            .as_ref()
            .is_some_and(|registry| registry.has(module))
    })
}

/// Builds `module`'s card, or `None` when it has none — which the hover wiring already gates on, so an id that reaches here anyway gets no card rather than a mislabelled one.
pub fn build(
    module: &str,
    config: &Config,
    theme: NordTheme,
) -> Option<Result<Box<dyn LayoutItem>, LayoutError>> {
    let build = LIVE.with(|live| {
        live.borrow()
            .as_ref()
            .and_then(|registry| registry.cards.get(module).copied())
    })?;
    Some(build(config, theme).build())
}
