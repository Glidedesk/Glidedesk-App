//! Outer monitor edges and edge runs.
//!
//! An *outer segment* is the part of one monitor's edge that does not touch
//! another monitor of the same machine — only those can hand the cursor over
//! to a neighbour. An *edge run* is the ordered list of outer
//! segments that take part in one link.

use glidedesk_proto::{MonitorInfo, Point, Rect, Side};

use crate::model::{HandoverMode, MonitorSelection};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Segment {
    /// Index into the machine's monitor list.
    pub monitor: usize,
    pub side: Side,
    /// Coordinate of the edge pixel line (x for left/right, y for top/bottom).
    pub fixed: i32,
    /// Start along the edge (inclusive).
    pub start: i32,
    /// End along the edge (exclusive).
    pub end: i32,
}

impl Segment {
    #[must_use]
    pub const fn len(&self) -> i32 {
        self.end - self.start
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.end <= self.start
    }

    /// Coordinate of `p` along this edge.
    #[must_use]
    pub const fn along(&self, p: Point) -> i32 {
        if self.side.is_vertical_edge() { p.y } else { p.x }
    }

    #[must_use]
    pub const fn across(&self, p: Point) -> i32 {
        if self.side.is_vertical_edge() { p.x } else { p.y }
    }

    #[must_use]
    pub const fn contains(&self, p: Point) -> bool {
        let a = self.along(p);
        self.across(p) == self.fixed && a >= self.start && a < self.end
    }

    /// Point on the edge at coordinate `a` (clamped into the segment).
    #[must_use]
    pub fn point_at(&self, a: i32) -> Point {
        let a = a.clamp(self.start, self.end - 1);
        if self.side.is_vertical_edge() { Point::new(self.fixed, a) } else { Point::new(a, self.fixed) }
    }
}

fn edge_line(r: &Rect, side: Side) -> (i32, i32, i32) {
    // (fixed pixel line, start, end)
    match side {
        Side::Left => (r.left(), r.top(), r.bottom()),
        Side::Right => (r.right() - 1, r.top(), r.bottom()),
        Side::Top => (r.top(), r.left(), r.right()),
        Side::Bottom => (r.bottom() - 1, r.left(), r.right()),
    }
}

/// Does `o` sit directly across `m`'s `side` edge? Returns the overlapping interval.
fn blocker(m: &Rect, o: &Rect, side: Side) -> Option<(i32, i32)> {
    let touches = match side {
        Side::Right => o.left() == m.right(),
        Side::Left => o.right() == m.left(),
        Side::Bottom => o.top() == m.bottom(),
        Side::Top => o.bottom() == m.top(),
    };
    if !touches {
        return None;
    }
    let (s1, e1, s2, e2) = if side.is_vertical_edge() {
        (m.top(), m.bottom(), o.top(), o.bottom())
    } else {
        (m.left(), m.right(), o.left(), o.right())
    };
    let (s, e) = (s1.max(s2), e1.min(e2));
    (s < e).then_some((s, e))
}

/// Outer segments of every monitor on `side`, in monitor order.
#[must_use]
pub fn outer_segments(monitors: &[Rect], side: Side) -> Vec<Segment> {
    let mut out = Vec::new();
    for (i, m) in monitors.iter().enumerate() {
        if !m.is_valid() {
            continue;
        }
        let (fixed, start, end) = edge_line(m, side);
        let mut blocked: Vec<(i32, i32)> = monitors
            .iter()
            .enumerate()
            .filter(|(j, o)| *j != i && o.is_valid())
            .filter_map(|(_, o)| blocker(m, o, side))
            .collect();
        blocked.sort_unstable();
        let mut cursor = start;
        for (bs, be) in blocked {
            if bs > cursor {
                out.push(Segment { monitor: i, side, fixed, start: cursor, end: bs });
            }
            cursor = cursor.max(be);
        }
        if cursor < end {
            out.push(Segment { monitor: i, side, fixed, start: cursor, end });
        }
    }
    out
}

