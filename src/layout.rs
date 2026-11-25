//! Pure tiling math: given an area and a window count, produce one rectangle per window.

use serde::Deserialize;

use crate::config::Dir;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self { x, y, w, h }
    }
    pub fn right(&self) -> i32 {
        self.x + self.w
    }
    pub fn bottom(&self) -> i32 {
        self.y + self.h
    }
    pub fn center(&self) -> (i32, i32) {
        (self.x + self.w / 2, self.y + self.h / 2)
    }
    pub fn contains(&self, (px, py): (i32, i32)) -> bool {
        px >= self.x && px < self.right() && py >= self.y && py < self.bottom()
    }
    pub fn inset(&self, d: i32) -> Rect {
        Rect::new(self.x + d, self.y + d, (self.w - 2 * d).max(1), (self.h - 2 * d).max(1))
    }
}

#[derive(Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum LayoutKind {
    Dwindle,
    Master,
    Monocle,
}

impl LayoutKind {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "dwindle" | "spiral" | "bsp" => Some(Self::Dwindle),
            "master" | "master-stack" | "tall" => Some(Self::Master),
            "monocle" | "full" => Some(Self::Monocle),
            _ => None,
        }
    }
    pub fn next(self) -> Self {
        match self {
            Self::Dwindle => Self::Master,
            Self::Master => Self::Monocle,
            Self::Monocle => Self::Dwindle,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::Dwindle => "dwindle",
            Self::Master => "master",
            Self::Monocle => "monocle",
        }
    }
}

pub struct Params<'a> {
    pub gaps_inner: i32,
    pub gaps_outer: i32,
    /// dwindle: share of the first half of each split; master: master column width share.
    pub ratio: f32,
    /// dwindle: per-split overrides of `ratio`; split `i` divides window `i` from the rest.
    pub splits: &'a [f32],
}

impl Params<'_> {
    fn split_ratio(&self, i: usize) -> f32 {
        self.splits.get(i).copied().unwrap_or(self.ratio)
    }

    /// The area tiles are carved from: shrunk by (outer - inner/2), then each tile by inner/2,
    /// so edges get `outer` and seams `inner`.
    fn inner(&self, area: Rect) -> Rect {
        area.inset(self.gaps_outer - self.gaps_inner / 2)
    }
}

/// Lays out `n` windows in `area`. Gaps: `gaps_outer` at the edges, `gaps_inner` between tiles.
pub fn tile(kind: LayoutKind, area: Rect, n: usize, p: &Params) -> Vec<Rect> {
    if n == 0 {
        return Vec::new();
    }
    let inner_area = p.inner(area);
    let raw = match kind {
        LayoutKind::Monocle => vec![inner_area; n],
        LayoutKind::Dwindle => dwindle(inner_area, n, p),
        LayoutKind::Master => master(inner_area, n, p.ratio),
    };
    raw.into_iter().map(|r| r.inset(p.gaps_inner / 2)).collect()
}

fn split(r: Rect, ratio: f32, vertical: bool) -> (Rect, Rect) {
    if vertical {
        let w1 = (r.w as f32 * ratio).round() as i32;
        (Rect::new(r.x, r.y, w1, r.h), Rect::new(r.x + w1, r.y, r.w - w1, r.h))
    } else {
        let h1 = (r.h as f32 * ratio).round() as i32;
        (Rect::new(r.x, r.y, r.w, h1), Rect::new(r.x, r.y + h1, r.w, r.h - h1))
    }
}

/// Hyprland-style dwindle: each window takes part of what's left, splitting along the longer
/// side, so windows spiral inward.
fn dwindle(area: Rect, n: usize, p: &Params) -> Vec<Rect> {
    let mut out = Vec::with_capacity(n);
    let mut rest = area;
    for i in 0..n {
        if i == n - 1 {
            out.push(rest);
            break;
        }
        let (a, b) = split(rest, p.split_ratio(i), rest.w > rest.h);
        out.push(a);
        rest = b;
    }
    out
}

