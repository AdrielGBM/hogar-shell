//! A group's popover, written through a draft of the area that holds it ([`GroupDraft`]) so it previews, reverts and records as one entry as an area's does.

use std::rc::Rc;

use telar::{LayoutError, LayoutStyle, ReactiveList, RwSignal, SizeDimension};

use layout::{Arrange, GroupKind, ResolvedGroup, Unset};
use ui::descriptor::Built;

use super::area::{self, help, parsed, spelled};
use super::draft::GroupDraft;
use super::handles;
use super::rows::{self, Range, Rows, label};
use super::{Inspector, look};

const INNER_TRACKS: f32 = 12.0;
const WIDEST_SPAN: f32 = 12.0;

pub(crate) fn inspector(draft: &GroupDraft) -> Result<Inspector, LayoutError> {
    let mut list = area::variant_rows(&draft.area.node)?;
    if draft.resolved.komponent.is_none() {
        list.extend(arrangement(draft)?);
    }
    list.extend(area::repeat_rows(&draft.area, Some(&draft.id))?);
    list.extend(area::parameter_rows(&draft.area, Some(&draft.id))?);
    list.push(rows::heading(|| telar::t!("editor.look.heading"))?);
    list.push(look::fill(draft)?);
    list.push(look::radius(draft)?);
    list.push(look::opacity(draft)?);
    list.push(padding_row(draft)?);
    list.extend(look::edges(draft)?);
    list.extend(span_rows(draft)?);
    list.push(super::remove_button(draft.area.node.clone())?);
    Ok(Inspector {
        rows: list,
        handles: Vec::new(),
    })
}

/// A group in one of an area's runs is a loose run or one at a time: the area lays the run out itself.
fn arrangement(draft: &GroupDraft) -> Rows {
    let in_a_run = matches!(draft.resolved.kind, GroupKind::Zone { .. });
    let inherits = draft.resolved.arrange.is_some()
        && !draft
            .group()
            .is_some_and(|group| group.writes_arrangement());
    let arrange = draft.setting(
        "arrange",
        "arrange",
        |group| {
            group
                .arrange
                .map(|arrange| spelled(&arrange))
                .unwrap_or_default()
        },
        move |group, picked: &String| match parsed::<Arrange>(picked) {
            Some(arrange) => {
                group.unset.retain(|taken| *taken != Unset::Arrange);
                group.arrange = Some(arrange);
            }
            None => {
                group.clear_arrangement();
                if inherits && !group.unset.contains(&Unset::Arrange) {
                    group.unset.push(Unset::Arrange);
                }
            }
        },
    );
    let mut offered = vec![
        (String::new(), telar::t!("editor.group.loose")),
        (spelled(&Arrange::Pages), telar::t!("editor.group.pages")),
    ];
    if !in_a_run {
        offered.extend([
            (spelled(&Arrange::Column), telar::t!("editor.group.column")),
            (spelled(&Arrange::Row), telar::t!("editor.group.row")),
            (spelled(&Arrange::Grid), telar::t!("editor.group.grid")),
            (spelled(&Arrange::Free), telar::t!("editor.group.free")),
        ]);
    }
    let mut list = vec![draft.marked(
        &["arrange"],
        rows::listed(
            label!("editor.group.arrange"),
            help("Group", "arrange"),
            arrange,
            Rc::from(offered),
        )?,
    )?];
    list.push(arranged_rows(draft, arrange)?);
    Ok(list)
}

fn arranged_rows(draft: &GroupDraft, arrange: RwSignal<String>) -> Built {
    let building = draft.clone();
    let in_bar = draft.area.kind() == "bar";
    Ok(Box::new(ReactiveList::with_style(
        LayoutStyle::new()
            .flex_column()
            .gap(ui::scale::space::sm())
            .width(SizeDimension::Percent(1.0)),
        move || vec![arrange.with(|now| parsed::<Arrange>(now))],
        |arrange: &Option<Arrange>| arrange.map(Arrange::as_str),
        move |arrange: Option<Arrange>| {
            let mut list = Vec::new();
            if arrange == Some(Arrange::Grid) {
                list.push(tracks(
                    &building,
                    "cols",
                    label!("editor.group.inner_cols"),
                )?);
                list.push(tracks(
                    &building,
                    "rows",
                    label!("editor.group.inner_rows"),
                )?);
            }
            if surfaces::container::arranges(&arranged_as(&building, arrange)) {
                let gap = building.setting(
                    "gap",
                    "gap",
                    move |group| surfaces::container::gap_of(group, in_bar),
                    |group, gap: &f32| group.gap = Some(*gap),
                );
                list.push(building.marked(
                    &["gap"],
                    rows::number(
                        label!("editor.area.gap"),
                        help("Group", "gap"),
                        gap,
                        handles::GAP,
                    )?,
                )?);
            }
            rows::together(list)
        },
    )?))
}