/// Result of applying a [`MonitorSelection`] to the connected monitors.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Selected {
    pub mask: Vec<bool>,
    /// The selection named monitors that are all missing, so it fell back to "all".
    pub fell_back: bool,
}

#[must_use]
pub fn select(monitors: &[MonitorInfo], sel: &MonitorSelection) -> Selected {
    let all = || Selected { mask: vec![true; monitors.len()], fell_back: false };
    let position = |id| monitors.iter().position(|m| &m.id == id);
    match sel.mode {
        HandoverMode::All => all(),
        HandoverMode::Single => match sel.monitors.iter().find_map(position) {
            Some(i) => {
                let mut mask = vec![false; monitors.len()];
                mask[i] = true;
                Selected { mask, fell_back: false }
            }
            None => Selected { fell_back: true, ..all() },
        },
        HandoverMode::Selected => {
            let mut mask = vec![false; monitors.len()];
            let mut any = false;
            for i in sel.monitors.iter().filter_map(position) {
                mask[i] = true;
                any = true;
            }
            if any { Selected { mask, fell_back: false } } else { Selected { fell_back: true, ..all() } }
        }
    }
}

/// Ordered outer segments that take part in one link, with a length index for
/// continuous mapping.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EdgeRun {
    pub segments: Vec<Segment>,
    prefix: Vec<i64>,
    total: i64,
}

impl EdgeRun {
    #[must_use]
    pub fn new(mut segments: Vec<Segment>) -> Self {
        segments.retain(|s| !s.is_empty());
        segments.sort_by_key(|s| (s.start, s.fixed));
        let mut prefix = Vec::with_capacity(segments.len());
        let mut total = 0i64;
        for s in &segments {
            prefix.push(total);
            total += i64::from(s.len());
        }
        Self { segments, prefix, total }
    }

    /// Run for `side` of a machine limited to the monitors in `mask`.
    #[must_use]
    pub fn build(monitors: &[MonitorInfo], side: Side, mask: &[bool]) -> Self {
        let rects: Vec<Rect> = monitors.iter().map(|m| m.bounds).collect();
        let segs = outer_segments(&rects, side)
            .into_iter()
            .filter(|s| mask.get(s.monitor).copied().unwrap_or(false))
            .collect();
        Self::new(segs)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.total == 0
    }

    #[must_use]
    pub fn locate(&self, p: Point) -> Option<usize> {
        self.segments.iter().position(|s| s.contains(p))
    }

    // Fractions are endpoint-preserving: the first pixel of an edge is 0.0 and
    // the last is 1.0, so edges of different lengths line up at both ends.

    /// Continuous fraction of `p` (which must lie on segment `idx`).
    #[must_use]
    #[allow(clippy::cast_precision_loss)]
    pub fn t_continuous(&self, idx: usize, p: Point) -> f64 {
        let s = &self.segments[idx];
        let off = self.prefix[idx] + i64::from(s.along(p) - s.start);
        off as f64 / (self.total - 1).max(1) as f64
    }

    /// Fraction of `p` inside its own segment.
    #[must_use]
    pub fn t_segment(&self, idx: usize, p: Point) -> f64 {
        let s = &self.segments[idx];
        f64::from(s.along(p) - s.start) / f64::from((s.len() - 1).max(1))
    }

    /// Point at continuous fraction `t`; returns the segment index too.
    #[must_use]
    #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
    pub fn point_continuous(&self, t: f64) -> Option<(usize, Point)> {
        if self.is_empty() {
            return None;
        }
        let off = ((t.clamp(0.0, 1.0) * (self.total - 1) as f64).round() as i64).clamp(0, self.total - 1);
        let idx = match self.prefix.binary_search(&off) {
            Ok(i) => i,
            Err(i) => i - 1,
        };
        let s = &self.segments[idx];
        let a = s.start + i32::try_from(off - self.prefix[idx]).unwrap_or(0);
        Some((idx, s.point_at(a)))
    }

