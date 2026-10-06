use telar::{MenuEntry, Rect};

use config::Edge;
use layout::{AreaId, LayerKind, ResolvedAreaKind};
use surfaces::menu::ShellAsked;
use surfaces::reconcile::{self, Desktop};
use surfaces::rects::{self, Node};

use crate::mode::{self, said};
use crate::session::{self, EditError, Selection};

#[derive(Clone)]
struct Target {
    output: Option<String>,
    layer: LayerKind,
    grid: Option<AreaId>,
}

pub(crate) fn install() {
    surfaces::menu::install_shell(|asked| {
        if let Err(why) = open(asked) {
            tracing::info!("no shell menu: {why}");
        }
    });
}

/// Opens the shell's own menu on the screen `asked` names: in an edit mode on that screen it acts on the layer being edited, outside one on the desktop.
pub fn open(asked: ShellAsked) -> Result<(), EditError> {
    let desktop =
        reconcile::desktop_now(asked.output.as_deref()).ok_or_else(EditError::no_output)?;
    let editing = mode::current()
        .filter(|mode| desktop.output.as_deref() == Some(mode.output.as_str()))
        .map(|mode| mode.layer);
    if editing == Some(LayerKind::Lock) {
        return Err(EditError::refused(util::message!("editor.menu.lock")));
    }
    let at = asked
        .at
        .unwrap_or((desktop.size.0 / 2.0, desktop.size.1 / 2.0));
    let layer = editing.unwrap_or(LayerKind::Desktop);
    let target = Target {
        output: desktop.output.clone(),
        layer,
        grid: grid_at(&desktop, layer, at),
    };
    let entries = entries(&target, editing);
    crate::context::show(
        &desktop,
        (LayerKind::Overlay, Edge::Top),
        Rect::new(at.0, at.1, 0.0, 0.0),
        at,
        entries,
    );
    Ok(())
}

fn entries(target: &Target, editing: Option<LayerKind>) -> Vec<MenuEntry> {
    let mut rows = Vec::new();
    let add = add_rows(target, editing.is_some());
    if !add.is_empty() {
        rows.push(MenuEntry::Sub {
            label: telar::t!("editor.shell.add"),
            entries: add,
        });
    }
    if target.grid.is_some() {
        rows.push(MenuEntry::row(
            telar::t!(
                "editor.menu.customize",
                name = telar::t!("editor.menu.kind.grid")
            ),
            "",
            into(target, customize_grid),
        ));
    }
    if editing.is_none() {
        rows.extend(
            crate::host::strip_actions()
                .into_iter()
                .map(|(label, act)| MenuEntry::row(label(), "", entered(target, act))),
        );
    }
    rows.push(MenuEntry::Separator);
    rows.push(edit_rows(target, editing));
    match editing {
        Some(_) => {
            rows.extend(crate::context::edit_mode_rows());
            rows.push(MenuEntry::row(telar::t!("editor.keys.title"), "", || {
                crate::keys::help().set(true)
            }));
            rows.push(MenuEntry::row(telar::t!("editor.done"), "", || {
                mode::leave();
            }));
        }
        None => {
            rows.push(MenuEntry::Separator);
            rows.push(lock_row());
        }
    }
    rows
}

/// In a mode, the layer's add tools follow; one whose label a row before it already has does the same thing, so it is left out.
fn add_rows(target: &Target, editing: bool) -> Vec<MenuEntry> {
    let mut rows = Vec::new();
    if target.grid.is_some() {
        rows.push(MenuEntry::row(
            telar::t!("editor.desktop.add_widget"),
            "",
            into(target, |_| crate::modes::palette::open()),
        ));
        rows.push(MenuEntry::row(
            telar::t!("editor.container.new"),
            "",
            into(target, |_| crate::modes::container::create()),
        ));
    }
    if target.layer == LayerKind::Top {
        rows.push(MenuEntry::row(
            telar::t!("editor.top.plate.new"),
            "",
            into(target, |_| crate::modes::top::plate()),
        ));
    }
    if editing {
        for (label, act) in crate::host::add_tools_of(target.layer) {
            let label = label();
            if !rows.iter().any(|row| says(row, &label)) {
                rows.push(MenuEntry::row(label, "", act));
            }
        }
    }
    rows
}

fn says(row: &MenuEntry, label: &str) -> bool {
    matches!(row, MenuEntry::Row { label: said, .. } if said == label)
}

fn edit_rows(target: &Target, editing: Option<LayerKind>) -> MenuEntry {
    let entries = LayerKind::ALL
        .into_iter()
        .filter(|layer| Some(*layer) != editing)
        .map(|layer| {
            let output = target.output.clone();
            MenuEntry::row(mode::name_of(layer), "", move || {
                if let Err(why) = mode::enter(layer, output.as_deref()) {
                    mode::refuse(why.render());
                }
            })
        })
        .collect();
    let label = match editing {
        Some(_) => telar::t!("editor.shell.edit_other"),
        None => telar::t!("editor.shell.edit"),
    };
    MenuEntry::Sub { label, entries }
}

fn lock_row() -> MenuEntry {
    let row = MenuEntry::row(telar::t!("editor.shell.lock"), "", services::lock::lock);
    match services::lock::can_lock() {
        Ok(()) => row,
        Err(_) => row.disabled(),
    }
}

fn customize_grid(target: &Target) -> Result<(), EditError> {
    let grid = target.grid.as_ref().ok_or_else(EditError::nothing)?;
    crate::popover::open_area(Node::area(target.output.as_deref(), target.layer, grid))
}

fn into(target: &Target, then: fn(&Target) -> Result<(), EditError>) -> impl Fn() + 'static {
    let target = target.clone();
    move || {
        said(enter(&target).and_then(|()| {
            if let Some(grid) = &target.grid {
                session::select(Selection::Area(Node::area(
                    target.output.as_deref(),
                    target.layer,
                    grid,
                )));
            }
            then(&target)
        }))
    }
}

fn entered(target: &Target, act: fn()) -> impl Fn() + 'static {
    let target = target.clone();
    move || said(enter(&target).map(|()| act()))
}

fn enter(target: &Target) -> Result<(), EditError> {
    mode::enter(target.layer, target.output.as_deref())
        .map(drop)
        .map_err(EditError::refused)
}

fn grid_at(desktop: &Desktop, layer: LayerKind, at: (f32, f32)) -> Option<AreaId> {
    let grids: Vec<&AreaId> = desktop
        .resolved
        .layer(layer)?
        .areas
        .iter()
        .filter(|area| matches!(area.kind, ResolvedAreaKind::Grid { .. }))
        .map(|area| &area.id)
        .collect();
    grids
        .iter()
        .find(|id| {
            rects::rect(&Node::area(desktop.output.as_deref(), layer, id))
                .is_some_and(|rect| rect.contains(at.0, at.1))
        })
        .or(grids.first())
        .map(|id| (*id).clone())
}
