//! What the surfaces' tests build an arrangement from: an instance, a group and an area with nothing set but what a test names, a bar running its whole edge, the desktop's grid, and modules that draw a plain dot.
#![cfg(test)]

use std::collections::BTreeMap;

use config::Edge;
use layout::{
    Anchor, AreaId, Arrange, BarShape, Extent, GroupId, GroupKind, InstanceId, Representation,
    ResolvedArea, ResolvedAreaKind, ResolvedGroup, ResolvedInstance, Style, Within,
};
use ui::descriptor::{
    Built, Category, ChipDef, Input, ModuleDescriptor, Representations, WidgetDef,
};
use ui::host::{Host, WidgetSize};

/// The screen the tests draw on.
pub(crate) const SCREEN: &str = "DP-1";

pub(crate) fn instance(id: &str, module: &str, representation: Representation) -> ResolvedInstance {
    ResolvedInstance {
        id: InstanceId::new(id),
        module: module.to_string(),
        representation,
        options: toml::Table::new(),
        bindings: BTreeMap::new(),
        style: Style::default(),
        placement: None,
        actions: BTreeMap::new(),
    }
}

pub(crate) fn group(id: &str, kind: GroupKind, children: Vec<ResolvedInstance>) -> ResolvedGroup {
    ResolvedGroup {
        id: GroupId::new(id),
        kind,
        arrange: None,
        cols: Arrange::TRACKS,
        rows: Arrange::TRACKS,
        gap: None,
        repeat: None,
        komponent: None,
        style: Style::default(),
        children,
    }
}

/// An area that reserves nothing, over the whole output.
pub(crate) fn area(id: &str, kind: ResolvedAreaKind, groups: Vec<ResolvedGroup>) -> ResolvedArea {
    ResolvedArea {
        id: AreaId::new(id),
        kind,
        reserve: false,
        above_fullscreen: false,
        within: Within::Output,
        style: Style::default(),
        visible: None,
        actions: BTreeMap::new(),
        groups,
    }
}

/// The one cell at `col` and `row`.
pub(crate) fn cell(col: u32, row: u32) -> GroupKind {
    GroupKind::Cell {
        col,
        row,
        col_span: 1,
        row_span: 1,
    }
}

/// A bar 34 pixels thick along all of `edge`, shaped as the theme says.
pub(crate) fn bar_kind(edge: Edge) -> ResolvedAreaKind {
    ResolvedAreaKind::Bar {
        edge,
        thickness: 34.0,
        length: Extent::Fill,
        offset: 0.0,
        shape: BarShape::default(),
        autohide: None,
    }
}

/// A grid of 80 pixel cells 16 apart, laid from the top left of the whole output.
pub(crate) fn grid_kind() -> ResolvedAreaKind {
    ResolvedAreaKind::Grid {
        rect: layout::Rect::default(),
        cell: 80.0,
        gap: 16.0,
        anchor: Anchor::TopLeft,
    }
}

pub(crate) fn dot(_: &Host) -> Built {
    Ok(Box::new(telar::Container::new(
        telar::LayoutStyle::new().width(20.0).height(20.0),
        Vec::new(),
    )?))
}

const fn dot_module(id: &'static str) -> ModuleDescriptor {
    ModuleDescriptor {
        id,
        name: id,
        icon: "circle",
        category: Category::Info,
        options: &[],
        representations: Representations {
            chip: Some(ChipDef::new(dot, Input::ReadOnly)),
            widget: Some(WidgetDef {
                sizes: &WidgetSize::ALL,
                build: dot,
                input: Input::ReadOnly,
            }),
            ..Representations::NONE
        },
        actions: &[],
        sources: &[],
    }
}

/// Two modules, `dot-a` and `dot-b`, each a [`dot`] as a chip and as a widget of every size.
pub(crate) const DOTS: &[ModuleDescriptor] = &[dot_module("dot-a"), dot_module("dot-b")];