    /// Point at fraction `u` of segment `idx`.
    #[must_use]
    #[allow(clippy::cast_possible_truncation)]
    pub fn point_in_segment(&self, idx: usize, u: f64) -> Option<Point> {
        let s = self.segments.get(idx)?;
        let a = s.start + (u.clamp(0.0, 1.0) * f64::from(s.len() - 1)).round() as i32;
        Some(s.point_at(a))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glidedesk_proto::MonitorId;

    fn mon(id: &str, r: Rect) -> MonitorInfo {
        MonitorInfo { id: MonitorId(id.into()), name: id.into(), bounds: r, scale: 1.0, primary: false }
    }

    #[test]
    fn side_by_side_internal_edges_are_not_outer() {
        let rects = [Rect::new(0, 0, 100, 50), Rect::new(100, 0, 100, 50)];
        let right = outer_segments(&rects, Side::Right);
        assert_eq!(right, vec![Segment { monitor: 1, side: Side::Right, fixed: 199, start: 0, end: 50 }]);
        let left = outer_segments(&rects, Side::Left);
        assert_eq!(left, vec![Segment { monitor: 0, side: Side::Left, fixed: 0, start: 0, end: 50 }]);
    }

    #[test]
    fn partially_covered_edge_splits() {
        // Monitor 1 covers only the middle of monitor 0's right edge.
        let rects = [Rect::new(0, 0, 100, 100), Rect::new(100, 25, 100, 50)];
        let right0: Vec<_> = outer_segments(&rects, Side::Right).into_iter().filter(|s| s.monitor == 0).collect();
        assert_eq!(right0.len(), 2);
        assert_eq!((right0[0].start, right0[0].end), (0, 25));
        assert_eq!((right0[1].start, right0[1].end), (75, 100));
    }

    #[test]
    fn stacked_monitors_both_have_outer_right_edges() {
        let mons = [mon("top", Rect::new(0, 0, 100, 50)), mon("bottom", Rect::new(0, 50, 100, 50))];
        let run = EdgeRun::build(&mons, Side::Right, &[true, true]);
        assert_eq!(run.segments.len(), 2);
        // Continuous: top of monitor 1 → t≈0, bottom of monitor 2 → t≈1.
        let t0 = run.t_continuous(0, Point::new(99, 0));
        let t1 = run.t_continuous(1, Point::new(99, 99));
        assert!(t0 < 0.01 && t1 > 0.99);
        assert_eq!(run.point_continuous(0.5).unwrap().1, Point::new(99, 50));
        assert!((t0 - 0.0).abs() < f64::EPSILON && (t1 - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn selection_falls_back_to_all_when_monitor_missing() {
        let mons = [mon("a", Rect::new(0, 0, 10, 10)), mon("b", Rect::new(10, 0, 10, 10))];
        let s = select(&mons, &MonitorSelection::single(MonitorId("gone".into())));
        assert!(s.fell_back);
        assert_eq!(s.mask, vec![true, true]);
        let s = select(&mons, &MonitorSelection::selected(vec![MonitorId("b".into()), MonitorId("gone".into())]));
        assert!(!s.fell_back);
        assert_eq!(s.mask, vec![false, true]);
    }

    #[test]
    fn continuous_roundtrip_every_pixel() {
        let mons = [mon("a", Rect::new(0, 0, 100, 37)), mon("b", Rect::new(-50, 37, 150, 63))];
        let run = EdgeRun::build(&mons, Side::Right, &[true, true]);
        for (idx, seg) in run.segments.iter().enumerate() {
            for a in seg.start..seg.end {
                let p = seg.point_at(a);
                let t = run.t_continuous(idx, p);
                assert_eq!(run.point_continuous(t).unwrap(), (idx, p));
            }
        }
    }
}
