//! Where a change to something on screen is written in the layout being edited.
//!
//! What a screen shows is every output rule of the edited layout that matches it, broadest first, laid over whatever that layout extends (F-10.1), and then the workspace rules of those rules for the workspace that is up. A change is written into the narrowest of those rules that already writes the area, which is where the value it replaces came from. An area only a layout it extends writes gets a partial entry in the narrowest rule that covers the screen, naming just what changed, so the change lies over the inherited area rather than copying the rest of it.
//!
//! A change made for one workspace alone ([`crate::variant`]) is written the same way one level further in: into that workspace's rule of the narrowest output rule that covers the screen, which is made for it if the layout has none yet. It then resolves on that workspace and nowhere else, and TA-2 holds because the layout refuses a workspace rule that touches what reserves space.
//!
//! Taking an area away works the same way round: it is deleted from the narrowest rule that writes it, and where a broader level still places it, the narrowest level covering the screen names it in its layer's `remove` ([`area_removal`]).

use std::collections::BTreeMap;

use layout::{
    Area, AreaId, Group, GroupId, Instance, InstanceId, LayerKind, Layout, LayoutId, LayoutOp,
    OpError, OutputRule, PromptEdit, Resolved, ResolvedArea, ResolvedAreaKind, Site, Spot,
    WorkspaceMatch, WorkspaceRule,
};
use surfaces::reconcile::Desktop;

use crate::session::EditError;

/// An area as the rule that decides it writes it.
#[derive(Clone, Debug, PartialEq)]
pub struct Written {
    pub site: Site,
    /// The area as that rule writes it; for one it does not write yet, an entry naming only its id.
    pub area: Area,
    /// Where a new entry goes when the rule does not write the area ([`new_at`]): merging never restacks, so an override written there moves nothing.
    insert_at: Option<usize>,
    /// The workspace rule the site is in, where the layout has none for that workspace yet and the first change made for it has to make it.
    rule: Option<LayoutOp>,
}

impl Written {
    /// Where the edited layout decides the area `id` on the layer `layer` of the screen `output`: for every workspace, or for `workspace` alone.
    pub fn area(
        layout: &Layout,
        output: Option<&str>,
        layer: LayerKind,
        id: &AreaId,
        workspace: Option<&WorkspaceMatch>,
    ) -> Result<Self, String> {
        let screen = output.unwrap_or_default();
        let mut rules: Vec<&OutputRule> = layout
            .outputs
            .iter()
            .filter(|rule| rule.matches.matches(screen))
            .collect();
        rules.sort_by_key(|rule| rule.matches.specificity());
        match workspace {
            None => Self::for_every_workspace(&rules, screen, layer, id),
            Some(workspace) => Self::for_workspace(&rules, screen, layer, id, workspace),
        }
    }

    fn for_every_workspace(
        rules: &[&OutputRule],
        screen: &str,
        layer: LayerKind,
        id: &AreaId,
    ) -> Result<Self, String> {
        let site = |rule: &OutputRule| Site {
            output: rule.matches.clone(),
            workspace: None,
            layer,
        };
        let writer = rules.iter().rev().find_map(|rule| {
            let area = rule
                .layers
                .get(layer)
                .areas
                .iter()
                .find(|area| area.id == *id)?;
            Some((*rule, area))
        });
        if let Some((rule, area)) = writer {
            return Ok(Self {
                site: site(rule),
                area: area.clone(),
                insert_at: None,
                rule: None,
            });
        }
        let rule = rules
            .last()
            .ok_or_else(|| telar::t!("editor.popover.no_rule", output = screen))?;
        Ok(Self {
            site: site(rule),
            area: blank(id),
            insert_at: Some(new_at(rule.layers.get(layer))),
            rule: None,
        })
    }

