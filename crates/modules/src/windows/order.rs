use std::cell::RefCell;
use std::collections::{BTreeMap, HashSet};

use config::Edge;
use platform_wayland::{ManagedToplevel, ManagedToplevelId};
use ui::host::Size;

thread_local! {
    static HELD: RefCell<BTreeMap<String, Vec<ManagedToplevelId>>> = const { RefCell::new(BTreeMap::new()) };
}

pub fn arrange(
    output: &str,
    windows: &[ManagedToplevel],
    remembered: &[String],
) -> Vec<ManagedToplevel> {
    HELD.with(|held| {
        let mut held = held.borrow_mut();
        let order = held.entry(output.to_string()).or_default();
        let shown = arranged(windows, order, remembered);
        *order = shown.iter().map(|window| window.id).collect();
        shown
    })
}

pub fn held(output: &str, windows: &[ManagedToplevel]) -> Vec<ManagedToplevel> {
    HELD.with(|held| {
        let held = held.borrow();
        let order = held.get(output).map(Vec::as_slice).unwrap_or_default();
        arranged(windows, order, &[])
    })
}

pub fn hold_moved(output: &str, dragged: ManagedToplevelId, onto: ManagedToplevelId) {
    HELD.with(|held| {
        let mut held = held.borrow_mut();
        let order = held.entry(output.to_string()).or_default();
        *order = moved(order, dragged, onto);
    });
}

/// Windows already held keep their places; a new one joins the last window of its own application, else takes the place its application was remembered at, else goes last.
pub fn arranged(
    windows: &[ManagedToplevel],
    held: &[ManagedToplevelId],
    remembered: &[String],
) -> Vec<ManagedToplevel> {
    let mut shown: Vec<ManagedToplevel> = held
        .iter()
        .filter_map(|id| windows.iter().find(|window| window.id == *id))
        .cloned()
        .collect();
    let rank = |app: &str| remembered.iter().position(|kept| kept == app);
    for window in windows {
        if shown.iter().any(|placed| placed.id == window.id) {
            continue;
        }
        let at = match shown
            .iter()
            .rposition(|placed| placed.app_id == window.app_id)
        {
            Some(sibling) => sibling + 1,
            None => match rank(&window.app_id) {
                Some(own) => shown
                    .iter()
                    .position(|placed| rank(&placed.app_id).is_none_or(|other| other > own))
                    .unwrap_or(shown.len()),
                None => shown.len(),
            },
        };
        shown.insert(at, window.clone());
    }
    shown
}

pub fn moved(
    order: &[ManagedToplevelId],
    dragged: ManagedToplevelId,
    onto: ManagedToplevelId,
) -> Vec<ManagedToplevelId> {
    let (Some(from), Some(to)) = (
        order.iter().position(|id| *id == dragged),
        order.iter().position(|id| *id == onto),
    ) else {
        return order.to_vec();
    };
    let mut order = order.to_vec();
    let id = order.remove(from);
    order.insert(to, id);
    order
}

/// The applications in the order `shown` puts them, with every application `before` remembered that is not open now kept after the one it followed there.
pub fn remembered_after(shown: &[ManagedToplevel], before: &[String]) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut apps: Vec<String> = shown
        .iter()
        .filter(|window| seen.insert(window.app_id.as_str()))
        .map(|window| window.app_id.clone())
        .collect();
    for (index, app) in before.iter().enumerate() {
        if apps.contains(app) {
            continue;
        }
        let after = before[..index]
            .iter()
            .rev()
            .find_map(|earlier| apps.iter().position(|kept| kept == earlier));
        apps.insert(after.map_or(0, |at| at + 1), app.clone());
    }
    apps
}

pub fn landing(spans: &[(ManagedToplevelId, f32, f32)], point: f32) -> Option<ManagedToplevelId> {
    let first = spans.first()?;
    let last = spans.last()?;
    if point < first.1 {
        return Some(first.0);
    }
    if point >= last.2 {
        return Some(last.0);
    }
    spans
        .iter()
        .find(|(_, start, end)| point >= *start && point < *end)
        .map(|(id, ..)| *id)
}

const NARROW: f32 = 64.0;
const TALL: f32 = 65.0;
const ROOMY: f32 = 300.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Arrangement {
    pub column: bool,
    pub titles: bool,
}

