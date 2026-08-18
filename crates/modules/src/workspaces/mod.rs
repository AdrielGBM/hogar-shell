//! Which workspaces the bar shows, and what each pill says.

use std::cell::RefCell;
use std::rc::Rc;

use telar::motion::{Animated, Spring};
use telar::{
    AlignItems, Canvas, Color, Container, JustifyContent, LayoutError, LayoutItem, LayoutStyle,
    ReactiveList, ReadSignal, Rect, RectStyle, RenderNode, RwSignal, StyledContainer, Text,
    box_item, signal, track_layout,
};

use config::WorkspacesConfig;
use config::theme::{FontRole, NordTheme};
use services::hyprland::{Snapshot, Workspace};
use ui::icon::{app_icon_view, icon_view};

/// The gap between pills, and what the indicator must not spill into.
const PILL_GAP: f32 = 8.0;

/// "Nowhere yet": what the active-pill slot holds before any pill has been laid out.
const ZERO_RECT: Rect = Rect {
    x: 0.0,
    y: 0.0,
    width: 0.0,
    height: 0.0,
};

/// A pill the bar draws: either a workspace that exists, or a placeholder holding its slot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pill {
    pub id: i32,
    pub label: String,
    pub occupied: bool,
    pub active: bool,
    pub special: bool,
    /// The window classes to draw icons for, already capped.
    pub clients: Vec<String>,
    /// An Iconify glyph that replaces the label — configured per special workspace.
    pub icon: Option<String>,
}

impl Pill {
    /// What the view keys its list on: everything the pill draws, not just its id.
    ///
    /// A keyed list rebuilds an item only when its key changes, and a workspace keeps its id while its window
    /// set turns over — so keying on the id alone leaves a pill showing the icons of applications that closed.
    pub fn key(&self) -> String {
        let mut key = format!(
            "{}|{}|{}{}",
            self.id,
            self.label,
            u8::from(self.active),
            u8::from(self.occupied)
        );
        for client in &self.clients {
            key.push('|');
            key.push_str(client);
        }
        key
    }
}

/// Everything a pill paints itself with that comes from the bar rather than from the workspace.
#[derive(Clone, Copy)]
pub struct PillStyle {
    pub theme: NordTheme,
    pub radius: f32,
    /// The bar's thickness, which is the pill's square side before any window icons widen it.
    pub side: f32,
    pub vertical: bool,
    pub occupied_background: bool,
    /// Whether a sliding indicator paints the active pill. When it does, the pill must not paint its own accent
    /// fill: two accents in the same place is the indicator arriving on top of a pill that already recoloured,
    /// which reads as no animation at all.
    pub indicator: bool,
}

/// The three states, three fills: the active pill takes the accent, an occupied one the surface token so it
/// reads as "something lives here", and an empty one the bar's own background so it recedes.
///
/// With the sliding indicator on, none of them paints anything. The indicator is one box *under* the row — it
/// has to be, or it would cover the label it marks — so every opaque fill in the row is something it travels
/// behind: the active pill hid it where it landed, and its neighbours hid it the whole way there, leaving a box
/// that only exists at its destination. Occupancy still reads, from the label colour, which is where the
/// difference between an occupied and an empty workspace was already carried.
fn fill_for(pill: &Pill, style: PillStyle) -> Color {
    if style.indicator {
        Color::TRANSPARENT
    } else if pill.active {
        style.theme.accent
    } else if pill.occupied && style.occupied_background {
        style.theme.surface
    } else {
        style.theme.base
    }
}

fn text_for(pill: &Pill, style: PillStyle) -> Color {
    if pill.active {
        style.theme.base
    } else if pill.occupied {
        style.theme.text
    } else {
        style.theme.muted
    }
}

/// One workspace pill, built in Rust rather than in the view because the view's `for` is reactive: it
/// constructs each item afresh whenever that workspace's key changes, so its content has to be an expression
/// (`build`) rather than a widget bound once in `[logic]`.
pub fn pill_view(
    pill: Pill,
    style: PillStyle,
    on_press: impl Fn(i32) + 'static,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    tracked_pill_view(pill, style, on_press, None)
}

