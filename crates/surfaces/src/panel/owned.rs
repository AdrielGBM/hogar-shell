//! The panel the layout gives an instance: opened and closed by that instance, drawn in its window as a drawer is, and kept open across a reload for as long as the layout still gives it one.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use telar::{Container, LayoutItem, ReactiveList, Rect, Shadow, StyledContainer};

use config::theme::NordTheme;
use config::{Config, Edge};
use layout::{InstanceId, LayerKind, Resolved, ResolvedArea, ResolvedAreaKind, ResolvedLayer};
use platform_wayland::KeyboardMode;
use ui::chrome::Chrome;
use ui::descriptor::Built;
use ui::host::Audience;
use ui::layout::{fill, painted_chrome};
use ui::scale::elevation;

use crate::area::{BACKDROP_BLUR, Surround, empty_space, padded, panel_cells};
use crate::expressions::{self, Expressions};
use crate::layer_window::{Blur, LayerWindowContext, Reserved, blur_of};
use crate::look::{self, Look, Rest};
use crate::panel_area::{BarSite, PanelShape, place};
use crate::reconcile::{with_desktop, with_desktop_now};
use crate::rects::{self, Node, Part};
use crate::transient::{self, Motion, Owned, Place, Slot, Spec};

/// An instance on one output, which is what an open panel is kept by: the same owner moved to another bar keeps its panel open.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Owner {
    pub output: Option<String>,
    pub instance: InstanceId,
}

impl Owner {
    pub fn of(node: &Node) -> Option<Self> {
        match &node.part {
            Part::Instance(_, instance) => Some(Self {
                output: node.output.clone(),
                instance: instance.clone(),
            }),
            _ => None,
        }
    }

    /// The id of the transient it opens, `<instance>@<output>` with an empty output where there is none, which no module id can be: `panel list` names it beside the module panels.
    pub fn id(&self) -> String {
        format!(
            "{}@{}",
            self.instance,
            self.output.as_deref().unwrap_or_default()
        )
    }
}

/// The panel an owner opens and the area the owner sits in, on the layer both are written on.
#[derive(Clone)]
struct Found {
    layer: LayerKind,
    panel: ResolvedArea,
    holder: ResolvedArea,
}

impl Found {
    fn along(&self) -> bool {
        matches!(self.panel.kind, ResolvedAreaKind::Panel { along: true, .. })
    }

    /// The edge the owner's strip hangs off, for an owner in a bar or a dock.
    fn edge(&self) -> Option<Edge> {
        match self.holder.kind {
            ResolvedAreaKind::Bar { edge, .. } | ResolvedAreaKind::Dock { edge, .. } => Some(edge),
            _ => None,
        }
    }

    fn owner_node(&self, owner: &Owner) -> Option<Node> {
        let group = self.holder.groups.iter().find(|group| {
            group
                .children
                .iter()
                .any(|child| child.id == owner.instance)
        })?;
        Some(
            Node::area(owner.output.as_deref(), self.layer, &self.holder.id)
                .instance(&group.id, &owner.instance),
        )
    }

    fn shape(&self) -> Option<PanelShape> {
        let ResolvedAreaKind::Panel {
            cols,
            rows,
            cell,
            gap,
            ..
        } = self.panel.kind
        else {
            return None;
        };
        Some(PanelShape {
            cols,
            rows,
            cell,
            gap,
            padding: self.panel.style.padding.unwrap_or_default(),
        })
    }
}

/// The area on `layer` holding the instance `owner`, a panel aside: the bar, dock, grid or container its panel opens from.
pub fn owner_area<'a>(layer: &'a ResolvedLayer, owner: &InstanceId) -> Option<&'a ResolvedArea> {
    layer.areas.iter().find(|area| {
        !matches!(area.kind, ResolvedAreaKind::Panel { .. })
            && area
                .groups
                .iter()
                .any(|group| group.children.iter().any(|child| child.id == *owner))
    })
}

fn find(resolved: &Resolved, instance: &InstanceId) -> Option<Found> {
    let (layer, panel) = resolved.areas().find(|(_, area)| {
        matches!(&area.kind, ResolvedAreaKind::Panel { owner, .. } if owner == instance)
    })?;
    let holder = owner_area(resolved.layer(layer)?, instance)?;
    Some(Found {
        layer,
        panel: panel.clone(),
        holder: holder.clone(),
    })
}

/// What the windows show now, without following it: a press and a reload ask once.
fn found_now(owner: &Owner) -> Option<Found> {
    with_desktop_now(owner.output.as_deref(), |desktop| {
        find(&desktop.resolved, &owner.instance)
    })
    .flatten()
}