fn master(area: Rect, n: usize, ratio: f32) -> Vec<Rect> {
    if n == 1 {
        return vec![area];
    }
    let (m, stack) = split(area, ratio, true);
    let mut out = vec![m];
    let k = (n - 1) as i32;
    for i in 0..k {
        let y0 = stack.y + stack.h * i / k;
        let y1 = stack.y + stack.h * (i + 1) / k;
        out.push(Rect::new(stack.x, y0, stack.w, y1 - y0));
    }
    out
}

/// What a directional resize adjusts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResizeTarget {
    /// The dwindle split at this index.
    Split(usize),
    /// The master column's width.
    Master,
}

/// Finds the boundary that "expand window `k` toward `dir`" moves. The boundary always moves
/// toward `dir` (see `nudge`): normally it's the edge of window `k` on the `dir` side, which
/// grows the window. A window touching the screen edge on that side has no such boundary, so
/// its opposite edge moves instead, shrinking it (like i3 / Hyprland). Monocle, and master's
/// up/down (stack rows are always even), have nothing to resize.
pub fn resize_target(kind: LayoutKind, area: Rect, n: usize, k: usize, p: &Params, dir: Dir) -> Option<ResizeTarget> {
    let vertical = matches!(dir, Dir::Left | Dir::Right);
    let forward = matches!(dir, Dir::Right | Dir::Down);
    match kind {
        LayoutKind::Monocle => None,
        LayoutKind::Master => (n >= 2 && k < n && vertical).then_some(ResizeTarget::Master),
        LayoutKind::Dwindle => {
            if k >= n {
                return None;
            }
            // Window k is the first half of split k and the second half of every split before
            // it, so split k is its right/bottom edge and the latest matching earlier one its
            // left/top edge.
            let (mut before, mut after) = (None, None);
            let mut rest = p.inner(area);
            for i in 0..(n - 1).min(k + 1) {
                let v = rest.w > rest.h;
                if v == vertical {
                    if i == k {
                        after = Some(i);
                    } else {
                        before = Some(i);
                    }
                }
                rest = split(rest, p.split_ratio(i), v).1;
            }
            let found = if forward { after.or(before) } else { before.or(after) };
            found.map(ResizeTarget::Split)
        }
    }
}

/// Moves a ratio's boundary `amount` toward `dir`: right/down grows the first half.
pub fn nudge(ratio: f32, dir: Dir, amount: f32) -> f32 {
    let d = if matches!(dir, Dir::Right | Dir::Down) { amount } else { -amount };
    (ratio + d).clamp(0.1, 0.9)
}

#[cfg(test)]
mod tests {
    use super::*;

    const P: Params<'static> = Params { gaps_inner: 0, gaps_outer: 0, ratio: 0.5, splits: &[] };

    #[test]
    fn dwindle_three() {
        let r = tile(LayoutKind::Dwindle, Rect::new(0, 0, 1000, 500), 3, &P);
        assert_eq!(r[0], Rect::new(0, 0, 500, 500));
        assert_eq!(r[1], Rect::new(500, 0, 500, 250));
        assert_eq!(r[2], Rect::new(500, 250, 500, 250));
    }

    #[test]
    fn master_stack() {
        let p = Params { ratio: 0.6, ..P };
        let r = tile(LayoutKind::Master, Rect::new(0, 0, 1000, 600), 4, &p);
        assert_eq!(r[0], Rect::new(0, 0, 600, 600));
        assert_eq!(r[3], Rect::new(600, 400, 400, 200));
    }

    #[test]
    fn gaps() {
        let p = Params { gaps_inner: 10, gaps_outer: 20, ..P };
        let r = tile(LayoutKind::Dwindle, Rect::new(0, 0, 1000, 500), 2, &p);
        assert_eq!(r[0].x, 20);
        assert_eq!(r[0].y, 20);
        assert_eq!(r[1].x - r[0].right(), 10);
        assert_eq!(1000 - r[1].right(), 20);
    }

    const WIDE: Rect = Rect::new(0, 0, 1000, 500);

