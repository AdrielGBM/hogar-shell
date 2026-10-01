//! The mode pie: which mode each arrow picks.

#[cfg(test)]
mod tests {
    use layout::LayerKind;

    use crate::keys::Direction;
    use crate::pie::{petals, toward};

    /// Four other modes sit at the four arrows, so each arrow reaches exactly one of them and every one is reached.
    #[test]
    fn every_arrow_reaches_its_own_petal() {
        for current in LayerKind::ALL {
            let reached: Vec<LayerKind> = [
                Direction::Up,
                Direction::Right,
                Direction::Down,
                Direction::Left,
            ]
            .into_iter()
            .filter_map(|direction| toward(current, direction))
            .collect();
            let petals: Vec<LayerKind> = petals(current).iter().map(|petal| petal.layer).collect();
            assert_eq!(reached, petals, "from {current}");
        }
    }
}