    fn for_workspace(
        rules: &[&OutputRule],
        screen: &str,
        layer: LayerKind,
        id: &AreaId,
        workspace: &WorkspaceMatch,
    ) -> Result<Self, String> {
        if layer == LayerKind::Lock {
            return Err(telar::t!("editor.variant.lock"));
        }
        let site = |rule: &OutputRule| Site {
            output: rule.matches.clone(),
            workspace: Some(workspace.clone()),
            layer,
        };
        fn variant<'a>(
            rule: &'a OutputRule,
            workspace: &WorkspaceMatch,
            layer: LayerKind,
        ) -> Option<&'a layout::Layer> {
            rule.workspaces
                .iter()
                .find(|held| held.matches == *workspace)
                .and_then(|held| held.layers.get(layer))
        }
        let writer = rules.iter().rev().find_map(|rule| {
            let area = variant(rule, workspace, layer)?
                .areas
                .iter()
                .find(|area| area.id == *id)?;
            Some((*rule, area))
        });
        if let Some((rule, area)) = writer {
            return Ok(Self {
                site: site(rule),
                area: area.clone(),
                insert_at: None,
                rule: None,
            });
        }
        let rule = rules
            .last()
            .ok_or_else(|| telar::t!("editor.popover.no_rule", output = screen))?;
        let (insert_at, made) = match variant(rule, workspace, layer) {
            Some(written) => (written.areas.len(), None),
            None => (
                0,
                Some(LayoutOp::InsertWorkspaceRule {
                    output: rule.matches.clone(),
                    index: rule.workspaces.len(),
                    rule: Box::new(WorkspaceRule {
                        matches: workspace.clone(),
                        ..WorkspaceRule::default()
                    }),
                }),
            ),
        };
        Ok(Self {
            site: site(rule),
            area: blank(id),
            insert_at: Some(insert_at),
            rule: made,
        })
    }

    /// The area `id` as the output rule `site` names writes it, else a new entry there naming only its id — for an edit that has already decided which level it is written in.
    pub fn at(layout: &Layout, site: Site, id: &AreaId) -> Self {
        let layer = layout::ops::layer(layout, &site).ok();
        match layer.and_then(|layer| layer.areas.iter().find(|area| area.id == *id)) {
            Some(area) => Self {
                area: area.clone(),
                site,
                insert_at: None,
                rule: None,
            },
            None => Self {
                insert_at: Some(layer.map_or(0, new_at)),
                site,
                area: blank(id),
                rule: None,
            },
        }
    }

    /// Whether the rule writes the area already, rather than inheriting it.
    pub fn is_present(&self) -> bool {
        self.insert_at.is_none()
    }

    /// The operations that write `changed` where this area was read from: the area in place where the rule writes it, else a new entry — after making the workspace rule it goes in, where there is none yet.
    pub fn ops(&self, changed: &Area) -> Vec<LayoutOp> {
        let written = match self.insert_at {
            None => LayoutOp::ReplaceArea {
                site: self.site.clone(),
                id: self.area.id.clone(),
                area: Box::new(changed.clone()),
            },
            Some(index) => LayoutOp::InsertArea {
                site: self.site.clone(),
                index,
                area: Box::new(changed.clone()),
            },
        };
        self.rule.iter().cloned().chain([written]).collect()
    }

    /// The instance `id` of the group `group` as this area writes it; for one it does not write, an entry naming only its id.
    pub fn instance(&self, group: &GroupId, id: &InstanceId) -> WrittenInstance {
        let written = self
            .area
            .groups
            .iter()
            .find(|held| held.id == *group)
            .and_then(|held| held.children.iter().find(|child| child.id == *id));
        WrittenInstance {
            area: self.clone(),
            group: group.clone(),
            present: self.is_present() && written.is_some(),
            instance: written.cloned().unwrap_or_else(|| Instance {
                id: id.clone(),
                ..Instance::default()
            }),
        }
    }
}

/// What takes the area `id` off the layer `layer` of the screen `screen` shows, for every workspace or `workspace` alone: out of the narrowest rule that writes it, and — while a broader rule or a layout this one extends still places it there — named in the `remove` of the narrowest level covering the screen, so it goes from this screen and no other. The lock screen's prompt is refused, since a lock with nothing to type a password into is a lockout (TA-8).
pub fn area_removal(
    layout: &Layout,
    known: &BTreeMap<LayoutId, Layout>,
    screen: &Resolved,
    layer: LayerKind,
    id: &AreaId,
    workspace: Option<&WorkspaceMatch>,
) -> Result<Vec<LayoutOp>, String> {
    let is_prompt = screen
        .area(layer, id)
        .is_some_and(|area| matches!(area.kind, ResolvedAreaKind::Prompt { .. }));
    if is_prompt {
        return Err(OpError::Prompt {
            id: id.clone(),
            refused: PromptEdit::Remove,
        }
        .to_string());
    }
    let output = Some(screen.output.as_str());
    let mut after = layout.clone();
    let written = Written::area(&after, output, layer, id, workspace)?;
    let mut ops = match written.is_present() {
        true => vec![LayoutOp::DeleteArea {
            site: written.site.clone(),
            id: id.clone(),
        }],
        false => Vec::new(),
    };
    layout::ops::apply_all(&mut after, &ops).map_err(|why| why.to_string())?;
    let (resolved, _) = layout::resolve(&after, known, &screen.output, screen.workspace.as_ref());
    if resolved.area(layer, id).is_some() {
        ops.extend(hiding(&after, output, layer, id, workspace)?);
    }
    Ok(ops)
}

