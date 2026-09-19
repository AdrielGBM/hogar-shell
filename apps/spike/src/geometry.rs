//! Exact pixel arithmetic over the handful of rects a commit or a region carries.
//!
//! Damage and input regions arrive as lists of possibly overlapping rects, and every question the report asks of them — how much of the buffer a frame damaged, whether that damage stayed inside the rects that were expected to change, whether a click target lies inside the declared input region — is an area question about their union. The rect counts are small (a frame damages a few rects, a region holds a few dozen), so the union is computed exactly by compressing coordinates rather than approximated by bounding boxes, which would turn two distant chips into one wide strip and report damage nobody sent.

/// A rect on the integer pixel grid: origin and size, in whatever space the caller is working in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PxRect {
    pub x: i64,
    pub y: i64,
    pub w: i64,
    pub h: i64,
}

impl PxRect {
    pub const fn new(x: i64, y: i64, w: i64, h: i64) -> Self {
        Self { x, y, w, h }
    }

    /// The smallest pixel rect holding a fractional one scaled by `scale` — rounded outward, so a box laid out on a half pixel owns both pixels it touches.
    pub fn enclosing(x: f64, y: f64, w: f64, h: f64, scale: f64) -> Self {
        let left = (x * scale).floor() as i64;
        let top = (y * scale).floor() as i64;
        let right = ((x + w) * scale).ceil() as i64;
        let bottom = ((y + h) * scale).ceil() as i64;
        Self::new(left, top, right - left, bottom - top)
    }

    pub fn right(&self) -> i64 {
        self.x + self.w
    }

    pub fn bottom(&self) -> i64 {
        self.y + self.h
    }

    pub fn is_empty(&self) -> bool {
        self.w <= 0 || self.h <= 0
    }

    pub fn area(&self) -> i64 {
        if self.is_empty() { 0 } else { self.w * self.h }
    }

    pub fn intersect(&self, other: PxRect) -> Option<PxRect> {
        let left = self.x.max(other.x);
        let top = self.y.max(other.y);
        let right = self.right().min(other.right());
        let bottom = self.bottom().min(other.bottom());
        (right > left && bottom > top).then(|| PxRect::new(left, top, right - left, bottom - top))
    }

    pub fn inflate(&self, by: i64) -> PxRect {
        PxRect::new(self.x - by, self.y - by, self.w + 2 * by, self.h + 2 * by)
    }

    /// The smallest rect holding both.
    pub fn hull(&self, other: PxRect) -> PxRect {
        let left = self.x.min(other.x);
        let top = self.y.min(other.y);
        let right = self.right().max(other.right());
        let bottom = self.bottom().max(other.bottom());
        PxRect::new(left, top, right - left, bottom - top)
    }

    fn contains_point(&self, x: f64, y: f64) -> bool {
        x >= self.x as f64
            && x < self.right() as f64
            && y >= self.y as f64
            && y < self.bottom() as f64
    }
}

/// A set of pixels built by adding and subtracting rects in order — the shape of a `wl_region`, and of a commit's damage, which only ever adds.
///
/// Order matters only for subtraction: a rect subtracted and then added back is in the set, which is what `wl_region` means by applying requests in sequence.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PxRegion {
    ops: Vec<(Op, PxRect)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Op {
    Add,
    Subtract,
}

impl PxRegion {
    pub fn from_rects(rects: impl IntoIterator<Item = PxRect>) -> Self {
        let mut region = Self::default();
        for rect in rects {
            region.add(rect);
        }
        region
    }

    pub fn add(&mut self, rect: PxRect) {
        if !rect.is_empty() {
            self.ops.push((Op::Add, rect));
        }
    }

    pub fn subtract(&mut self, rect: PxRect) {
        if !rect.is_empty() {
            self.ops.push((Op::Subtract, rect));
        }
    }

    /// How many rects were added — what a reader of the protocol would call the region's size, whatever they cover.
    pub fn rect_count(&self) -> usize {
        self.ops.iter().filter(|(op, _)| *op == Op::Add).count()
    }

    pub fn extend(&mut self, other: &PxRegion) {
        self.ops.extend(other.ops.iter().copied());
    }

    fn contains_point(&self, x: f64, y: f64) -> bool {
        let mut inside = false;
        for (op, rect) in &self.ops {
            if rect.contains_point(x, y) {
                inside = *op == Op::Add;
            }
        }
        inside
    }

    pub fn area(&self) -> i64 {
        self.measure(None, |mine, _| mine, &[])
    }

    /// The area of this region that falls inside `clip`.
    pub fn area_within(&self, clip: PxRect) -> i64 {
        self.measure(Some(clip), |mine, _| mine, &[])
    }