/// The screen a panel is drawn on, without the arrangement it was found in.
#[derive(Clone)]
struct Screen {
    output: Option<String>,
    config: Arc<Config>,
    reserved: Reserved,
    size: (f32, f32),
}

/// What the windows show, followed: read inside an effect or a build, it runs again on the next reconcile or preview.
fn found(owner: &Owner, layer: LayerKind) -> Option<(Found, Screen)> {
    with_desktop(owner.output.as_deref(), |desktop| {
        let found =
            find(&desktop.resolved, &owner.instance).filter(|found| found.layer == layer)?;
        let screen = Screen {
            output: desktop.output.clone(),
            config: Arc::clone(&desktop.config),
            reserved: desktop.reserved,
            size: desktop.size,
        };
        Some((found, screen))
    })
    .flatten()
}

thread_local! {
    /// Every owner whose panel was opened and the layer it was opened on, while it may still be open.
    static OPENED: RefCell<Vec<(Owner, LayerKind)>> = const { RefCell::new(Vec::new()) };
}

/// Whether the instance at `node` owns a panel in the layout the windows show. Asked of every instance a window builds, so it clones nothing.
pub fn owns_panel(node: &Node) -> bool {
    let Some(owner) = Owner::of(node) else {
        return false;
    };
    with_desktop_now(owner.output.as_deref(), |desktop| {
        desktop.resolved.layer(node.layer).is_some_and(|layer| {
            layer.areas.iter().any(|area| {
                matches!(&area.kind, ResolvedAreaKind::Panel { owner: held, .. } if *held == owner.instance)
            })
        })
    })
    .unwrap_or(false)
}

pub fn is_open(owner: &Owner) -> bool {
    transient::is_open(&owner.id())
}

/// Toggles the panel `owner` has, answering whether it has one.
pub fn toggle(owner: &Owner) -> bool {
    if is_open(owner) {
        close(owner);
        return true;
    }
    open(owner)
}

/// Opens the panel `owner` has, answering whether it has one. Opened as a drawer is: one at a time with every other drawer, closed by a press outside it, Esc, or any window opening.
pub fn open(owner: &Owner) -> bool {
    let Some(found) = found_now(owner) else {
        return false;
    };
    if is_open(owner) || !may_open(owner, &found) {
        return true;
    }
    crate::popout::close();
    let motion = found.edge().map_or(Motion::Fade, Motion::Slide);
    let layer = found.layer;
    let placing = owner.clone();
    let building = owner.clone();
    let casting = owner.clone();
    let last = Cell::new(Rect::default());
    let spec = Spec::new(
        owner.id(),
        Place::Owned(Owned {
            output: owner.output.clone(),
            layer,
            rect: Rc::new(move |usable| {
                let rect = rect_of(&placing, layer, usable).unwrap_or(last.get());
                last.set(rect);
                rect
            }),
        }),
        Rc::new(move |_: &Chrome| content(&building, layer)),
    )
    .slot(Slot::Drawer)
    .dismiss_on_outside()
    .motion(motion)
    .shadow(move || shadow_of(&casting, layer))
    .keyboard(KeyboardMode::OnDemand)
    .output(owner.output.clone())
    .from(found.owner_node(owner))
    .holding(Node::area(owner.output.as_deref(), layer, &found.panel.id));
    OPENED.with(|opened| {
        let mut opened = opened.borrow_mut();
        opened.retain(|(held, _)| held != owner && transient::is_open(&held.id()));
        opened.push((owner.clone(), layer));
    });
    transient::open(spec);
    true
}

fn shadow_of(owner: &Owner, layer: LayerKind) -> Option<Shadow> {
    let (found, _) = found(owner, layer)?;
    found.panel.style.shadow.and_then(elevation::shadow)
}

/// A panel its `visible` hides stays shut, except in its layer's edit mode, which draws it dim so it can be selected.
fn may_open(owner: &Owner, found: &Found) -> bool {
    let Some(visible) = &found.panel.visible else {
        return true;
    };
    expressions::is_edited(owner.output.as_deref(), found.layer)
        || Expressions::here(Audience::Owner).is_shown_now(visible)
}

pub fn close(owner: &Owner) {
    transient::close(&owner.id());
}

/// Closes every open panel whose owner the layout `desktops` gives none any more, or moved to another layer: a reload keeps the others open.
pub(crate) fn prune(desktops: &[(Option<&str>, &Resolved)]) {
    let gone: Vec<Owner> = OPENED.with(|opened| {
        let mut opened = opened.borrow_mut();
        opened.retain(|(owner, _)| transient::is_open(&owner.id()));
        opened
            .iter()
            .filter(|(owner, layer)| {
                let still = desktops
                    .iter()
                    .find(|(output, _)| *output == owner.output.as_deref())
                    .and_then(|(_, resolved)| find(resolved, &owner.instance));
                still.is_none_or(|found| found.layer != *layer)
            })
            .map(|(owner, _)| owner.clone())
            .collect()
    });
    for owner in gone {
        close(&owner);
    }
}

