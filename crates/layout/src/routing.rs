//! Which stack a card lands in (F-2.8): the routes of the stacks on its output, matched against what the card is.

use crate::model::{CardKind, Route, Urgency};

/// What a route is matched against: a card's kind, and for a notification the application it came from and how urgent it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RoutedCard<'a> {
    pub kind: CardKind,
    pub app: Option<&'a str>,
    pub urgency: Option<Urgency>,
}

impl Route {
    /// Whether this route takes `card`: every field it sets matches. A route naming an app or an urgency takes only notifications, the one kind of card that has either.
    pub fn takes(&self, card: &RoutedCard) -> bool {
        self.kind.is_none_or(|kind| kind == card.kind)
            && self
                .app
                .as_deref()
                .is_none_or(|app| card.app.is_some_and(|from| from.eq_ignore_ascii_case(app)))
            && self
                .urgency
                .is_none_or(|urgency| card.urgency == Some(urgency))
    }
}

/// Which of `stacks`, in the order a card is offered to them, takes `card`: the first with a route that takes it, else the first with no routes at all. A stack that routes nothing is where everything else goes wherever it is in that order, and a stack whose routes take nothing of this card's never sees it.
pub fn route_card<'a, T>(
    stacks: &'a [T],
    routes: impl Fn(&T) -> &[Route],
    card: &RoutedCard,
) -> Option<&'a T> {
    stacks
        .iter()
        .find(|stack| routes(stack).iter().any(|route| route.takes(card)))
        .or_else(|| stacks.iter().find(|stack| routes(stack).is_empty()))
}
