//! Which module has a hover card, and what builds it.
//!
//! The third half of the composition root, beside [`crate::core::registry`] and [`crate::core::panels`]: the
//! popout is one surface that opens under whichever chip was hovered, and this is where it learns what to draw
//! there without naming a module itself.

use ui::popouts::PopoutRegistry;

pub fn default_popouts() -> PopoutRegistry {
    modules::popout_cards::cards()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every card is offered for a module that exists, so no hover can be wired to an id nothing puts on a bar.
    #[test]
    fn every_card_belongs_to_a_registered_module() {
        let popouts = default_popouts();
        let modules = crate::core::registry::default_registry(&popouts);
        for id in popouts.ids() {
            assert!(
                modules.def(&id).is_some(),
                "'{id}' has a popout card but is not a registered module"
            );
        }
    }
}
