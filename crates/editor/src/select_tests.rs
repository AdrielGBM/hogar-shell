//! What a click on the edited layer selects: the smallest thing under the pointer, then what holds it.

#[cfg(test)]
mod tests {
    use telar::Rect;

    use layout::{GroupId, InstanceId};
    use surfaces::rects::Node;

    use crate::rig::bar;
    use crate::session::{self, Selection};

    /// A bar, its centre zone and the clock in it, one inside the other, and the end zone beside them.
    fn placed() -> Vec<(Node, Rect)> {
        let center = GroupId::new("center");
        vec![
            (bar(), Rect::new(0.0, 0.0, 1920.0, 34.0)),
            (bar().group(&center), Rect::new(900.0, 0.0, 120.0, 34.0)),
            (
                bar().instance(&center, &InstanceId::new("clock")),
                Rect::new(920.0, 2.0, 80.0, 30.0),
            ),
            (
                bar().group(&GroupId::new("end")),
                Rect::new(1800.0, 0.0, 120.0, 34.0),
            ),
        ]
    }

    /// The smallest thing under the pointer is what a click selects, and clicking it again climbs to what holds it — instance, group, area — and round again.
    #[test]
    fn a_click_selects_the_smallest_thing_and_the_next_click_the_one_around_it() {
        let placed = placed();
        let on_the_clock = (950.0, 10.0);
        let center = GroupId::new("center");
        let mut selected = Selection::None;
        let mut picked = Vec::new();
        for _ in 0..4 {
            selected = session::pick(&selected, on_the_clock, &placed);
            picked.push(selected.clone());
        }
        assert_eq!(
            picked,
            [
                Selection::Instance(bar().instance(&center, &InstanceId::new("clock"))),
                Selection::Group(bar().group(&center)),
                Selection::Area(bar()),
                Selection::Instance(bar().instance(&center, &InstanceId::new("clock"))),
            ]
        );
    }

    #[test]
    fn a_click_elsewhere_starts_again_from_the_smallest_there_and_a_click_on_nothing_selects_nothing()
     {
        let placed = placed();
        let clock = session::pick(&Selection::None, (950.0, 10.0), &placed);
        assert_eq!(
            session::pick(&clock, (1850.0, 10.0), &placed),
            Selection::Group(bar().group(&GroupId::new("end")))
        );
        assert_eq!(
            session::pick(&clock, (500.0, 500.0), &placed),
            Selection::None
        );
    }
}