fn tracks(draft: &GroupDraft, key: &'static str, label: telar::Reactive<String>) -> Built {
    let columns = key == "cols";
    let count = draft.setting(
        key,
        key,
        move |group| {
            (match columns {
                true => group.cols,
                false => group.rows,
            }) as f32
        },
        move |group, count: &f32| {
            let count = Some(count.round().max(1.0) as u32);
            match columns {
                true => group.cols = count,
                false => group.rows = count,
            }
        },
    );
    draft.marked(
        &[key],
        rows::number(
            label,
            help("Group", key),
            count,
            Range::whole(1.0, INNER_TRACKS),
        )?,
    )
}

fn span_rows(draft: &GroupDraft) -> Rows {
    let GroupKind::Cell { .. } = draft.resolved.kind else {
        return Ok(Vec::new());
    };
    let area_node = surfaces::rects::Node::area(
        draft.area.node.output.as_deref(),
        draft.area.node.layer,
        &draft.area.node.area,
    );
    let room = surfaces::rects::rect(&area_node)
        .and_then(|rect| surfaces::area::lattice(&draft.area.resolved, rect));
    let (most_cols, most_rows) = room.map_or((WIDEST_SPAN, WIDEST_SPAN), |room| {
        (f32::from(room.columns), f32::from(room.rows))
    });
    let placed = draft.resolved.kind;
    let span = |name: &'static str, across: bool| {
        draft.setting(
            name,
            "place",
            move |group| match group.kind {
                GroupKind::Cell {
                    col_span, row_span, ..
                } => {
                    (match across {
                        true => col_span,
                        false => row_span,
                    }) as f32
                }
                GroupKind::Zone { .. } => 1.0,
            },
            move |group, cells: &f32| {
                let cells = cells.round().max(1.0) as u32;
                if let GroupKind::Cell {
                    col_span, row_span, ..
                } = group.kind.get_or_insert(placed)
                {
                    match across {
                        true => *col_span = cells,
                        false => *row_span = cells,
                    }
                }
            },
        )
    };
    let cols = span("col_span", true);
    let rows_spanned = span("row_span", false);
    Ok(vec![
        rows::heading(|| telar::t!("editor.group.span"))?,
        draft.marked(
            &["place"],
            rows::together(vec![
                rows::number(
                    label!("editor.group.span_cols"),
                    help("GroupKind::Cell", "col_span"),
                    cols,
                    Range::whole(1.0, most_cols),
                )?,
                rows::number(
                    label!("editor.group.span_rows"),
                    help("GroupKind::Cell", "row_span"),
                    rows_spanned,
                    Range::whole(1.0, most_rows),
                )?,
            ])?,
        )?,
    ])
}

/// The group as drawn when the popover opened, arranged as `arrange` instead: what the rows that only a container has are decided on while the arrangement row moves.
fn arranged_as(draft: &GroupDraft, arrange: Option<Arrange>) -> ResolvedGroup {
    ResolvedGroup {
        arrange,
        ..(*draft.resolved).clone()
    }
}

fn padding_row(draft: &GroupDraft) -> Built {
    let arrange = draft.area.shared::<String>("arrange");
    let (reading, building) = (draft.clone(), draft.clone());
    Ok(Box::new(ReactiveList::with_style(
        LayoutStyle::new()
            .flex_column()
            .width(SizeDimension::Percent(1.0)),
        move || {
            let arrange = match arrange {
                Some(arrange) => arrange.with(|now| parsed::<Arrange>(now)),
                None => reading.resolved.arrange,
            };
            let padded = crate::tools::target::group_has_padding(&arranged_as(&reading, arrange));
            padded.then_some(()).into_iter().collect::<Vec<_>>()
        },
        |_: &()| (),
        move |()| look::padding(&building),
    )?))
}