/// The operations that name `id` in the `remove` of the narrowest level covering the screen `output` — its most specific output rule, or that rule's rule for `workspace`, made first where the layout has none.
fn hiding(
    layout: &Layout,
    output: Option<&str>,
    layer: LayerKind,
    id: &AreaId,
    workspace: Option<&WorkspaceMatch>,
) -> Result<Vec<LayoutOp>, String> {
    let screen = output.unwrap_or_default();
    let rule = layout
        .outputs
        .iter()
        .filter(|rule| rule.matches.matches(screen))
        .max_by_key(|rule| rule.matches.specificity())
        .ok_or_else(|| telar::t!("editor.popover.no_rule", output = screen))?;
    let mut site = Site {
        output: rule.matches.clone(),
        workspace: None,
        layer,
    };
    let mut ops = Vec::new();
    if let Some(workspace) = workspace {
        if layer == LayerKind::Lock {
            return Err(telar::t!("editor.variant.lock"));
        }
        site.workspace = Some(workspace.clone());
        if !rule
            .workspaces
            .iter()
            .any(|held| held.matches == *workspace)
        {
            ops.push(LayoutOp::InsertWorkspaceRule {
                output: rule.matches.clone(),
                index: rule.workspaces.len(),
                rule: Box::new(WorkspaceRule {
                    matches: workspace.clone(),
                    ..WorkspaceRule::default()
                }),
            });
        }
    }
    let mut remove = layout::ops::layer(layout, &site)
        .map(|held| held.remove.clone())
        .unwrap_or_default();
    if !remove.contains(id) {
        remove.push(id.clone());
    }
    ops.push(LayoutOp::SetLayerRemove { site, remove });
    Ok(ops)
}

/// One edit being planned on one screen: the layout as far as it has got, and the operations that got it there. Every step is written where the layout as it stands then decides what it changes, for the workspace the mode edits ([`crate::variant::editing`]).
pub(crate) struct Work<'a> {
    pub(crate) layout: Layout,
    pub(crate) known: BTreeMap<LayoutId, Layout>,
    pub(crate) desktop: &'a Desktop,
    /// The layer the edit is about, which the modes' planners read their areas on.
    pub(crate) layer: LayerKind,
    pub(crate) workspace: Option<WorkspaceMatch>,
    ops: Vec<LayoutOp>,
}

impl<'a> Work<'a> {
    pub(crate) fn new(layout: &Layout, desktop: &'a Desktop, layer: LayerKind) -> Self {
        Self {
            layout: layout.clone(),
            known: known(),
            desktop,
            layer,
            workspace: crate::variant::editing(),
            ops: Vec::new(),
        }
    }

    pub(crate) fn apply(&mut self, ops: Vec<LayoutOp>) -> Result<(), EditError> {
        layout::ops::apply_all(&mut self.layout, &ops)?;
        self.ops.extend(ops);
        Ok(())
    }

    /// The screen as the layout planned so far arranges it.
    pub(crate) fn screen(&self) -> Desktop {
        self.desktop.resolving(&self.layout, &self.known)
    }

    /// The area `id` of `layer` as the layout planned so far resolves it.
    pub(crate) fn area(&self, layer: LayerKind, id: &AreaId) -> Result<ResolvedArea, EditError> {
        self.screen()
            .resolved
            .area(layer, id)
            .cloned()
            .ok_or_else(|| EditError::gone(id))
    }

    pub(crate) fn written(&self, layer: LayerKind, id: &AreaId) -> Result<Written, EditError> {
        Written::area(
            &self.layout,
            self.desktop.output.as_deref(),
            layer,
            id,
            self.workspace.as_ref(),
        )
        .map_err(EditError::Refused)
    }

    /// The area `id` of `layer` changed by `change`, where the layout writes it; nothing where `change` leaves it as it was.
    pub(crate) fn rewrite(
        &mut self,
        layer: LayerKind,
        id: &AreaId,
        change: impl FnOnce(&mut Area),
    ) -> Result<(), EditError> {
        let written = self.written(layer, id)?;
        let mut area = written.area.clone();
        change(&mut area);
        if area == written.area {
            return Ok(());
        }
        self.apply(written.ops(&area))
    }