/// [`pill_view`], with the active pill reporting where it landed so the indicator can follow it.
///
/// The rect has to come from the pill rather than be worked out from an index: a pill carrying window icons is
/// wider than a bare one, so "the third slot" is not a position the row can compute.
fn tracked_pill_view(
    pill: Pill,
    style: PillStyle,
    on_press: impl Fn(i32) + 'static,
    active_rect: Option<RwSignal<Rect>>,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let fg = text_for(&pill, style);
    let fill = fill_for(&pill, style);
    let icon_size = (style.side * 0.5).round().clamp(8.0, 32.0);

    let mut content: Vec<Box<dyn LayoutItem>> = Vec::new();
    match pill.icon.clone() {
        Some(glyph) => content.push(icon_view(move || glyph.clone(), move || fg, icon_size)?),
        None => {
            let label = pill.label.clone();
            let theme = style.theme;
            content.push(box_item(Text::auto(
                move || label.clone(),
                LayoutStyle::new(),
                move || theme.text_style(FontRole::Caption, fg),
            )?));
        }
    }
    // A class with no installed icon contributes nothing: a row of identical fallbacks says less than the window count the pill already implies.
    for class in &pill.clients {
        if let Some(icon) = app_icon_view(class, icon_size * 0.8)? {
            content.push(icon);
        }
    }

    // `min_*` on the axis the pill grows along, so window icons widen it instead of squashing, while a bare pill stays exactly as square as before.
    let inner = if style.vertical {
        LayoutStyle::new()
            .flex_column()
            .width(style.side)
            .min_height(style.side)
    } else {
        LayoutStyle::new()
            .flex_row()
            .min_width(style.side)
            .height(style.side)
    };
    let inner = inner
        .align_items(AlignItems::CENTER)
        .justify_content(JustifyContent::CENTER)
        .padding_horizontal(if pill.clients.is_empty() { 0.0 } else { 4.0 })
        .gap(if pill.clients.is_empty() { 0.0 } else { 3.0 })
        .flex_shrink(0.0);

    let id = pill.id;
    let container = StyledContainer::new(
        inner,
        move |_r| RectStyle::filled(fill, style.radius),
        content,
    )?;

    // Only the active pill is tracked. Every pill reporting its rect would be a signal write per pill per
    // layout pass, to answer a question about exactly one of them.
    let mut tracking = Vec::new();
    if let Some(slot) = active_rect.filter(|_| pill.active)
        && let Some(rect) = track_layout(container.layout_node())
    {
        tracking.push(telar::effect(move || {
            let rect = rect.get();
            // A rebuilt pill's node is laid out at zero before its first pass; reporting that would send the
            // indicator to the corner and back on every workspace change.
            if rect.width > 0.0 && rect.height > 0.0 {
                slot.set(rect);
            }
        }));
    }

    // The subscription lives exactly as long as the pill: the list rebuilds its rows, and an effect outliving
    // one would keep reporting a rect for a workspace that is no longer active.
    Ok(Box::new(telar::Holding::new(
        Box::new(container.on_press(move || on_press(id))),
        tracking,
    )))
}

/// The pills, with the active-workspace indicator sliding behind them.
///
/// The indicator is one box that moves, not a fill each pill paints: it is a canvas laid over the whole row,
/// painting its rect wherever the active pill landed. That distinction is the whole feature — the canvas sits
/// outside the flow, so the row does not reflow sixty times a second while the indicator travels, and the
/// pills underneath never move.
pub fn grid(
    items: ReadSignal<Vec<Pill>>,
    style: PillStyle,
    on_press: fn(i32),
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let slot = signal(ZERO_RECT);
    let for_rows = slot.clone();
    // `with_style` rather than `with_gap`: the gap constructors hardcode a column, so a bottom bar would stack
    // its pills downwards inside a strip one pill high and show nothing at all.
    let axis = if style.vertical {
        LayoutStyle::new().flex_column()
    } else {
        LayoutStyle::new().flex_row()
    }
    .align_items(AlignItems::CENTER);
    let rows = ReactiveList::with_style(
        axis.clone().gap(PILL_GAP),
        move || items.get(),
        |pill: &Pill| pill.key(),
        move |pill: Pill| {
            let slot = style.indicator.then(|| for_rows.clone());
            tracked_pill_view(pill, style, on_press, slot)
        },
    )?;

    let mut children: Vec<Box<dyn LayoutItem>> = Vec::with_capacity(2);
    if style.indicator {
        // First, so it paints under the pills: the label has to stay readable over the accent.
        children.push(indicator(slot, style)?);
    }
    children.push(Box::new(rows));
    Ok(Box::new(Container::new(axis, children)?))
}

/// The box that marks the active workspace, carried to it rather than repainted in place.
/// The spring the indicator chases its target with: `[animation] curve`, else the shell's own default. Read
/// from the surface's config rather than hardcoded, so one `[animation]` section governs every moving part.
fn indicator_spring() -> Spring {
    config::surface_env()
        .map(|env| env.config.animation.spring())
        .unwrap_or_else(Spring::gentle)
}

/// The box actually painted: `target` stretched along its direction of travel toward `goal`.
///
/// Not new machinery — the same animated rect, drawn as the union of where it is and a `trail` fraction of
/// where it is still going. That makes it exactly one pill wide the instant it arrives (the distance is zero,
/// so the union is the rect itself) and longest at the moment it is moving fastest, which is what reads as
/// speed rather than as a box that grew.
fn with_trail(target: Rect, goal: Rect, trail: f32) -> Rect {
    if trail <= 0.0 {
        return target;
    }
    let lead_x = (goal.x - target.x) * trail;
    let lead_y = (goal.y - target.y) * trail;
    Rect {
        x: target.x + lead_x.min(0.0),
        y: target.y + lead_y.min(0.0),
        width: target.width + lead_x.abs(),
        height: target.height + lead_y.abs(),
    }
}

