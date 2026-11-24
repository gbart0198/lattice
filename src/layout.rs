//! Pure tiling math: given an area and a window count, produce one rectangle per window.

use serde::Deserialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
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

pub struct Params {
    pub gaps_inner: i32,
    pub gaps_outer: i32,
    /// dwindle: share of the first half of each split; master: master column width share.
    pub ratio: f32,
}

/// Lays out `n` windows in `area`. Gaps: `gaps_outer` at the edges, `gaps_inner` between tiles.
pub fn tile(kind: LayoutKind, area: Rect, n: usize, p: &Params) -> Vec<Rect> {
    if n == 0 {
        return Vec::new();
    }
    let half = p.gaps_inner / 2;
    // Shrink by (outer - inner/2), then each tile by inner/2: edges get `outer`, seams `inner`.
    let inner_area = area.inset(p.gaps_outer - half);
    let raw = match kind {
        LayoutKind::Monocle => vec![inner_area; n],
        LayoutKind::Dwindle => dwindle(inner_area, n, p.ratio),
        LayoutKind::Master => master(inner_area, n, p.ratio),
    };
    raw.into_iter().map(|r| r.inset(half)).collect()
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
fn dwindle(area: Rect, n: usize, ratio: f32) -> Vec<Rect> {
    let mut out = Vec::with_capacity(n);
    let mut rest = area;
    for i in 0..n {
        if i == n - 1 {
            out.push(rest);
            break;
        }
        let (a, b) = split(rest, ratio, rest.w > rest.h);
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

#[cfg(test)]
mod tests {
    use super::*;

    const P: Params = Params { gaps_inner: 0, gaps_outer: 0, ratio: 0.5 };

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
        let p = Params { gaps_inner: 10, gaps_outer: 20, ratio: 0.5 };
        let r = tile(LayoutKind::Dwindle, Rect::new(0, 0, 1000, 500), 2, &p);
        assert_eq!(r[0].x, 20);
        assert_eq!(r[0].y, 20);
        assert_eq!(r[1].x - r[0].right(), 10);
        assert_eq!(1000 - r[1].right(), 20);
    }
}
