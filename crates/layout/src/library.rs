//! What a layout is resolved against besides itself.

use std::collections::BTreeMap;

use crate::model::*;
use crate::trust::Trust;

/// The layouts a layout's `extends` chain may name and the komponents its groups may use, by name: what the layout store holds, or what a test or a check states for itself.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Library {
    pub layouts: BTreeMap<LayoutId, Layout>,
    pub komponents: BTreeMap<KomponentId, Komponent>,
    /// Which of them came with a bundle, and what of theirs the user let run: what resolution holds back.
    pub trust: Trust,
}

impl Library {
    /// `layouts`, by their ids, and no komponents.
    pub fn of_layouts(layouts: impl IntoIterator<Item = Layout>) -> Self {
        Self {
            layouts: layouts
                .into_iter()
                .map(|layout| (layout.id.clone(), layout))
                .collect(),
            komponents: BTreeMap::new(),
            trust: Trust::default(),
        }
    }

    /// The same library holding `komponent` as `id` too.
    pub fn with_komponent(mut self, id: impl AsRef<str>, komponent: Komponent) -> Self {
        self.komponents.insert(KomponentId::new(id), komponent);
        self
    }

    pub fn layout(&self, id: &LayoutId) -> Option<&Layout> {
        self.layouts.get(id)
    }

    pub fn komponent(&self, id: &KomponentId) -> Option<&Komponent> {
        self.komponents.get(id)
    }
}

/// Where a komponent's file is, by the name a group uses it as.
pub fn komponent_path(id: &KomponentId) -> String {
    format!("components/{id}.toml")
}

/// Whether `name` can name a komponent, a bundle or a file a bundle carries — what its file is called, and what `layout trust` and an import name it by: lowercase ASCII letters, digits, `-` and `_`. ASCII because a name is how a person tells one bundle from another, and a letter of another script shaped like a Latin one (`nоrd` with a Cyrillic `о`) would let a stranger's bundle pass for one they know; lowercase because `I` and `l` are one shape in many fonts.
pub fn is_komponent_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}
