//! Ids, names and keys as a person reads them (F-10.3): an id is what IPC, a finding and the trust prompt name a thing by, so it holds nothing that draws as something else.
//!
//! A file can write any character into a TOML string or a quoted key, and an id that holds a terminal escape, a bidirectional control or an invisible character reads as another id — in a finding on a terminal, in the editor, in a list of what a bundle runs. Every id, name and key a layout or a komponent writes is checked here, wherever the file came from: [`crate::validate`] runs it on every layout, and [`crate::bundle::read`] on a bundle before anything else looks at it. The id of an area, a group or an instance is held to [`id_refusal`] as well, the rule a rename is held to, since each is a part of an address.

use util::report::{Finding, Report};
use util::text::{hides, is_bidi_control};

use crate::model::*;
use crate::ops::sites;

/// What a text may not hold.
#[derive(Clone, Copy)]
enum Rule {
    /// The id of an area, a group or an instance: [`id_refusal`].
    Id,
    /// A key, or a name that refers to something by its id: nothing [`hides`].
    Key,
    /// A name only shown — a layout's `name` — which may hold an emoji, and so a joiner or a variation selector, but no control character and nothing that changes the direction text is shown in.
    Shown,
}

/// A finding in `file` for every id, name and key `layout` writes that holds a character a person cannot read as itself, and every id of an area, a group or an instance that holds a separator: its `name`, `extends`, the names of its sources, its output and workspace patterns, and at every level each area, group and instance id, every id it removes or takes back, the komponent a group draws and the parameters it sets, and each instance's module, option keys and binding paths.
pub fn unreadable_in_layout(layout: &Layout, file: &str, report: &mut Report) {
    let mut found = Found { file, report };
    found.check("name", &layout.name, Rule::Shown);
    if let Some(extends) = &layout.extends {
        found.check("extends", extends.as_str(), Rule::Key);
    }
    for name in layout.sources.keys() {
        found.check(&format!("sources.{name}"), name, Rule::Key);
    }
    for rule in &layout.outputs {
        let at = format!("outputs.{}", rule.matches.0);
        found.check(&at, &rule.matches.0, Rule::Key);
        for workspace in &rule.workspaces {
            found.check(
                &format!("{at}.workspaces.{}", workspace.matches.0),
                &workspace.matches.0,
                Rule::Key,
            );
        }
    }
    for (site, layer) in sites(layout) {
        found.layer(&site.to_string(), layer);
    }
}

/// The same for the komponent written at `file`: its parameters' names and each child as an instance is checked.
pub fn unreadable_in_komponent(komponent: &Komponent, file: &str, report: &mut Report) {
    let mut found = Found { file, report };
    for name in komponent.parameters.keys() {
        found.check(&format!("parameters.{name}"), name, Rule::Key);
    }
    for child in &komponent.children {
        found.instance(&format!("children.{}", child.id), child);
    }
}

struct Found<'a> {
    file: &'a str,
    report: &'a mut Report,
}

impl Found<'_> {
    fn check(&mut self, key: &str, text: &str, rule: Rule) {
        let message = match rule {
            Rule::Id => id_refusal(text).map(|refusal| refusal.message(text)),
            Rule::Key => text
                .chars()
                .find(|c| hides(*c))
                .map(|c| IdRefusal::Hidden(c).message(text)),
            Rule::Shown => text
                .chars()
                .find(|c| c.is_control() || is_bidi_control(*c))
                .map(|c| {
                    util::message!(
                        "finding.unreadable_display_name",
                        name = text,
                        code = format!("{:04X}", u32::from(c))
                    )
                }),
        };
        if let Some(message) = message {
            self.report.error(Finding::new(self.file, key, message));
        }
    }

    fn layer(&mut self, at: &str, layer: &Layer) {
        for removed in &layer.remove {
            self.check(&format!("{at}.remove"), removed.as_str(), Rule::Key);
        }
        for area in &layer.areas {
            let at = format!("{at}.areas.{}", area.id);
            self.check(&at, area.id.as_str(), Rule::Id);
            self.unsets(&at, &area.unset);
            for removed in &area.remove {
                self.check(&format!("{at}.remove"), removed.as_str(), Rule::Key);
            }
            for group in &area.groups {
                let at = format!("{at}.groups.{}", group.id);
                self.check(&at, group.id.as_str(), Rule::Id);
                self.unsets(&at, &group.unset);
                for removed in &group.remove {
                    self.check(&format!("{at}.remove"), removed.as_str(), Rule::Key);
                }
                if let Some(komponent) = &group.komponent {
                    self.check(&format!("{at}.komponent"), komponent.as_str(), Rule::Key);
                }
                for name in group.parameters.keys() {
                    self.check(&format!("{at}.parameters.{name}"), name, Rule::Key);
                }
                for child in &group.children {
                    self.instance(&format!("{at}.children.{}", child.id), child);
                }
            }
        }
    }

    fn instance(&mut self, at: &str, instance: &Instance) {
        self.check(at, instance.id.as_str(), Rule::Id);
        if let Some(module) = &instance.module {
            self.check(&format!("{at}.module"), module, Rule::Key);
        }
        self.table(&format!("{at}.options"), &instance.options);
        for path in instance.bindings.keys() {
            self.check(&format!("{at}.bindings.{path}"), path, Rule::Key);
        }
        self.unsets(at, &instance.unset);
    }

    fn table(&mut self, at: &str, table: &toml::Table) {
        for (key, value) in table {
            let at = format!("{at}.{key}");
            self.check(&at, key, Rule::Key);
            if let toml::Value::Table(inner) = value {
                self.table(&at, inner);
            }
        }
    }

    fn unsets(&mut self, at: &str, unsets: &[Unset]) {
        for unset in unsets {
            self.check(&format!("{at}.unset"), &unset.to_string(), Rule::Key);
        }
    }
}