/// A strip on a bar runs along it; anywhere else it is a list once it is tall, a column of icons once it is narrow, and a row otherwise. Titles need the room for them: never in a narrow box, and in a row only where it is roomy.
pub fn arrangement(extent: Size, axis: Option<Edge>, titles: bool) -> Arrangement {
    let narrow = extent.width <= NARROW;
    let column = match axis {
        Some(edge) => edge.is_vertical(),
        None => narrow || extent.height >= TALL,
    };
    Arrangement {
        column,
        titles: titles && !narrow && (column || extent.width > ROOMY),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(id: u32, app: &str) -> ManagedToplevel {
        ManagedToplevel {
            id: ManagedToplevelId::from_raw(id),
            app_id: app.to_string(),
            title: format!("{app} {id}"),
            ..ManagedToplevel::default()
        }
    }

    fn ids(windows: &[ManagedToplevel]) -> Vec<u32> {
        windows.iter().map(|window| window.id.raw()).collect()
    }

    fn held(raw: &[u32]) -> Vec<ManagedToplevelId> {
        raw.iter()
            .copied()
            .map(ManagedToplevelId::from_raw)
            .collect()
    }

    fn apps(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| name.to_string()).collect()
    }

    #[test]
    fn with_nothing_held_or_remembered_the_compositor_s_order_stands() {
        let open = [window(1, "kitty"), window(2, "firefox"), window(3, "code")];
        assert_eq!(ids(&arranged(&open, &[], &[])), vec![1, 2, 3]);
    }

    #[test]
    fn a_held_order_survives_the_compositor_announcing_another() {
        let open = [window(1, "kitty"), window(2, "firefox"), window(3, "code")];
        assert_eq!(ids(&arranged(&open, &held(&[3, 1, 2]), &[])), vec![3, 1, 2]);
    }

    #[test]
    fn a_closed_window_leaves_the_rest_where_they_were() {
        let open = [window(1, "kitty"), window(3, "code")];
        assert_eq!(ids(&arranged(&open, &held(&[3, 2, 1]), &[])), vec![3, 1]);
    }

    #[test]
    fn a_new_window_joins_the_last_of_its_own_application() {
        let open = [
            window(1, "kitty"),
            window(2, "firefox"),
            window(3, "code"),
            window(4, "kitty"),
        ];
        assert_eq!(
            ids(&arranged(&open, &held(&[1, 2, 3]), &[])),
            vec![1, 4, 2, 3]
        );
    }

    #[test]
    fn a_new_application_takes_its_remembered_place_and_an_unknown_one_goes_last() {
        let remembered = apps(&["code", "firefox", "kitty"]);
        let open = [window(1, "kitty"), window(2, "code")];
        assert_eq!(ids(&arranged(&open, &[], &remembered)), vec![2, 1]);

        let open = [
            window(1, "kitty"),
            window(2, "code"),
            window(3, "firefox"),
            window(4, "gimp"),
        ];
        assert_eq!(
            ids(&arranged(&open, &held(&[2, 1]), &remembered)),
            vec![2, 3, 1, 4],
            "firefox ranks between code and kitty, gimp was never placed"
        );
    }

    #[test]
    fn a_drag_puts_the_window_where_the_one_it_landed_on_stood() {
        let order = held(&[1, 2, 3, 4]);
        let raw = |order: Vec<ManagedToplevelId>| {
            order
                .into_iter()
                .map(ManagedToplevelId::raw)
                .collect::<Vec<_>>()
        };
        let id = ManagedToplevelId::from_raw;
        assert_eq!(raw(moved(&order, id(1), id(3))), vec![2, 3, 1, 4]);
        assert_eq!(raw(moved(&order, id(4), id(1))), vec![4, 1, 2, 3]);
        assert_eq!(raw(moved(&order, id(2), id(2))), vec![1, 2, 3, 4]);
        assert_eq!(
            raw(moved(&order, id(9), id(1))),
            vec![1, 2, 3, 4],
            "a window that is gone moves nothing"
        );
    }

    #[test]
    fn the_held_order_is_kept_per_output() {
        let open = [window(1, "kitty"), window(2, "firefox")];
        arrange("DP-1", &open, &[]);
        arrange("HDMI-A-1", &open, &[]);
        hold_moved(
            "DP-1",
            ManagedToplevelId::from_raw(2),
            ManagedToplevelId::from_raw(1),
        );
        assert_eq!(ids(&arrange("DP-1", &open, &[])), vec![2, 1]);
        assert_eq!(ids(&arrange("HDMI-A-1", &open, &[])), vec![1, 2]);
    }

    #[test]
    fn what_is_remembered_follows_the_strip_and_keeps_closed_applications_beside_their_neighbours()
    {
        let shown = [window(3, "code"), window(1, "kitty"), window(4, "kitty")];
        assert_eq!(
            remembered_after(&shown, &apps(&["kitty", "gimp", "code", "mpv"])),
            apps(&["code", "mpv", "kitty", "gimp"])
        );
        assert_eq!(
            remembered_after(&shown, &apps(&["inkscape"])),
            apps(&["inkscape", "code", "kitty"]),
            "an application that followed nothing open goes first"
        );
    }

    #[test]
    fn a_drag_lands_on_the_span_under_it_or_the_nearest_end() {
        let id = ManagedToplevelId::from_raw;
        let spans = [
            (id(1), 0.0, 40.0),
            (id(2), 44.0, 84.0),
            (id(3), 88.0, 128.0),
        ];
        assert_eq!(landing(&spans, 50.0), Some(id(2)));
        assert_eq!(landing(&spans, -10.0), Some(id(1)));
        assert_eq!(landing(&spans, 400.0), Some(id(3)));
        assert_eq!(
            landing(&spans, 42.0),
            None,
            "the gap between two is neither"
        );
        assert_eq!(landing(&[], 5.0), None);
    }

    fn size(width: f32, height: f32) -> Size {
        Size { width, height }
    }

    #[test]
    fn a_strip_follows_its_bar_and_writes_titles_only_along_a_horizontal_one() {
        for edge in Edge::ALL {
            let along = if edge.is_vertical() {
                size(34.0, f32::INFINITY)
            } else {
                size(f32::INFINITY, 34.0)
            };
            assert_eq!(
                arrangement(along, Some(edge), true),
                Arrangement {
                    column: edge.is_vertical(),
                    titles: !edge.is_vertical(),
                },
                "{edge:?}"
            );
            assert!(!arrangement(along, Some(edge), false).titles, "{edge:?}");
        }
        assert_eq!(
            arrangement(size(200.0, f32::INFINITY), Some(Edge::Left), true),
            Arrangement {
                column: true,
                titles: true,
            },
            "a bar wide enough to write in is written in"
        );
        assert!(
            !arrangement(size(f32::INFINITY, 120.0), Some(Edge::Top), true).column,
            "however thick, a horizontal bar runs one way"
        );
    }

    #[test]
    fn a_box_is_a_list_once_tall_icons_once_narrow_and_a_row_otherwise() {
        assert_eq!(
            arrangement(size(176.0, 176.0), None, true),
            Arrangement {
                column: true,
                titles: true,
            },
            "a tall widget lists its windows"
        );
        assert_eq!(
            arrangement(size(48.0, 300.0), None, true),
            Arrangement {
                column: true,
                titles: false,
            },
            "a narrow box stacks icons"
        );
        assert_eq!(
            arrangement(size(600.0, 40.0), None, true),
            Arrangement {
                column: false,
                titles: true,
            },
        );
        assert_eq!(
            arrangement(size(240.0, 40.0), None, true),
            Arrangement {
                column: false,
                titles: false,
            },
            "a short row has no room for titles"
        );
        assert!(!arrangement(size(176.0, 176.0), None, false).titles);
    }

    #[test]
    fn a_box_changes_its_arrangement_exactly_at_the_sizes_that_decide_it() {
        let shape = |width: f32, height: f32| {
            let Arrangement { column, titles } = arrangement(size(width, height), None, true);
            (column, titles)
        };
        assert_eq!(shape(600.0, TALL), (true, true), "tall from {TALL}");
        assert_eq!(shape(600.0, TALL - 0.5), (false, true), "a row below it");
        assert_eq!(shape(ROOMY, 40.0), (false, false), "{ROOMY} is not roomy");
        assert_eq!(shape(ROOMY + 0.5, 40.0), (false, true));
        assert_eq!(shape(NARROW, 40.0), (true, false), "{NARROW} is narrow");
        assert_eq!(shape(NARROW + 0.5, 40.0), (false, false));
        assert_eq!(shape(NARROW + 0.5, TALL), (true, true));
    }
}
