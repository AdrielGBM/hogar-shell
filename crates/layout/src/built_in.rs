//! The layout that ships with the shell.
//!
//! It is built in code rather than read from a file so that it cannot go missing, cannot fail to parse, and is there to fall back to when the user's own layout does not resolve. It is read-only: the first edit to it forks a copy under a name of the user's own ([`crate::store::LayoutStore::fork`]).
//!
//! It is deliberately the smallest arrangement that is still a usable desktop — one top bar with the workspaces, the clock and notes, which is what a fresh install has shown since before layouts existed — plus the prompt the lock layer must have. Anything more would be a preference the shell had decided on the user's behalf and that every new user would have to undo.

use config::Edge;

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
                lock: Layer {
                    areas: vec![prompt()],
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
            shape: BarShape::default(),
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
        remove: Vec::new(),
    }
}

fn prompt() -> Area {
    Area {
        id: AreaId::new("prompt"),
        kind: Some(AreaKind::Prompt {
            rect: None,
            style: PromptStyle::default(),
        }),
        ..Area::default()
    }
}