    /// The area of this region that `allowed` does not cover.
    pub fn area_outside(&self, allowed: &PxRegion) -> i64 {
        self.measure(
            None,
            |mine, theirs| mine && !theirs,
            std::slice::from_ref(allowed),
        )
    }

    /// The area of `other` that this region covers.
    pub fn area_covering(&self, other: &PxRegion) -> i64 {
        self.measure(
            None,
            |mine, theirs| mine && theirs,
            std::slice::from_ref(other),
        )
    }

    pub fn covers(&self, rect: PxRect) -> bool {
        self.area_within(rect) == rect.area()
    }

    pub fn touches(&self, rect: PxRect) -> bool {
        self.area_within(rect) > 0
    }

    /// Sums the cells of the grid every rect edge cuts the plane into, keeping those `keep` accepts given whether the cell is in this region and whether it is in `other`. Each cell is wholly in or out of every rect, so testing its centre is exact.
    fn measure(
        &self,
        clip: Option<PxRect>,
        keep: impl Fn(bool, bool) -> bool,
        other: &[PxRegion],
    ) -> i64 {
        let mut xs: Vec<i64> = Vec::new();
        let mut ys: Vec<i64> = Vec::new();
        let rects = self
            .ops
            .iter()
            .chain(other.iter().flat_map(|region| region.ops.iter()))
            .map(|(_, r)| *r)
            .chain(clip);
        for rect in rects {
            xs.extend([rect.x, rect.right()]);
            ys.extend([rect.y, rect.bottom()]);
        }
        xs.sort_unstable();
        xs.dedup();
        ys.sort_unstable();
        ys.dedup();
        let mut total = 0;
        for xw in xs.windows(2) {
            for yw in ys.windows(2) {
                let (cx, cy) = ((xw[0] + xw[1]) as f64 / 2.0, (yw[0] + yw[1]) as f64 / 2.0);
                if clip.is_some_and(|c| !c.contains_point(cx, cy)) {
                    continue;
                }
                let theirs = other.iter().any(|region| region.contains_point(cx, cy));
                if keep(self.contains_point(cx, cy), theirs) {
                    total += (xw[1] - xw[0]) * (yw[1] - yw[0]);
                }
            }
        }
        total
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlapping_rects_count_once() {
        let region = PxRegion::from_rects([PxRect::new(0, 0, 10, 10), PxRect::new(5, 5, 10, 10)]);
        assert_eq!(
            region.area(),
            175,
            "two 100 px squares sharing a 25 px corner cover 175 px"
        );
    }

    #[test]
    fn distant_rects_are_not_merged_into_their_bounding_box() {
        let region =
            PxRegion::from_rects([PxRect::new(0, 0, 10, 10), PxRect::new(1000, 0, 10, 10)]);
        assert_eq!(region.area(), 200);
        assert!(
            !region.touches(PxRect::new(500, 0, 10, 10)),
            "the gap between them is not damage"
        );
    }

    #[test]
    fn a_subtraction_applies_in_order() {
        let mut region = PxRegion::default();
        region.add(PxRect::new(0, 0, 10, 10));
        region.subtract(PxRect::new(0, 0, 5, 10));
        assert_eq!(region.area(), 50);
        region.add(PxRect::new(0, 0, 2, 10));
        assert_eq!(
            region.area(),
            70,
            "adding back after a subtraction restores those pixels"
        );
    }

    #[test]
    fn area_within_clips_to_the_given_rect() {
        let region = PxRegion::from_rects([PxRect::new(0, 0, 100, 100)]);
        assert_eq!(region.area_within(PxRect::new(90, 90, 20, 20)), 100);
        assert!(region.covers(PxRect::new(10, 10, 20, 20)));
        assert!(!region.covers(PxRect::new(90, 90, 20, 20)));
        assert!(
            !region.touches(PxRect::new(100, 0, 5, 5)),
            "an edge shared with the region is not inside it"
        );
    }

    #[test]
    fn area_outside_is_what_the_allowed_region_misses() {
        let damage = PxRegion::from_rects([PxRect::new(0, 0, 20, 10)]);
        let allowed = PxRegion::from_rects([PxRect::new(0, 0, 10, 10)]);
        assert_eq!(damage.area_outside(&allowed), 100);
        assert_eq!(damage.area_covering(&allowed), 100);
        assert_eq!(allowed.area_outside(&damage), 0);
    }

    #[test]
    fn a_fractional_box_scales_outward() {
        assert_eq!(
            PxRect::enclosing(10.5, 0.25, 20.0, 10.0, 1.0),
            PxRect::new(10, 0, 21, 11)
        );
        assert_eq!(
            PxRect::enclosing(10.0, 5.0, 20.0, 10.0, 2.0),
            PxRect::new(20, 10, 40, 20)
        );
    }
}