    /// `area`, new on `layer`, written in the narrowest rule that covers the screen ([`Written::area`]).
    pub(crate) fn add(&mut self, layer: LayerKind, area: Area) -> Result<(), EditError> {
        let written = self.written(layer, &area.id)?;
        self.apply(written.ops(&area))
    }

    /// `area` put right after `beside` on `layer`, in the rule that writes `beside` — made first where it is a workspace's rule the layout has none for yet.
    pub(crate) fn insert_after(
        &mut self,
        layer: LayerKind,
        beside: &AreaId,
        area: Area,
    ) -> Result<(), EditError> {
        let written = self.written(layer, beside)?;
        let mut ops = written.ops(&written.area);
        ops.retain(|op| matches!(op, LayoutOp::InsertWorkspaceRule { .. }));
        let written_now = layout::ops::areas_at(&self.layout, &written.site);
        let index = match ops.is_empty() {
            true => written_now
                .iter()
                .position(|held| held.id == *beside)
                .map_or(written_now.len(), |at| at + 1),
            false => 0,
        };
        ops.push(LayoutOp::InsertArea {
            site: written.site,
            index,
            area: Box::new(area),
        });
        self.apply(ops)
    }

    /// The area `id` taken off `layer` of the screen ([`area_removal`]).
    pub(crate) fn remove(&mut self, layer: LayerKind, id: &AreaId) -> Result<(), EditError> {
        let ops = area_removal(
            &self.layout,
            &self.known,
            &self.screen().resolved,
            layer,
            id,
            self.workspace.as_ref(),
        )
        .map_err(EditError::Refused)?;
        self.apply(ops)
    }

    pub(crate) fn done(self) -> Vec<LayoutOp> {
        self.ops
    }
}

/// Every layout the running shell's store holds, which is what an extended layout is looked up in.
pub(crate) fn known() -> BTreeMap<LayoutId, Layout> {
    surfaces::layouts::read(|store| store.all().clone()).unwrap_or_default()
}

/// Where a new entry goes in `layer`: after everything, so it overrides without restacking — except under the lock's prompt where the same layer writes it, since nothing may be stacked over the prompt (TA-8).
fn new_at(layer: &layout::Layer) -> usize {
    layer
        .areas
        .iter()
        .position(|area| matches!(area.kind, Some(layout::AreaKind::Prompt { .. })))
        .unwrap_or(layer.areas.len())
}

/// An entry naming only `id`, which a partial override fills field by field.
fn blank(id: &AreaId) -> Area {
    Area {
        id: id.clone(),
        ..Area::default()
    }
}

/// An instance as the rule that decides it writes it.
#[derive(Clone, Debug, PartialEq)]
pub struct WrittenInstance {
    pub area: Written,
    pub group: GroupId,
    pub instance: Instance,
    present: bool,
}

impl WrittenInstance {
    /// The operations that write `changed` where this instance was read from: the instance alone where the rule writes it, else the entries that lay it over the inherited one.
    pub fn ops(&self, changed: &Instance) -> Vec<LayoutOp> {
        if self.present {
            return vec![LayoutOp::SetInstance {
                spot: self.spot(),
                id: changed.id.clone(),
                instance: Box::new(changed.clone()),
            }];
        }
        let mut area = self.area.area.clone();
        let group = match area.groups.iter().position(|held| held.id == self.group) {
            Some(at) => &mut area.groups[at],
            None => {
                area.groups.push(Group {
                    id: self.group.clone(),
                    ..Group::default()
                });
                area.groups.last_mut().expect("a group was just pushed")
            }
        };
        match group
            .children
            .iter_mut()
            .find(|child| child.id == changed.id)
        {
            Some(child) => *child = changed.clone(),
            None => group.children.push(changed.clone()),
        }
        self.area.ops(&area)
    }

    /// The operations that take this instance out: from where the rule writes it, else by naming it in its group's `remove` over the inherited one.
    pub fn removal(&self) -> Vec<LayoutOp> {
        if self.present {
            return vec![LayoutOp::DeleteInstance {
                spot: self.spot(),
                id: self.instance.id.clone(),
            }];
        }
        let mut area = self.area.area.clone();
        match area.groups.iter_mut().find(|held| held.id == self.group) {
            Some(group) => group.remove.push(self.instance.id.clone()),
            None => area.groups.push(Group {
                id: self.group.clone(),
                remove: vec![self.instance.id.clone()],
                ..Group::default()
            }),
        }
        self.area.ops(&area)
    }

    fn spot(&self) -> Spot {
        Spot {
            site: self.area.site.clone(),
            area: self.area.area.id.clone(),
            group: self.group.clone(),
        }
    }
}