/// How far the indicator stretches while travelling, as a fraction of the distance left to cover. `0` is the
/// square box that was there before; the config bounds it below `1`, where the trail would reach the whole way
/// to the goal and read as one long bar rather than as motion.
fn indicator_trail() -> f32 {
    config::surface_env()
        .map(|env| env.config.workspaces.trail())
        .unwrap_or(0.0)
}

fn indicator(slot: RwSignal<Rect>, style: PillStyle) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let trail = indicator_trail();
    // Built on the first target rather than at construction: an `Animated` seeded with a zero rect would
    // travel out of the corner the first time the bar ever draws, which reads as a glitch rather than as the
    // motion this exists for. A spring rather than a tween because it keeps velocity through a retarget —
    // holding a workspace key down should bend the indicator's path, not restart it from a standstill.
    let motion: Rc<RefCell<Option<Animated<Rect>>>> = Rc::new(RefCell::new(None));

    let follow = {
        let motion = Rc::clone(&motion);
        let source = slot.read_only();
        telar::effect(move || {
            let wanted = source.get();
            if wanted.width <= 0.0 || wanted.height <= 0.0 {
                return;
            }
            // Cloned out before `retarget`, which writes a signal and flushes: reaching back through the
            // `RefCell` while this one is still borrowed is the re-entrant panic, not a compile error.
            let existing = motion.borrow().clone();
            match existing {
                Some(animated) => animated.retarget(wanted),
                None => {
                    // Seeded collapsed on the goal's own centre and retargeted at once, so the indicator grows
                    // into place on the workspace it marks rather than arriving from nowhere.
                    //
                    // The appearance is the smaller half of it. An `Animated` created already at its goal is
                    // born *settled*, and a settled animation never registers with the ticker — so nothing
                    // scheduled the frame that would have painted the indicator for the first time, and it
                    // stayed invisible until some unrelated event forced a redraw. Starting it in motion is
                    // what makes the loop keep drawing frames until it arrives.
                    let seed = Rect {
                        x: wanted.x + wanted.width / 2.0,
                        y: wanted.y + wanted.height / 2.0,
                        width: 0.0,
                        height: 0.0,
                    };
                    let animated = Animated::new(seed, indicator_spring());
                    animated.retarget(wanted);
                    *motion.borrow_mut() = Some(animated);
                }
            }
        })
    };

    // Where the row itself sits. The pill rects are absolute and a canvas paints in its own local space, so
    // the row's origin is what converts between them. Filled in after construction, below.
    let origin = signal(ZERO_RECT);
    let painted = origin.read_only();

    let accent = style.theme.accent;
    let radius = style.radius;
    let wanted = slot.read_only();
    let canvas = Canvas::new(LayoutStyle::new().absolute_fill(), move |_local| {
        // Both read unconditionally, before anything can return early. `motion` lives in a `RefCell`, not a
        // signal, so reading only *it* subscribes this canvas to nothing: while it was still `None` the
        // indicator had no reason to repaint when a pill finally reported its rect, and stayed invisible until
        // some unrelated event — moving the pointer over the bar — forced a redraw.
        let goal = wanted.get();
        let row = painted.get();
        // Nothing to point at: the active workspace is on another monitor, or scrolled out of a fixed window.
        if goal.width <= 0.0 || goal.height <= 0.0 {
            return RenderNode::Empty;
        }
        // Painted rather than transformed. Scaling one box down to a pill would squash its corner radius with
        // it — the row is far wider than a pill, so the rounding came out flattened on one axis — and drawing
        // the rect where it belongs costs one command either way.
        //
        // The raw goal until the animation exists, so the first paint lands in the right place rather than
        // waiting a frame for the spring to be built.
        let target = motion
            .borrow()
            .clone()
            .map(|animated| animated.get())
            .unwrap_or(goal);
        if target.width <= 0.0 || target.height <= 0.0 {
            return RenderNode::Empty;
        }
        let drawn = with_trail(target, goal, trail);
        RenderNode::rect(
            Rect {
                x: drawn.x - row.x,
                y: drawn.y - row.y,
                width: drawn.width,
                height: drawn.height,
            },
            RectStyle::filled(accent, radius),
        )
    })?;

    // The canvas has to outlive both: a handle that drops deregisters its effect.
    let mut held = vec![follow];
    if let Some(rect) = track_layout(canvas.layout_node()) {
        held.push(telar::effect(move || origin.set(rect.get())));
    }
    Ok(Box::new(telar::Holding::new(Box::new(canvas), held)))
}

