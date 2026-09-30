//! The layout that ships with the shell.
//!
//! It is built in code rather than read from a file so that it cannot go missing, cannot fail to parse, and is there to fall back to when the user's own layout does not resolve. It is read-only: the first edit to it forks a copy under a name of the user's own ([`crate::store::LayoutStore::fork`]).
//!
//! It is deliberately the smallest arrangement that is still a usable desktop — one top bar with the workspaces, the clock and notes, and the column notifications arrive in, which is what a fresh install has shown since before layouts existed. Anything more would be a preference the shell had decided on the user's behalf and that every new user would have to undo.
//!
//! The lock layer is the one place that is not minimal, and for the same reason: the clock, the user and what is playing are what this shell's lock screen has shown since before it had a layout, and a fresh install that lost them would be a regression dressed as a default.

use config::{Edge, Shape};

use crate::model::*;

pub fn layout() -> Layout {
    Layout {
        id: LayoutId::new(crate::store::BUILT_IN),
        name: "Default".into(),
        extends: None,
        outputs: vec![OutputRule {
            matches: OutputMatch("*".into()),
            layers: Layers {
                background: Layer {
                    areas: vec![wallpaper()],
                    remove: Vec::new(),
                },
                top: Layer {
                    areas: vec![top_bar()],
                    remove: Vec::new(),
                },
                overlay: Layer {
                    areas: vec![stack()],
                    remove: Vec::new(),
                },
                lock: Layer {
                    areas: vec![lock_readings(), prompt()],
                    remove: Vec::new(),
                },
                ..Layers::default()
            },
            workspaces: Vec::new(),
        }],
    }
}

/// The desktop's picture: the whole output, showing whatever `[background]` is set to.
///
/// `source` is deliberately empty, which is what a region says when it means "the configured wallpaper" rather than one file — so changing the picture stays a `[background]` edit and a `hogar-shell wallpaper set`, not a layout edit.
fn wallpaper() -> Area {
    Area {
        id: AreaId::new("background"),
        kind: Some(AreaKind::WallpaperRegion {
            rect: None,
            source: None,
            fit: None,
            transition: None,
        }),
        ..Area::default()
    }
}

fn top_bar() -> Area {
    Area {
        id: AreaId::new("bar-top"),
        kind: Some(AreaKind::Bar {
            edge: Some(Edge::Top),
            thickness: Some(34.0),
            length: Some(Extent::Fill),
            offset: Some(0.0),
            shape: BarShape {
                mode: Some(Shape::Bar),
                gap: Some(0.0),
                ..BarShape::default()
            },
            autohide: None,
        }),
        reserve: Some(true),
        groups: vec![
            zone("start", Zone::Start, &[("workspaces", "workspaces")]),
            zone("center", Zone::Center, &[("clock", "clock")]),
            zone("end", Zone::End, &[("notes", "notes")]),
        ],
        ..Area::default()
    }
}

/// Where notifications, toasts and OSDs are drawn: a column in the top right corner of what the bars leave, on whichever screen has focus.
fn stack() -> Area {
    Area {
        id: AreaId::new("stack"),
        kind: Some(AreaKind::Stack {
            anchor: Some(Anchor::TopRight),
            width: Some(380.0),
            output_policy: Some(StackOutputPolicy::Focused),
            routes: Vec::new(),
        }),
        within: Some(Within::Usable),
        ..Area::default()
    }
}

fn zone(id: &str, zone: Zone, modules: &[(&str, &str)]) -> Group {
    Group {
        id: GroupId::new(id),
        kind: Some(GroupKind::Zone { zone }),
        children: modules
            .iter()
            .map(|(instance, module)| Instance {
                id: InstanceId::new(*instance),
                module: Some((*module).to_string()),
                representation: Some(Representation::Chip),
                ..Instance::default()
            })
            .collect(),
        ..Group::default()
    }
}

fn prompt() -> Area {
    Area {
        id: AreaId::new("prompt"),
        kind: Some(AreaKind::Prompt { rect: None }),
        ..Area::default()
    }
}

/// What a locked screen shows besides the prompt: the time, who is signed in, what is playing and what is waiting.
///
/// A grid above the prompt rather than a column inside it, because these are placed readings like any other — the same module instances the desktop's widgets are, at the same sizes, arranged by the same resolver. That is the whole point of the lock layer being a layer: what it shows is edited the way everything else is, and the prompt is the one thing in it that cannot be moved away.
///
/// Every one is a reading (`ReadOnly`), which is what the layer allows. The media instance says the track and the artist or only that something is playing, and the notifications one says the count or the count and the applications, according to `[lock] media_detail` and `[lock] notification_detail` — which is where "how much may a stranger read" belongs, rather than in which instances are placed here.
fn lock_readings() -> Area {
    Area {
        id: AreaId::new("lock-readings"),
        kind: Some(AreaKind::Grid {
            rect: Some(Rect {
                x: 0.0,
                y: 0.12,
                w: 1.0,
                h: 0.3,
            }),
            cell: None,
            gap: None,
            anchor: Some(Anchor::Top),
        }),
        groups: vec![
            cell("clock", 0, 0, &[("lock-clock", "clock")]),
            cell("user", 0, 2, &[("lock-user", "user")]),
            cell("media", 4, 0, &[("lock-media", "media")]),
            cell(
                "notifications",
                4,
                2,
                &[("lock-notifications", "notifications")],
            ),
        ],
        ..Area::default()
    }
}

fn cell(id: &str, col: u32, row: u32, modules: &[(&str, &str)]) -> Group {
    Group {
        id: GroupId::new(id),
        kind: Some(GroupKind::Cell {
            col,
            row,
            col_span: 1,
            row_span: 1,
        }),
        children: modules
            .iter()
            .map(|(instance, module)| Instance {
                id: InstanceId::new(*instance),
                module: Some((*module).to_string()),
                representation: Some(Representation::WidgetM),
                ..Instance::default()
            })
            .collect(),
        ..Group::default()
    }
}