/// Where the panel sits now: beside its owner, or along its owner's bar, inside what the reserving areas leave. Reactive, so it follows the owner, the bar and the layout; `None` while the owner or its bar is between a rebuild and its first layout.
fn rect_of(owner: &Owner, layer: LayerKind, usable: Rect) -> Option<Rect> {
    let output = owner.output.as_deref();
    let (found, _) = found(owner, layer)?;
    let shape = found.shape()?;
    let (_, at) = rects::instance(output, &owner.instance).filter(|(_, at)| laid_out(*at))?;
    let bar = match found.edge() {
        Some(edge) => Some(BarSite {
            edge,
            strip: rects::rect(&Node::area(output, layer, &found.holder.id))
                .filter(|strip| laid_out(*strip))?,
        }),
        None => None,
    };
    Some(place(at, bar, found.along(), usable, shape).rect)
}

fn laid_out(rect: Rect) -> bool {
    rect.width > 0.0 || rect.height > 0.0
}

/// The panel's box, built again whenever the layout changes what the panel is, and empty while a preview shows it gone.
fn content(owner: &Owner, layer: LayerKind) -> Built {
    let holder = owner.clone();
    let owner = owner.clone();
    let seen: RefCell<(u64, Option<ResolvedArea>)> = RefCell::new((0, None));
    let shown = move || -> Vec<(u64, Found, Screen)> {
        let Some((found, screen)) = found(&owner, layer) else {
            return Vec::new();
        };
        let mut seen = seen.borrow_mut();
        if seen.1.as_ref() != Some(&found.panel) {
            *seen = (seen.0.wrapping_add(1), Some(found.panel.clone()));
        }
        vec![(seen.0, found, screen)]
    };
    let list = ReactiveList::with_style(
        fill(),
        shown,
        |(version, _, _): &(u64, Found, Screen)| *version,
        move |(_, found, screen)| panel_box(&holder, &found, &screen),
    )?;
    Ok(Box::new(list))
}

fn panel_box(owner: &Owner, found: &Found, screen: &Screen) -> Built {
    let theme = screen.config.resolve_theme();
    let surround = Surround {
        config: &screen.config,
        theme,
        output: screen.output.as_deref(),
        layer: found.layer,
        bounds: screen.reserved.box_of(found.panel.within, screen.size),
        reserved: screen.reserved,
        audience: Audience::Owner,
    };
    let panel = &found.panel;
    let node = Node::area(screen.output.as_deref(), found.layer, &panel.id);
    let presence = panel.visible.as_ref().map(|visible| {
        let presence = Expressions::here(Audience::Owner).visible(&node, visible);
        let owner = owner.clone();
        telar::effect(move || {
            if !presence.displayed() {
                close(&owner);
            }
        });
        presence
    });
    let look = look_of(panel, found.along(), &screen.config, &theme);
    let window = LayerWindowContext::current();
    let blur = window
        .as_ref()
        .and_then(|window| blur_of(window.layer, panel));
    let cells = look::cut(
        Box::new(Container::new(
            padded(fill().flex_column(), panel.style.padding),
            vec![panel_cells(panel, surround)?],
        )?),
        look.radius,
        false,
    );
    let painted = StyledContainer::new(
        fill().flex_column(),
        move |rect| look.paint(rect),
        vec![Box::new(cells)],
    )?;
    let painted = match blur {
        Some(Blur::InSurface) => painted.with_backdrop_blur(|| BACKDROP_BLUR),
        Some(Blur::Compositor) | None => painted,
    };
    let painted = painted_chrome(painted, look.fill);
    let root = empty_space(panel, surround, painted, true);
    let root = match presence {
        Some(presence) => root.with_opacity(move || presence.opacity()),
        None => root,
    };
    if let (Some(Blur::Compositor), Some(window)) = (blur, &window)
        && let Some(rect) = telar::track_layout(root.layout_node())
    {
        window.demands.blur_behind(rect);
    }
    rects::track(node, root.layout_node());
    Ok(Box::new(root) as Box<dyn LayoutItem>)
}

/// The panel's own style laid over what a panel rests on: the theme's surface at `[theme] opacity`, square along a bar and at the theme's radius anywhere else.
fn look_of(panel: &ResolvedArea, along: bool, config: &Config, theme: &NordTheme) -> Look {
    let radius = look::rest_radius(&panel.kind, along, config).largest();
    let rest = Rest::on_surface(theme, config.opacity(), radius);
    Look::of_instance(&panel.style, None, theme, rest).unwrap_or_else(|| Look::resting(rest))
}