/// The pills to draw for a snapshot.
///
/// With `shown = 0` this is exactly the workspaces that exist. With `shown = N` the bar always draws N pills,
/// filling gaps with placeholders — which is the point: a bar whose width changes every time a workspace is
/// created or destroyed makes every module to its right jump around.
pub fn pills(snapshot: &Snapshot, config: &WorkspacesConfig, output: Option<&str>) -> Vec<Pill> {
    let mut visible: Vec<&Workspace> = snapshot
        .workspaces
        .iter()
        .filter(|w| !w.is_special() && belongs_here(w, config, output, snapshot))
        .collect();
    visible.sort_by_key(|w| w.id);

    let mut pills: Vec<Pill> = if config.shown == 0 {
        visible
            .iter()
            .enumerate()
            .map(|(index, w)| pill_for(w, config, index, snapshot.active))
            .collect()
    } else {
        fixed_window(&visible, config, snapshot.active)
    };

    if config.show_special {
        let specials = snapshot
            .workspaces
            .iter()
            .filter(|w| w.is_special() && w.is_occupied())
            .filter(|w| belongs_here(w, config, output, snapshot));
        for (index, w) in specials.enumerate() {
            pills.push(pill_for(w, config, index, snapshot.active));
        }
    }
    pills
}

/// Whether a workspace belongs on this bar. Without `per_monitor` every bar shows every workspace; with it, a
/// bar shows only its own monitor's — which is what a multi-head setup usually means by "my workspaces".
fn belongs_here(
    workspace: &Workspace,
    config: &WorkspacesConfig,
    output: Option<&str>,
    snapshot: &Snapshot,
) -> bool {
    if !config.per_monitor {
        return true;
    }
    // A bar with no output name (a single-monitor setup, or a surface the compositor didn't name) falls back to
    // the focused monitor rather than hiding everything.
    let mine = output.unwrap_or(&snapshot.focused_monitor);
    mine.is_empty() || workspace.monitor.is_empty() || workspace.monitor == mine
}

/// A fixed-width run of `shown` ids, anchored so the active workspace is always inside it.
///
/// Anchoring on the active one is what lets `shown = 5` work on a setup that uses workspaces 1–20: the window
/// slides to follow you instead of stranding you off the end of a fixed 1–5.
fn fixed_window(visible: &[&Workspace], config: &WorkspacesConfig, active: i32) -> Vec<Pill> {
    let count = config.shown as i32;
    let lowest = visible.first().map(|w| w.id).unwrap_or(1).max(1);
    let start = if active >= lowest && active < lowest + count {
        lowest
    } else if active >= lowest + count {
        (active - count + 1).max(1)
    } else {
        active.max(1)
    };

    (0..count)
        .map(|offset| {
            let id = start + offset;
            match visible.iter().find(|w| w.id == id) {
                Some(w) => pill_for(w, config, offset as usize, active),
                // A slot with no workspace behind it is still clickable: pressing it is how you get there.
                None => Pill {
                    id,
                    label: config.render_label(
                        id,
                        &id.to_string(),
                        offset as usize,
                        false,
                        id == active,
                    ),
                    occupied: false,
                    active: id == active,
                    special: false,
                    clients: Vec::new(),
                    icon: None,
                },
            }
        })
        .collect()
}

fn pill_for(w: &Workspace, config: &WorkspacesConfig, index: usize, active: i32) -> Pill {
    let icon = if w.is_special() {
        config.special_icons.get(w.special_name()).cloned()
    } else {
        None
    };
    let is_active = w.id == active;
    let label = if w.is_special() {
        config.capitalize.apply(w.special_name())
    } else {
        config.render_label(w.id, &w.name, index, w.is_occupied(), is_active)
    };
    Pill {
        id: w.id,
        label,
        occupied: w.is_occupied(),
        active: is_active,
        special: w.is_special(),
        clients: if config.window_icons {
            w.clients
                .iter()
                .take(config.max_window_icons as usize)
                .cloned()
                .collect()
        } else {
            Vec::new()
        },
        icon,
    }
}

/// The workspace one wheel notch away from `active`, clamped to the ones that exist. Returns `None` when there
/// is nowhere to go, so the handler can leave the compositor alone rather than dispatching a no-op.
pub fn scroll_target(snapshot: &Snapshot, up: bool) -> Option<i32> {
    let mut ids: Vec<i32> = snapshot
        .workspaces
        .iter()
        .filter(|w| !w.is_special())
        .map(|w| w.id)
        .collect();
    ids.sort_unstable();
    let at = ids.iter().position(|id| *id == snapshot.active)?;
    let next = if up {
        at.checked_sub(1)?
    } else {
        at.checked_add(1).filter(|n| *n < ids.len())?
    };
    ids.get(next).copied()
}

