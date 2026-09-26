//! Integer screen geometry shared by every crate.
//!
//! Coordinates are in each machine's own native desktop space (logical points
//! on macOS, physical pixels on Windows). Mapping *between* machines is always
//! done with normalised positions along an edge, so the units never mix.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

impl Point {
    #[must_use]
    pub const fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }
}

/// Axis-aligned rectangle. `right()`/`bottom()` are exclusive.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    #[must_use]
    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self { x, y, w, h }
    }

    #[must_use]
    pub const fn left(&self) -> i32 {
        self.x
    }
    #[must_use]
    pub const fn top(&self) -> i32 {
        self.y
    }
    /// Exclusive right edge.
    #[must_use]
    pub const fn right(&self) -> i32 {
        self.x.saturating_add(self.w)
    }
    /// Exclusive bottom edge.
    #[must_use]
    pub const fn bottom(&self) -> i32 {
        self.y.saturating_add(self.h)
    }

    #[must_use]
    pub const fn is_valid(&self) -> bool {
        self.w > 0 && self.h > 0
    }

    #[must_use]
    pub const fn contains(&self, p: Point) -> bool {
        p.x >= self.left() && p.x < self.right() && p.y >= self.top() && p.y < self.bottom()
    }

    /// Nearest point inside the rectangle.
    #[must_use]
    pub fn clamp(&self, p: Point) -> Point {
        Point { x: p.x.clamp(self.left(), self.right() - 1), y: p.y.clamp(self.top(), self.bottom() - 1) }
    }

    #[must_use]
    pub fn intersects(&self, o: &Rect) -> bool {
        self.left() < o.right() && o.left() < self.right() && self.top() < o.bottom() && o.top() < self.bottom()
    }

    /// Bounding box of several rectangles; `None` for an empty iterator.
    pub fn union_all<'a>(rects: impl IntoIterator<Item = &'a Rect>) -> Option<Rect> {
        let mut it = rects.into_iter();
        let first = *it.next()?;
        let (mut l, mut t, mut r, mut b) = (first.left(), first.top(), first.right(), first.bottom());
        for rc in it {
            l = l.min(rc.left());
            t = t.min(rc.top());
            r = r.max(rc.right());
            b = b.max(rc.bottom());
        }
        Some(Rect::new(l, t, r - l, b - t))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    Left,
    Right,
    Top,
    Bottom,
}

impl Side {
    pub const ALL: [Side; 4] = [Side::Left, Side::Right, Side::Top, Side::Bottom];

    #[must_use]
    pub const fn opposite(self) -> Side {
        match self {
            Side::Left => Side::Right,
            Side::Right => Side::Left,
            Side::Top => Side::Bottom,
            Side::Bottom => Side::Top,
        }
    }

    /// `true` for sides whose edge runs vertically (left/right).
    #[must_use]
    pub const fn is_vertical_edge(self) -> bool {
        matches!(self, Side::Left | Side::Right)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_edges_are_exclusive() {
        let r = Rect::new(0, 0, 10, 5);
        assert!(r.contains(Point::new(9, 4)));
        assert!(!r.contains(Point::new(10, 4)));
        assert!(!r.contains(Point::new(9, 5)));
        assert_eq!(r.clamp(Point::new(50, -3)), Point::new(9, 0));
    }

    #[test]
    fn union_covers_all() {
        let a = Rect::new(0, 0, 10, 10);
        let b = Rect::new(10, -5, 5, 5);
        assert_eq!(Rect::union_all([&a, &b]), Some(Rect::new(0, -5, 15, 15)));
        assert_eq!(Rect::union_all(std::iter::empty()), None);
    }

    #[test]
    fn opposite_is_involution() {
        for s in Side::ALL {
            assert_eq!(s.opposite().opposite(), s);
        }
    }
}