    fn target(kind: LayoutKind, n: usize, k: usize, dir: Dir) -> Option<ResizeTarget> {
        resize_target(kind, WIDE, n, k, &P, dir)
    }

    #[test]
    fn per_split_ratios() {
        let p = Params { splits: &[0.7], ..P };
        let r = tile(LayoutKind::Dwindle, WIDE, 3, &p);
        assert_eq!(r[0], Rect::new(0, 0, 700, 500));
        // Split 1 has no override: the default ratio.
        assert_eq!(r[1], Rect::new(700, 0, 300, 250));
    }

    #[test]
    fn dwindle_resize_toward_neighbor() {
        use ResizeTarget::Split;
        // Window 0 left; windows 1 (top) and 2 (bottom) on the right.
        assert_eq!(target(LayoutKind::Dwindle, 3, 0, Dir::Right), Some(Split(0)));
        assert_eq!(target(LayoutKind::Dwindle, 3, 1, Dir::Left), Some(Split(0)));
        assert_eq!(target(LayoutKind::Dwindle, 3, 2, Dir::Left), Some(Split(0)));
        assert_eq!(target(LayoutKind::Dwindle, 3, 1, Dir::Down), Some(Split(1)));
        assert_eq!(target(LayoutKind::Dwindle, 3, 2, Dir::Up), Some(Split(1)));
        // Expanding right grows window 0.
        let ratio = nudge(0.5, Dir::Right, 0.05);
        let p = Params { splits: &[ratio], ..P };
        assert_eq!(tile(LayoutKind::Dwindle, WIDE, 3, &p)[0].w, 550);
    }

    #[test]
    fn dwindle_resize_at_screen_edge() {
        use ResizeTarget::Split;
        // At the edge, the opposite boundary moves the same way (shrinking the window).
        assert_eq!(target(LayoutKind::Dwindle, 3, 1, Dir::Right), Some(Split(0)));
        assert_eq!(target(LayoutKind::Dwindle, 3, 0, Dir::Left), Some(Split(0)));
        assert_eq!(target(LayoutKind::Dwindle, 3, 1, Dir::Up), Some(Split(1)));
        assert_eq!(nudge(0.5, Dir::Left, 0.05), 0.45);
        // No split along that axis at all, or nothing to split.
        assert_eq!(target(LayoutKind::Dwindle, 3, 0, Dir::Up), None);
        assert_eq!(target(LayoutKind::Dwindle, 1, 0, Dir::Right), None);
    }

    #[test]
    fn dwindle_resize_picks_nearest_split() {
        use ResizeTarget::Split;
        // 5 windows on a wide screen: splits alternate vertical, horizontal, vertical, ...
        // Window 2's left edge is split 0 (split 1 is horizontal), its right edge split 2.
        assert_eq!(target(LayoutKind::Dwindle, 5, 2, Dir::Left), Some(Split(0)));
        assert_eq!(target(LayoutKind::Dwindle, 5, 2, Dir::Right), Some(Split(2)));
        // Window 3's top edge is split 1 (not split 2, which is vertical), its bottom split 3.
        assert_eq!(target(LayoutKind::Dwindle, 5, 3, Dir::Up), Some(Split(1)));
        assert_eq!(target(LayoutKind::Dwindle, 5, 3, Dir::Down), Some(Split(3)));
    }

    #[test]
    fn master_and_monocle_resize() {
        assert_eq!(target(LayoutKind::Master, 3, 0, Dir::Right), Some(ResizeTarget::Master));
        assert_eq!(target(LayoutKind::Master, 3, 2, Dir::Left), Some(ResizeTarget::Master));
        assert_eq!(target(LayoutKind::Master, 3, 1, Dir::Up), None);
        assert_eq!(target(LayoutKind::Master, 1, 0, Dir::Right), None);
        assert_eq!(target(LayoutKind::Monocle, 3, 0, Dir::Right), None);
        assert_eq!(nudge(0.88, Dir::Right, 0.05), 0.9);
    }
}