/// The wheel over the pills switches workspace, when `[workspaces] scroll` allows it.
///
/// Reads the shared snapshot rather than the compositor: a wheel notch must not spend a socket round-trip
/// deciding where to go, and the service already holds the answer.
pub fn scroll(_dx: f32, dy: f32) {
    let enabled = config::config()
        .map(|c| c.workspaces.scroll)
        .unwrap_or(true);
    if !enabled {
        return;
    }
    let Some(snapshot) = services::hyprland::current_workspaces() else {
        return;
    };
    // `dy > 0` is a scroll up (the platform already flips Wayland's axis), which moves to the lower id.
    let Some(target) = scroll_target(&snapshot, dy > 0.0) else {
        return;
    };
    services::hyprland::focus_workspace_id(target);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ws(id: i32, windows: u32, monitor: &str) -> Workspace {
        Workspace {
            id,
            name: id.to_string(),
            windows,
            monitor: monitor.to_string(),
            ..Workspace::default()
        }
    }

    fn snapshot(workspaces: Vec<Workspace>, active: i32) -> Snapshot {
        Snapshot {
            workspaces,
            active,
            focused_monitor: "eDP-1".to_string(),
        }
    }

    #[test]
    fn shown_zero_draws_exactly_what_exists() {
        let snap = snapshot(vec![ws(1, 1, "eDP-1"), ws(4, 0, "eDP-1")], 1);
        let pills = pills(&snap, &WorkspacesConfig::default(), None);
        assert_eq!(pills.len(), 2);
        assert_eq!(pills[0].id, 1);
        assert_eq!(pills[1].id, 4, "sparse ids are shown as they are");
    }

    #[test]
    fn a_fixed_window_keeps_the_bar_width_stable() {
        let config = WorkspacesConfig {
            shown: 5,
            ..WorkspacesConfig::default()
        };
        let one = pills(&snapshot(vec![ws(1, 1, "eDP-1")], 1), &config, None);
        let many = pills(
            &snapshot(
                vec![ws(1, 1, "eDP-1"), ws(2, 1, "eDP-1"), ws(3, 0, "eDP-1")],
                1,
            ),
            &config,
            None,
        );
        assert_eq!(one.len(), 5, "empty slots hold the width");
        assert_eq!(many.len(), 5, "and it does not grow as workspaces appear");
        assert!(!one[3].occupied, "a placeholder is not occupied");
    }

    #[test]
    fn the_window_slides_to_keep_the_active_workspace_visible() {
        let config = WorkspacesConfig {
            shown: 5,
            ..WorkspacesConfig::default()
        };
        let workspaces: Vec<Workspace> = (1..=20).map(|id| ws(id, 1, "eDP-1")).collect();

        let early = pills(&snapshot(workspaces.clone(), 2), &config, None);
        assert_eq!(early.first().unwrap().id, 1);
        assert!(early.iter().any(|p| p.active && p.id == 2));

        // On workspace 12 a fixed 1–5 run would strand the user off the end.
        let late = pills(&snapshot(workspaces, 12), &config, None);
        assert!(
            late.iter().any(|p| p.active && p.id == 12),
            "the active workspace is always inside the window: {:?}",
            late.iter().map(|p| p.id).collect::<Vec<_>>()
        );
        assert_eq!(late.len(), 5);
    }

    #[test]
    fn per_monitor_filters_to_this_bars_output() {
        let config = WorkspacesConfig {
            per_monitor: true,
            ..WorkspacesConfig::default()
        };
        let snap = snapshot(
            vec![ws(1, 1, "eDP-1"), ws(2, 1, "DP-2"), ws(3, 1, "eDP-1")],
            1,
        );
        let mine = pills(&snap, &config, Some("eDP-1"));
        assert_eq!(
            mine.iter().map(|p| p.id).collect::<Vec<_>>(),
            vec![1, 3],
            "the other monitor's workspaces stay on the other monitor"
        );
        let all = pills(&snap, &WorkspacesConfig::default(), Some("eDP-1"));
        assert_eq!(
            all.len(),
            3,
            "without per_monitor every bar shows every one"
        );
    }

    #[test]
    fn special_workspaces_appear_only_when_occupied_and_can_carry_an_icon() {
        let mut special = ws(-99, 1, "eDP-1");
        special.name = "special:magic".to_string();
        let mut empty_special = ws(-98, 0, "eDP-1");
        empty_special.name = "special:unused".to_string();

        let mut config = WorkspacesConfig::default();
        config
            .special_icons
            .insert("magic".to_string(), "sparkles".to_string());

        let snap = snapshot(vec![ws(1, 1, "eDP-1"), special, empty_special], 1);
        let drawn = pills(&snap, &config, None);
        assert_eq!(drawn.len(), 2, "the empty scratchpad is not drawn");
        let magic = drawn.last().unwrap();
        assert!(magic.special);
        assert_eq!(magic.label, "magic", "the `special:` prefix is stripped");
        assert_eq!(magic.icon.as_deref(), Some("sparkles"));

        config.show_special = false;
        assert_eq!(pills(&snap, &config, None).len(), 1);
    }

    #[test]
    fn labels_render_from_the_template() {
        let config = WorkspacesConfig {
            label: "[{index}]".to_string(),
            ..WorkspacesConfig::default()
        };
        let snap = snapshot(vec![ws(7, 1, "eDP-1"), ws(9, 1, "eDP-1")], 7);
        let drawn = pills(&snap, &config, None);
        assert_eq!(drawn[0].label, "[1]", "index is the position, not the id");
        assert_eq!(drawn[1].label, "[2]");

        let by_id = WorkspacesConfig::default();
        assert_eq!(pills(&snap, &by_id, None)[0].label, "7");
    }

    #[test]
    fn window_icons_are_capped_and_off_by_default() {
        let mut w = ws(1, 6, "eDP-1");
        w.clients = vec!["a", "b", "c", "d", "e", "f"]
            .into_iter()
            .map(String::from)
            .collect();
        let snap = snapshot(vec![w], 1);

        assert!(
            pills(&snap, &WorkspacesConfig::default(), None)[0]
                .clients
                .is_empty(),
            "icons cost an icon lookup per window, so they are opt-in"
        );

        let config = WorkspacesConfig {
            window_icons: true,
            max_window_icons: 3,
            ..WorkspacesConfig::default()
        };
        assert_eq!(pills(&snap, &config, None)[0].clients.len(), 3);
    }

    #[test]
    fn a_pills_key_changes_when_its_windows_do() {
        // A keyed list rebuilds nothing whose key held still, and a workspace keeps its id while its windows turn over.
        let mut w = ws(1, 1, "eDP-1");
        w.clients = vec!["firefox".to_string()];
        let config = WorkspacesConfig {
            window_icons: true,
            max_window_icons: 4,
            ..WorkspacesConfig::default()
        };
        let before = pills(&snapshot(vec![w.clone()], 1), &config, None)[0].key();

        w.clients = vec!["firefox".to_string(), "kitty".to_string()];
        let after = pills(&snapshot(vec![w.clone()], 1), &config, None)[0].key();
        assert_ne!(before, after, "a new window has to rebuild the pill");

        let same = pills(&snapshot(vec![w], 1), &config, None)[0].key();
        assert_eq!(after, same, "an unchanged workspace keeps its key");
    }

    #[test]
    fn a_pills_key_tracks_focus_and_occupancy() {
        let config = WorkspacesConfig::default();
        let idle = pills(
            &snapshot(vec![ws(1, 0, "eDP-1"), ws(2, 0, "eDP-1")], 2),
            &config,
            None,
        );
        let focused = pills(
            &snapshot(vec![ws(1, 0, "eDP-1"), ws(2, 0, "eDP-1")], 1),
            &config,
            None,
        );
        assert_ne!(idle[0].key(), focused[0].key(), "focus repaints the pill");

        let occupied = pills(&snapshot(vec![ws(1, 3, "eDP-1")], 2), &config, None);
        let empty = pills(&snapshot(vec![ws(1, 0, "eDP-1")], 2), &config, None);
        assert_ne!(occupied[0].key(), empty[0].key());
    }

    #[test]
    fn a_pill_builds_on_every_edge_with_and_without_window_icons() {
        let style = |vertical| PillStyle {
            theme: NordTheme::new(),
            radius: 8.0,
            side: 32.0,
            vertical,
            occupied_background: true,
            indicator: false,
        };
        let bare = Pill {
            id: 1,
            label: "1".to_string(),
            occupied: false,
            active: true,
            special: false,
            clients: Vec::new(),
            icon: None,
        };
        let with_icons = Pill {
            clients: vec!["firefox".to_string(), "kitty".to_string()],
            active: false,
            occupied: true,
            ..bare.clone()
        };
        let special = Pill {
            icon: Some("sparkles".to_string()),
            special: true,
            ..bare.clone()
        };
        for vertical in [false, true] {
            for pill in [bare.clone(), with_icons.clone(), special.clone()] {
                telar::reset_layout_runtime();
                telar::set_theme(NordTheme::new());
                assert!(pill_view(pill, style(vertical), |_| {}).is_ok());
            }
        }
    }

    /// The row runs along the bar, not across it.
    ///
    /// The regression this exists for: the keyed-list constructors that take a gap hardcode a column, so the
    /// pills stacked downwards inside a bottom bar one pill high and the module drew nothing at all. Building
    /// successfully proves none of that — only laying it out does.
    #[test]
    fn the_pills_run_along_the_bar_on_every_edge() {
        use telar::{AvailableSpace, compute_layout, new_container, track_layout};

        let side = 32.0;
        let rows = vec![
            Pill {
                id: 1,
                label: "1".to_string(),
                occupied: false,
                active: true,
                special: false,
                clients: Vec::new(),
                icon: None,
            },
            Pill {
                id: 2,
                label: "2".to_string(),
                occupied: true,
                active: false,
                special: false,
                clients: Vec::new(),
                icon: None,
            },
        ];
        let along = side * 2.0 + PILL_GAP;

        for vertical in [false, true] {
            telar::reset_layout_runtime();
            telar::set_theme(NordTheme::new());
            let items = telar::signal(rows.clone()).read_only();
            let style = PillStyle {
                theme: NordTheme::new(),
                radius: 8.0,
                side,
                vertical,
                occupied_background: true,
                indicator: true,
            };
            let grid = grid(items, style, |_| {}).expect("the grid builds");
            let rect = track_layout(grid.layout_node()).expect("the grid registers its rect");
            // Centred rather than the default stretch, so the grid reports the size of its own content
            // instead of the harness's.
            let root = new_container(
                LayoutStyle::new()
                    .flex_row()
                    .align_items(AlignItems::CENTER)
                    .width(400.0)
                    .height(400.0),
                &[grid.layout_node()],
            )
            .expect("root container");
            compute_layout(
                root,
                AvailableSpace::Definite(400.0),
                AvailableSpace::Definite(400.0),
            )
            .expect("layout");

            let rect = rect.get();
            let (expected_w, expected_h) = if vertical {
                (side, along)
            } else {
                (along, side)
            };
            assert_eq!(
                (rect.width, rect.height),
                (expected_w, expected_h),
                "vertical={vertical}: two pills should measure {expected_w}x{expected_h}, got \
                 {}x{} — a row laid out across the bar instead of along it",
                rect.width,
                rect.height
            );
        }
    }

    /// The indicator's paint closure, the pill's style closure and the tracking effect only run when something
    /// builds them, and each reads a signal — which is the shape that panics on a re-entrant borrow.
    #[test]
    fn the_grid_builds_on_every_edge_with_and_without_the_indicator() {
        let bare = Pill {
            id: 1,
            label: "1".to_string(),
            occupied: false,
            active: true,
            special: false,
            clients: Vec::new(),
            icon: None,
        };
        let rows = vec![
            bare.clone(),
            Pill {
                id: 2,
                active: false,
                occupied: true,
                ..bare.clone()
            },
        ];
        for vertical in [false, true] {
            for indicator in [false, true] {
                telar::reset_layout_runtime();
                telar::set_theme(NordTheme::new());
                let items = telar::signal(rows.clone()).read_only();
                let style = PillStyle {
                    theme: NordTheme::new(),
                    radius: 8.0,
                    side: 32.0,
                    vertical,
                    occupied_background: true,
                    indicator,
                };
                assert!(
                    grid(items, style, |_| {}).is_ok(),
                    "vertical={vertical} indicator={indicator}"
                );
            }
        }
    }

    #[test]
    fn only_the_indicator_moves_the_accent_off_the_pill() {
        let active = Pill {
            id: 1,
            label: "1".to_string(),
            occupied: true,
            active: true,
            special: false,
            clients: Vec::new(),
            icon: None,
        };
        let style = |indicator| PillStyle {
            theme: NordTheme::new(),
            radius: 8.0,
            side: 32.0,
            vertical: false,
            occupied_background: true,
            indicator,
        };
        let theme = NordTheme::new();
        assert_eq!(
            fill_for(&active, style(false)),
            theme.accent,
            "without a sliding indicator the pill paints its own accent, as it always did"
        );
        assert_eq!(
            fill_for(&active, style(true)),
            Color::TRANSPARENT,
            "with one, ANY opaque fill sits on top of the indicator and hides it — the bug this caught was an \
             active pill still painting its occupied background, leaving only the corners showing"
        );
        // The text still has to read against the accent the indicator puts behind it.
        assert_eq!(text_for(&active, style(true)), theme.base);

        // And the neighbours, which is the same bug one pill over: the indicator travels *under* the row, so
        // an occupied pill painting its tint is a box the indicator disappears behind on the way past.
        let occupied = Pill {
            active: false,
            ..active.clone()
        };
        let empty = Pill {
            occupied: false,
            ..occupied.clone()
        };
        assert_eq!(fill_for(&occupied, style(true)), Color::TRANSPARENT);
        assert_eq!(fill_for(&empty, style(true)), Color::TRANSPARENT);
        assert_eq!(
            fill_for(&occupied, style(false)),
            theme.surface,
            "without the indicator the occupied tint is unchanged"
        );
        // Occupancy has to stay legible once the tint is gone, and the label colour is what carries it.
        assert_ne!(
            text_for(&occupied, style(true)),
            text_for(&empty, style(true))
        );
    }

    /// The indicator paints on its own, with no event to force a redraw.
    #[test]
    fn the_trail_stretches_toward_the_goal_and_collapses_on_arrival() {
        let at = |x: f32| Rect {
            x,
            y: 10.0,
            width: 30.0,
            height: 30.0,
        };

        // Arrived: the distance is zero, so the union is the rect itself — one pill wide, as before.
        assert_eq!(with_trail(at(100.0), at(100.0), 0.5), at(100.0));
        // A trail of zero is the old square box the whole way.
        assert_eq!(with_trail(at(0.0), at(100.0), 0.0), at(0.0));

        let ahead = with_trail(at(0.0), at(100.0), 0.5);
        assert_eq!(ahead.x, 0.0, "the trailing edge stays put");
        assert_eq!(
            ahead.width, 80.0,
            "and it reaches half the remaining distance"
        );

        let behind = with_trail(at(100.0), at(0.0), 0.5);
        assert_eq!(behind.x, 50.0);
        assert_eq!(behind.width, 80.0);

        let down = with_trail(
            Rect {
                x: 5.0,
                y: 0.0,
                width: 30.0,
                height: 30.0,
            },
            Rect {
                x: 5.0,
                y: 60.0,
                width: 30.0,
                height: 30.0,
            },
            0.5,
        );
        assert_eq!((down.y, down.height, down.width), (0.0, 60.0, 30.0));
    }

    #[test]
    fn the_trail_is_bounded_and_off_whenever_the_indicator_is() {
        use config::WorkspacesConfig;
        assert_eq!(WorkspacesConfig::default().trail(), 0.35);
        let no_indicator = WorkspacesConfig {
            indicator: false,
            ..WorkspacesConfig::default()
        };
        assert_eq!(
            no_indicator.trail(),
            0.0,
            "there is nothing sliding for a trail to follow"
        );
        let absurd = WorkspacesConfig {
            indicator_trail: 5.0,
            ..WorkspacesConfig::default()
        };
        assert_eq!(absurd.trail(), 0.9, "clamped below the whole distance");
        let broken = WorkspacesConfig {
            indicator_trail: f32::NAN,
            ..WorkspacesConfig::default()
        };
        assert_eq!(broken.trail(), 0.0);
    }

    #[test]
    fn the_indicator_paints_without_a_pointer_event() {
        use telar::{AvailableSpace, ComponentList, DrawCommand, compute_layout, new_container};

        telar::reset_layout_runtime();
        let theme = NordTheme::new();
        telar::set_theme(theme);
        let side = 32.0;
        let rows = vec![
            Pill {
                id: 1,
                label: "1".to_string(),
                occupied: false,
                active: true,
                special: false,
                clients: Vec::new(),
                icon: None,
            },
            Pill {
                id: 2,
                label: "2".to_string(),
                occupied: true,
                active: false,
                special: false,
                clients: Vec::new(),
                icon: None,
            },
        ];
        let items = telar::signal(rows).read_only();
        let style = PillStyle {
            theme,
            radius: 8.0,
            side,
            vertical: false,
            occupied_background: true,
            indicator: true,
        };
        let built = grid(items, style, |_| {}).expect("the grid builds");
        let root_node = new_container(
            LayoutStyle::new()
                .flex_row()
                .align_items(AlignItems::CENTER)
                .width(400.0)
                .height(400.0),
            &[built.layout_node()],
        )
        .expect("root container");
        let root = Container::new(
            LayoutStyle::new()
                .flex_row()
                .align_items(AlignItems::CENTER)
                .width(400.0)
                .height(400.0),
            vec![built],
        )
        .expect("root");
        let tree = ComponentList::new(root);

        let accent = theme.accent;
        let painted = |tree: &ComponentList| {
            tree.commands().iter().any(|cmd| match cmd {
                DrawCommand::Rect { rect, style } => {
                    style.fill == Some(telar::Paint::Solid(accent))
                        && rect.width > 0.0
                        && rect.height > 0.0
                }
                _ => false,
            })
        };

        // The driver's loop: lay out, compose, tick the motion engine, compose again — never an event.
        compute_layout(
            root_node,
            AvailableSpace::Definite(400.0),
            AvailableSpace::Definite(400.0),
        )
        .expect("layout");
        let _ = tree.commands();
        telar::relayout_if_dirty();
        let start = std::time::Instant::now();
        for frame in 1..=30 {
            telar::motion::tick(start + std::time::Duration::from_millis(16 * frame));
            if painted(&tree) {
                return;
            }
        }
        panic!("the indicator never painted: half a second of frames with no accent rect anywhere");
    }

    #[test]
    fn scroll_moves_one_workspace_and_stops_at_the_ends() {
        let snap = snapshot(
            vec![ws(1, 1, "eDP-1"), ws(2, 1, "eDP-1"), ws(3, 1, "eDP-1")],
            2,
        );
        assert_eq!(scroll_target(&snap, true), Some(1));
        assert_eq!(scroll_target(&snap, false), Some(3));

        let first = snapshot(vec![ws(1, 1, "eDP-1"), ws(2, 1, "eDP-1")], 1);
        assert_eq!(
            scroll_target(&first, true),
            None,
            "nowhere to go means no dispatch at all"
        );
        let last = snapshot(vec![ws(1, 1, "eDP-1"), ws(2, 1, "eDP-1")], 2);
        assert_eq!(scroll_target(&last, false), None);
    }

    #[test]
    fn scroll_skips_special_workspaces() {
        let mut special = ws(-99, 1, "eDP-1");
        special.name = "special:magic".to_string();
        let snap = snapshot(vec![ws(1, 1, "eDP-1"), special, ws(2, 1, "eDP-1")], 1);
        assert_eq!(
            scroll_target(&snap, false),
            Some(2),
            "a scratchpad is not somewhere the wheel should land"
        );
    }
}
