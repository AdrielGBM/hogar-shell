use std::collections::HashMap;
use std::hash::Hash;

use platform_wayland::ToplevelArea;

/// The minimise targets the strips have sent, each with the entry that speaks for it now.
///
/// An entry is built again whenever its strip changes shape, and the list may build the new one before it lets the old one go. Both send from the same surface and the compositor keeps one target per window, so the old entry's withdrawal would take down the target the new one had just set. Only the latest entry for a window on a surface sets or withdraws its target.
pub(crate) struct Targets<K> {
    sent: HashMap<K, Sent>,
    next: u64,
}

struct Sent {
    holder: u64,
    area: Option<ToplevelArea>,
}

impl<K: Hash + Eq> Targets<K> {
    pub(crate) fn new() -> Self {
        Self {
            sent: HashMap::new(),
            next: 0,
        }
    }

    /// Makes a new entry the one that speaks for `key`, keeping the target its predecessor sent so the next change is measured against what the compositor holds.
    pub(crate) fn claim(&mut self, key: K) -> u64 {
        self.next += 1;
        let holder = self.next;
        self.sent
            .entry(key)
            .and_modify(|sent| sent.holder = holder)
            .or_insert(Sent { holder, area: None });
        holder
    }

    /// Whether `holder` is to send `area` for `key`: only while it is the latest entry, and only when that is not what was last sent.
    pub(crate) fn send(&mut self, key: &K, holder: u64, area: Option<ToplevelArea>) -> bool {
        match self.sent.get_mut(key) {
            Some(sent) if sent.holder == holder && sent.area != area => {
                sent.area = area;
                true
            }
            _ => false,
        }
    }

    /// Whether `holder`, going, is to withdraw the target for `key`: only while it is still the latest entry and a target is up.
    pub(crate) fn release(&mut self, key: &K, holder: u64) -> bool {
        match self.sent.get(key) {
            Some(sent) if sent.holder == holder => self
                .sent
                .remove(key)
                .is_some_and(|sent| sent.area.is_some()),
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SLOT: ToplevelArea = ToplevelArea {
        x: 10,
        y: 0,
        width: 30,
        height: 30,
    };
    const MOVED: ToplevelArea = ToplevelArea { x: 50, ..SLOT };

    /// The list builds the reshaped entry before it lets the old one go: the old one's withdrawal must not take down the target the new one stands for.
    #[test]
    fn an_entry_replaced_before_it_goes_leaves_its_successors_target_up() {
        let mut targets = Targets::new();
        let old = targets.claim(7);
        assert!(targets.send(&7, old, Some(SLOT)));

        let new = targets.claim(7);
        assert!(
            !targets.send(&7, new, Some(SLOT)),
            "the target is already where the new entry is, so nothing is sent again"
        );
        assert!(
            !targets.send(&7, old, Some(MOVED)),
            "the old entry no longer speaks for the window"
        );
        assert!(!targets.release(&7, old), "nor withdraws its target");
        assert!(targets.send(&7, new, Some(MOVED)));
        assert!(targets.release(&7, new), "the latest entry withdraws it");
        assert!(!targets.release(&7, new), "once");
    }

    /// The old entry going first is the other order the list may take: it withdraws, and the new entry sets the target again.
    #[test]
    fn an_entry_that_goes_before_its_successor_is_built_withdraws_and_the_successor_sets_again() {
        let mut targets = Targets::new();
        let old = targets.claim(7);
        assert!(targets.send(&7, old, Some(SLOT)));
        assert!(targets.release(&7, old));
        let new = targets.claim(7);
        assert!(targets.send(&7, new, Some(SLOT)));
    }

    /// A new entry with nowhere to stand still withdraws the target its predecessor left up, rather than leaving the window minimising to a place nothing is drawn.
    #[test]
    fn a_successor_with_no_target_withdraws_its_predecessors() {
        let mut targets = Targets::new();
        let old = targets.claim(7);
        assert!(targets.send(&7, old, Some(SLOT)));
        let new = targets.claim(7);
        assert!(targets.send(&7, new, None));
        assert!(
            !targets.release(&7, new),
            "nothing is up to withdraw as it goes"
        );
    }

    #[test]
    fn windows_are_kept_apart() {
        let mut targets = Targets::new();
        let first = targets.claim(1);
        let second = targets.claim(2);
        assert!(targets.send(&1, first, Some(SLOT)));
        assert!(targets.send(&2, second, Some(SLOT)));
        assert!(targets.release(&1, first));
        assert!(targets.release(&2, second));
    }
}
