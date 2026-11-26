//! Window animations: moves, swaps, fullscreen and reflows glide to their new tile, and a
//! workspace switch slides one workspace out and the next one in.
//!
//! The compositor does the animating. While a window animates, lattice makes it fully
//! transparent (it keeps drawing, it just isn't shown) and a live DWM preview of it glides on a
//! click-through overlay. A window changing tiles stays put during the glide, so its preview
//! shows it fully drawn, just scaled; when the preview lands, the window is moved there in one
//! go, made opaque once it has arrived, and the preview goes. Apps get one move per animation,
//! never a stream of them, so every animation runs at the display's refresh rate however heavy
//! the app. A window that can't be made transparent or previewed (one running as administrator,
//! say) doesn't animate at all: it moves instantly. Nothing is ever stepped.
//!
//! Cost: lattice opens no GPU device. The overlay has no surface of its own
//! (`WS_EX_NOREDIRECTIONBITMAP`); previews show the surfaces apps already have. The overlay
//! exists only while something moves, and so does the frame clock: a small thread that waits
//! for each DWM composition (`DwmFlush`) and wakes the WM thread to move the previews. Idle or
//! off, nothing here exists.

mod dwm;
pub mod preview;

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::config::AnimStyle;
use crate::layout::Rect;
use crate::win::{Faded, Hwnd};

/// What a window is doing, which picks its timing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Gliding to a new spot: moves, swaps, fullscreen, reflows after a minimize.
    Move,
    /// Sliding in or out on a workspace switch.
    Slide,
}

/// What happens to a window when its animation ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum After {
    Nothing,
    /// Hide it: it slid off with its workspace. (The window itself never moved; only its preview
    /// did, so it's already where it belongs.)
    Hide,
}

/// A CSS-style cubic Bézier easing curve through (0,0), (x1,y1), (x2,y2), (1,1). `y` may leave
/// 0..1 for overshoot.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bezier {
    pub x1: f32,
    pub y1: f32,
    pub x2: f32,
    pub y2: f32,
}

impl Bezier {
    pub const fn new(x1: f32, y1: f32, x2: f32, y2: f32) -> Self {
        Self { x1, y1, x2, y2 }
    }

    fn coord(t: f32, a: f32, b: f32) -> f32 {
        // Bernstein form with p0 = 0, p3 = 1.
        let u = 1.0 - t;
        3.0 * u * u * t * a + 3.0 * u * t * t * b + t * t * t
    }

    fn slope(t: f32, a: f32, b: f32) -> f32 {
        let u = 1.0 - t;
        3.0 * u * u * a + 6.0 * u * t * (b - a) + 3.0 * t * t * (1.0 - b)
    }

    /// Eased progress at time fraction `x` (0..1).
    pub fn at(&self, x: f32) -> f32 {
        if x <= 0.0 {
            return 0.0;
        }
        if x >= 1.0 {
            return 1.0;
        }
        // Solve coord(t) = x for t: Newton first (fast), bisection if the slope is too flat.
        let mut t = x;
        for _ in 0..8 {
            let err = Self::coord(t, self.x1, self.x2) - x;
            if err.abs() < 1e-5 {
                return Self::coord(t, self.y1, self.y2);
            }
            let d = Self::slope(t, self.x1, self.x2);
            if d.abs() < 1e-6 {
                break;
            }
            t = (t - err / d).clamp(0.0, 1.0);
        }
        let (mut lo, mut hi) = (0.0f32, 1.0f32);
        t = x;
        for _ in 0..30 {
            let v = Self::coord(t, self.x1, self.x2);
            if (v - x).abs() < 1e-5 {
                break;
            }
            if v < x {
                lo = t;
            } else {
                hi = t;
            }
            t = (lo + hi) / 2.0;
        }
        Self::coord(t, self.y1, self.y2)
    }
}

/// How long a kind of animation takes and how it eases, per style.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Timing {
    pub dur: Duration,
    pub curve: Bezier,
    /// Delay between consecutive windows sliding in (a cascade); zero for none.
    pub stagger: Duration,
}

pub fn timing(style: AnimStyle, kind: Kind) -> Timing {
    let ms = Duration::from_millis;
    match (style, kind) {
        // Fancy: long enough to follow, settling with a slight overshoot (Hyprland's default
        // curve), and workspaces that cascade in on a long exponential ease-out.
        (AnimStyle::Fancy, Kind::Move) => Timing { dur: ms(340), curve: Bezier::new(0.05, 0.9, 0.1, 1.05), stagger: ms(0) },
        (AnimStyle::Fancy, Kind::Slide) => Timing { dur: ms(380), curve: Bezier::new(0.16, 1.0, 0.3, 1.0), stagger: ms(35) },
        // Fast: there and done, a plain ease-out with no overshoot.
        (AnimStyle::Fast, Kind::Move) => Timing { dur: ms(140), curve: Bezier::new(0.33, 1.0, 0.68, 1.0), stagger: ms(0) },
        (AnimStyle::Fast, Kind::Slide) => Timing { dur: ms(170), curve: Bezier::new(0.33, 1.0, 0.68, 1.0), stagger: ms(0) },
    }
}

/// How long a window coming back from a hidden workspace gets to draw before its preview starts
/// sliding in: apps take two or three frames to paint after being shown, and until then their
/// preview is blank.
pub const WARMUP: Duration = Duration::from_millis(50);

/// A live preview's handle (a DWM thumbnail).
pub type Thumb = isize;

/// What the animator needs from Windows (the real one is `dwm::Dwm`; tests record calls).
pub trait Backend {
    /// Makes `h` invisible while it keeps drawing. None if it can't be.
    fn fade(&mut self, h: Hwnd) -> Option<Faded>;
    fn unfade(&mut self, h: Hwnd, f: Faded);
    /// Starts a live preview of `h` (making the overlay if needed). None if it can't.
    fn preview(&mut self, h: Hwnd) -> Option<Thumb>;
    /// Draws preview `t` at screen rect `at`, cut off outside `clip`.
    fn show(&mut self, t: Thumb, at: Rect, clip: Option<Rect>);
    fn drop_preview(&mut self, t: Thumb);
    /// No previews are left: the overlay can go.
    fn idle(&mut self);
    fn is_visible(&self, h: Hwnd) -> bool;
    /// Moves the real window to `r` (its visible frame).
    fn place(&mut self, h: Hwnd, r: Rect);
    /// Whether the real window is at `r` yet (moves reach apps asynchronously).
    fn arrived(&self, h: Hwnd, r: Rect) -> bool;
}

/// One animation: a window's preview going from `from` to `to`.
#[derive(Clone, Copy, Debug)]
pub struct Glide {
    pub from: Rect,
    pub to: Rect,
    pub timing: Timing,
    pub delay: Duration,
    pub after: After,
    /// Previews are cut off outside this (a monitor's tiling area, for slides).
    pub clip: Option<Rect>,
    /// The animator moves the real window to `to` when the preview lands. Otherwise it's
    /// already there (a window coming in with its workspace) or stays (one sliding away).
    pub moves: bool,
}

impl Glide {
    /// A window changing tiles: it moves when its preview lands.
    pub fn moving(from: Rect, to: Rect, timing: Timing) -> Self {
        Self { from, to, timing, delay: Duration::ZERO, after: After::Nothing, clip: None, moves: true }
    }

    /// A window sliding with its workspace, cut off outside `clip`; it doesn't move.
    pub fn sliding(from: Rect, to: Rect, timing: Timing, clip: Option<Rect>) -> Self {
        Self { from, to, timing, delay: Duration::ZERO, after: After::Nothing, clip, moves: false }
    }

    pub fn delay(self, delay: Duration) -> Self {
        Self { delay, ..self }
    }

    /// Hide the window when the slide ends.
    pub fn hiding(self) -> Self {
        Self { after: After::Hide, ..self }
    }
}

fn lerp(a: i32, b: i32, k: f32) -> i32 {
    (a as f32 + (b - a) as f32 * k).round() as i32
}

/// The rect `k` of the way from `a` to `b` (k may overshoot); never smaller than 1x1.
pub fn lerp_rect(a: Rect, b: Rect, k: f32) -> Rect {
    Rect::new(lerp(a.x, b.x, k), lerp(a.y, b.y, k), lerp(a.w, b.w, k).max(1), lerp(a.h, b.h, k).max(1))
}

/// For a preview of a `src`-sized source drawn at `dest`: the part of `dest` inside `clip`, and
/// the matching part of the source. None when nothing of it is inside.
pub fn clip_preview(dest: Rect, clip: Option<Rect>, src: (i32, i32)) -> Option<(Rect, Rect)> {
    let full = Rect::new(0, 0, src.0, src.1);
    let Some(c) = clip else { return Some((dest, full)) };
    let (x0, y0) = (dest.x.max(c.x), dest.y.max(c.y));
    let (x1, y1) = (dest.right().min(c.right()), dest.bottom().min(c.bottom()));
    if x1 <= x0 || y1 <= y0 || dest.w <= 0 || dest.h <= 0 {
        return None;
    }
    let sx = |x: i32| ((x - dest.x) as i64 * src.0 as i64 / dest.w as i64) as i32;
    let sy = |y: i32| ((y - dest.y) as i64 * src.1 as i64 / dest.h as i64) as i32;
    Some((Rect::new(x0, y0, x1 - x0, y1 - y0), Rect::new(sx(x0), sy(y0), sx(x1) - sx(x0), sy(y1) - sy(y0))))
}

struct Anim {
    from: Rect,
    to: Rect,
    start: Instant,
    dur: Duration,
    curve: Bezier,
    /// Where the preview was last drawn, for retargeting.
    shown: Rect,
    after: After,
    /// Previews are cut off outside this (a monitor, for slides); None draws them anywhere.
    clip: Option<Rect>,
    thumb: Thumb,
    faded: Faded,
    /// The real window still has to move to `to`, near the end of the glide.
    moves: bool,
    /// The animator moved it: it's opaque again once it's arrived.
    placed: bool,
    stage: Stage,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stage {
    Gliding,
    /// The preview landed; the window was moved there and hasn't arrived yet.
    Settling { since: Instant },
    /// Made opaque; the preview goes on the next frame, once the window shows.
    Landing,
}

impl Anim {
    /// How far through the glide it is at `now`, 0 to 1.
    fn progress(&self, now: Instant) -> f32 {
        let Some(elapsed) = now.checked_duration_since(self.start) else { return 0.0 };
        if self.dur.is_zero() {
            return 1.0;
        }
        (elapsed.as_secs_f32() / self.dur.as_secs_f32()).min(1.0)
    }

    /// The rect at `now`, and whether the animation is over.
    fn at(&self, now: Instant) -> (Rect, bool) {
        let Some(elapsed) = now.checked_duration_since(self.start) else { return (self.from, false) };
        if elapsed >= self.dur || self.dur.is_zero() {
            return (self.to, true);
        }
        let k = self.curve.at(elapsed.as_secs_f32() / self.dur.as_secs_f32());
        (lerp_rect(self.from, self.to, k), false)
    }
}

/// How long a window that slid away stays transparent waiting for its hide to land, at most.
const HIDE_GRACE: Duration = Duration::from_millis(600);
/// How long a landed preview waits for its window to arrive, at most (a hung app).
const SETTLE_MAX: Duration = Duration::from_millis(250);
/// How far into a glide the real window moves. The curves are ease-outs, so by then the
/// preview is all but there: the app's own redraw at its new size happens while the preview
/// is still, not as a hitch after it stops.
const MOVE_AT: f32 = 0.6;

pub struct Animator<B: Backend = dwm::Dwm> {
    anims: HashMap<Hwnd, Anim>,
    /// Windows that slid away and are being hidden: made opaque once they're gone.
    unfading: Vec<(Hwnd, Faded, Instant)>,
    /// The running frame clock's keep-going flag; None when no clock thread exists.
    clock: Option<Arc<AtomicBool>>,
    backend: B,
    /// Wakes whoever calls `tick` (the WM thread); the preview ticks on its own.
    wake: fn(),
}

impl Animator {
    pub fn new() -> Self {
        Self::with(dwm::Dwm::default(), crate::shared::wake_wm)
    }
}

impl<B: Backend> Animator<B> {
    pub fn with(backend: B, wake: fn()) -> Self {
        Self { anims: HashMap::new(), unfading: Vec::new(), clock: None, backend, wake }
    }

    /// Anything still to do: previews moving or landing, windows waiting to be made opaque.
    pub fn busy(&self) -> bool {
        !self.anims.is_empty() || !self.unfading.is_empty()
    }

    pub fn is_animating(&self, h: Hwnd) -> bool {
        self.anims.contains_key(&h)
    }

    /// Where an animating window is headed.
    pub fn target(&self, h: Hwnd) -> Option<Rect> {
        self.anims.get(&h).map(|a| a.to)
    }

    /// Where an animating window appears to be right now (its preview).
    pub fn current(&self, h: Hwnd) -> Option<Rect> {
        self.anims.get(&h).map(|a| a.shown)
    }

    /// Starts animating `h` (see `Glide`). A window already animating turns around from where
    /// it appears to be. False when `h` can't be animated: the caller moves it itself, at once.
    pub fn glide(&mut self, h: Hwnd, g: Glide) -> bool {
        let start = Instant::now() + g.delay;
        if let Some(a) = self.anims.get_mut(&h) {
            if a.stage == Stage::Landing {
                // Already made opaque: start over from here.
                let a = self.anims.remove(&h).unwrap();
                self.backend.drop_preview(a.thumb);
                return self.glide(h, Glide { from: a.to, ..g });
            }
            // Settling, it's been moved to its old target: a moving glide moves it again.
            a.from = a.shown;
            a.to = g.to;
            a.start = start;
            a.dur = g.timing.dur;
            a.curve = g.timing.curve;
            a.after = g.after;
            a.clip = g.clip;
            // A new target means a new move, if it was already moved to the old one.
            a.moves = g.moves || a.moves;
            a.placed = a.placed && !a.moves;
            a.stage = Stage::Gliding;
            return true;
        }
        // It may still be waiting to be made opaque after sliding away: it's coming back.
        let pending = self.unfading.iter().position(|u| u.0 == h).map(|i| self.unfading.remove(i).1);
        let Some(thumb) = self.backend.preview(h) else {
            if let Some(f) = pending {
                self.backend.unfade(h, f);
            }
            self.settle();
            return false;
        };
        let Some(faded) = pending.or_else(|| self.backend.fade(h)) else {
            self.backend.drop_preview(thumb);
            self.settle();
            return false;
        };
        self.backend.show(thumb, g.from, g.clip);
        let a = Anim {
            from: g.from,
            to: g.to,
            start,
            dur: g.timing.dur,
            curve: g.timing.curve,
            shown: g.from,
            after: g.after,
            clip: g.clip,
            thumb,
            faded,
            moves: g.moves,
            placed: false,
            stage: Stage::Gliding,
        };
        self.anims.insert(h, a);
        self.ensure_clock();
        true
    }

    /// Puts `h`'s preview on top of the others (the window you moved crosses over the one it
    /// swaps with). Previews stack in the order they were made, so it gets a new one.
    pub fn raise(&mut self, h: Hwnd) {
        let Some(a) = self.anims.get(&h) else { return };
        let (old, shown, clip) = (a.thumb, a.shown, a.clip);
        let Some(t) = self.backend.preview(h) else { return };
        self.backend.show(t, shown, clip);
        self.backend.drop_preview(old);
        self.anims.get_mut(&h).unwrap().thumb = t;
    }

    /// Stops animating `h`; returns what it was going to do at the end. With `hiding`, the
    /// caller is hiding it: it's made opaque again once it's gone, so it never flashes.
    pub fn cancel(&mut self, h: Hwnd, hiding: bool) -> Option<After> {
        let a = self.anims.remove(&h)?;
        self.backend.drop_preview(a.thumb);
        if a.moves {
            // Cut short before it moved: it goes where it was headed, so layout stays right.
            self.backend.place(h, a.to);
        }
        if a.stage == Stage::Landing {
            // Already opaque.
        } else if hiding {
            self.unfading.push((h, a.faded, Instant::now()));
        } else {
            self.backend.unfade(h, a.faded);
        }
        self.settle();
        Some(a.after)
    }

    /// Whether `h` is sliding away to be hidden.
    pub fn is_hiding(&self, h: Hwnd) -> bool {
        self.anims.get(&h).is_some_and(|a| a.after == After::Hide)
    }

    /// Drops a pending end-of-slide hide (the window's workspace came back before it finished
    /// sliding away). True if there was one: the window is still on screen.
    pub fn take_hide(&mut self, h: Hwnd) -> bool {
        match self.anims.get_mut(&h) {
            Some(a) if a.after == After::Hide => {
                a.after = After::Nothing;
                true
            }
            _ => false,
        }
    }

    /// A window that slid away won't be hidden after all: make it opaque now.
    pub fn unfade_now(&mut self, h: Hwnd) {
        if let Some(i) = self.unfading.iter().position(|u| u.0 == h) {
            let (h, f, _) = self.unfading.remove(i);
            self.backend.unfade(h, f);
        }
        self.settle();
    }

    /// Moves every preview one frame on; returns the windows whose animation ended, with what's
    /// left to do (for `After::Hide`: hide it; it's made opaque once it's gone).
    pub fn tick(&mut self, now: Instant) -> Vec<(Hwnd, After)> {
        let mut done = Vec::new();
        let mut gone = Vec::new();
        for (h, a) in self.anims.iter_mut() {
            match a.stage {
                Stage::Landing => {
                    // Made opaque last frame, and the window is showing: the preview can go.
                    gone.push(*h);
                    continue;
                }
                Stage::Settling { since } => {
                    // The preview holds still at its target until the window gets there.
                    if self.backend.arrived(*h, a.to) || now.saturating_duration_since(since) > SETTLE_MAX {
                        self.backend.unfade(*h, a.faded);
                        a.stage = Stage::Landing;
                        done.push((*h, a.after));
                    }
                    continue;
                }
                Stage::Gliding => {}
            }
            let (r, finished) = a.at(now);
            if r != a.shown || finished {
                self.backend.show(a.thumb, r, a.clip);
                a.shown = r;
            }
            if a.moves && a.after == After::Nothing && (finished || a.progress(now) >= MOVE_AT) {
                // The one move: invisible, while the preview finishes.
                self.backend.place(*h, a.to);
                a.moves = false;
                a.placed = true;
            }
            if !finished {
                continue;
            }
            match a.after {
                After::Hide => {
                    self.unfading.push((*h, a.faded, now));
                    gone.push(*h);
                    done.push((*h, a.after));
                }
                After::Nothing if a.placed => {
                    // Opaque once it's arrived; the preview holds still till then.
                    a.stage = Stage::Settling { since: now };
                    if self.backend.arrived(*h, a.to) {
                        self.backend.unfade(*h, a.faded);
                        a.stage = Stage::Landing;
                        done.push((*h, a.after));
                    }
                }
                After::Nothing => {
                    self.backend.unfade(*h, a.faded);
                    a.stage = Stage::Landing;
                    done.push((*h, a.after));
                }
            }
        }
        for h in gone {
            if let Some(a) = self.anims.remove(&h) {
                self.backend.drop_preview(a.thumb);
            }
        }
        // Windows that slid away: opaque again once they're hidden (or after a grace period).
        let backend = &mut self.backend;
        self.unfading.retain(|&(h, f, since)| {
            let gone = !backend.is_visible(h) || now.saturating_duration_since(since) > HIDE_GRACE;
            if gone {
                backend.unfade(h, f);
            }
            !gone
        });
        self.settle();
        done
    }

    /// Ends every animation now, each window where it was going (shutdown, animations turned
    /// off). Windows that slid away are returned for hiding; call `flush` after hiding them.
    pub fn finish_all(&mut self) -> Vec<(Hwnd, After)> {
        let mut done = Vec::new();
        for (h, a) in std::mem::take(&mut self.anims) {
            self.backend.drop_preview(a.thumb);
            if a.moves {
                self.backend.place(h, a.to);
            }
            if a.stage == Stage::Landing {
                continue;
            }
            match a.after {
                After::Nothing => self.backend.unfade(h, a.faded),
                After::Hide => self.unfading.push((h, a.faded, Instant::now())),
            }
            done.push((h, a.after));
        }
        self.settle();
        done
    }

    /// Makes every window still waiting to be hidden opaque now.
    pub fn flush(&mut self) {
        for (h, f, _) in std::mem::take(&mut self.unfading) {
            self.backend.unfade(h, f);
        }
        self.settle();
    }

    /// Closes the overlay and stops the clock once nothing is left to do.
    fn settle(&mut self) {
        if !self.busy() {
            self.backend.idle();
            if let Some(run) = self.clock.take() {
                run.store(false, Ordering::Relaxed);
            }
        }
    }

    fn ensure_clock(&mut self) {
        if self.clock.is_some() {
            return;
        }
        let run = Arc::new(AtomicBool::new(true));
        let flag = run.clone();
        let wake = self.wake;
        let spawned = std::thread::Builder::new().name("lattice-frames".into()).stack_size(64 * 1024).spawn(move || {
            while flag.load(Ordering::Relaxed) {
                // Blocks until DWM composes the next frame; sleep if it can't (no compositor).
                if unsafe { windows::Win32::Graphics::Dwm::DwmFlush() }.is_err() {
                    std::thread::sleep(Duration::from_millis(8));
                }
                wake();
            }
        });
        match spawned {
            Ok(_) => self.clock = Some(run),
            // No clock: the next event still ticks, and `finish_all` lands everything.
            Err(e) => log::warn!("animation clock: {e}"),
        }
    }
}

impl<B: Backend> Drop for Animator<B> {
    fn drop(&mut self) {
        self.finish_all();
        self.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all_curves() -> Vec<Bezier> {
        [AnimStyle::Fancy, AnimStyle::Fast].into_iter().flat_map(|s| [timing(s, Kind::Move).curve, timing(s, Kind::Slide).curve]).collect()
    }

    #[test]
    fn curves_start_at_zero_and_end_at_one() {
        for c in all_curves() {
            assert_eq!(c.at(0.0), 0.0);
            assert_eq!(c.at(1.0), 1.0);
            assert!(c.at(0.5) > 0.5, "{c:?} should be an ease-out");
        }
    }

    #[test]
    fn linear_bezier_is_identity() {
        let c = Bezier::new(0.25, 0.25, 0.75, 0.75);
        for i in 0..=20 {
            let x = i as f32 / 20.0;
            assert!((c.at(x) - x).abs() < 1e-3, "at {x}: {}", c.at(x));
        }
    }

    #[test]
    fn fancy_moves_overshoot_a_little_and_fast_never_does() {
        let peak = |c: Bezier| (0..=200).map(|i| c.at(i as f32 / 200.0)).fold(0.0f32, f32::max);
        let fancy = peak(timing(AnimStyle::Fancy, Kind::Move).curve);
        assert!(fancy > 1.005 && fancy < 1.06, "fancy peak {fancy}");
        assert!(peak(timing(AnimStyle::Fast, Kind::Move).curve) <= 1.0 + 1e-4);
        // Slides never overshoot: a window would show a gap past its tile.
        for s in [AnimStyle::Fancy, AnimStyle::Fast] {
            assert!(peak(timing(s, Kind::Slide).curve) <= 1.0 + 1e-4);
        }
    }

    #[test]
    fn fast_is_faster() {
        for k in [Kind::Move, Kind::Slide] {
            assert!(timing(AnimStyle::Fast, k).dur * 2 <= timing(AnimStyle::Fancy, k).dur);
        }
    }

    #[test]
    fn rect_interpolation() {
        let a = Rect::new(0, 0, 100, 50);
        let b = Rect::new(200, 100, 300, 150);
        assert_eq!(lerp_rect(a, b, 0.0), a);
        assert_eq!(lerp_rect(a, b, 1.0), b);
        assert_eq!(lerp_rect(a, b, 0.5), Rect::new(100, 50, 200, 100));
        // Overshooting a shrink never makes a window vanish.
        assert_eq!(lerp_rect(Rect::new(0, 0, 10, 10), Rect::new(0, 0, 1, 1), 1.5).w, 1);
    }

    #[test]
    fn previews_are_cut_to_the_clip_with_the_matching_source() {
        // A 1000x500 source drawn at 2x, half of it below the clip's bottom edge.
        let dest = Rect::new(0, 500, 2000, 1000);
        let clip = Rect::new(0, 0, 2000, 1000);
        assert_eq!(clip_preview(dest, Some(clip), (1000, 500)), Some((Rect::new(0, 500, 2000, 500), Rect::new(0, 0, 1000, 250))));
        // Entirely outside: nothing to draw.
        assert_eq!(clip_preview(dest.offset(0, 1000), Some(clip), (1000, 500)), None);
        // No clip: all of it, everywhere.
        assert_eq!(clip_preview(dest, None, (1000, 500)), Some((dest, Rect::new(0, 0, 1000, 500))));
    }

    /// Records what the animator asks of Windows.
    #[derive(Default)]
    struct Fake {
        log: Vec<String>,
        /// Windows that refuse to be made transparent.
        stubborn: Vec<Hwnd>,
        hidden: Vec<Hwnd>,
        /// Windows slow to get where they're put.
        slow: Vec<Hwnd>,
        next: Thumb,
    }

    impl Backend for Fake {
        fn fade(&mut self, h: Hwnd) -> Option<Faded> {
            self.log.push(format!("fade {h}"));
            (!self.stubborn.contains(&h)).then_some(Faded { ex: 0, attrs: None })
        }
        fn unfade(&mut self, h: Hwnd, _: Faded) {
            self.log.push(format!("unfade {h}"));
        }
        fn preview(&mut self, h: Hwnd) -> Option<Thumb> {
            self.next += 1;
            self.log.push(format!("preview {h}"));
            Some(self.next)
        }
        fn show(&mut self, t: Thumb, at: Rect, _: Option<Rect>) {
            self.log.push(format!("show {t} {} {}", at.x, at.y));
        }
        fn drop_preview(&mut self, t: Thumb) {
            self.log.push(format!("drop {t}"));
        }
        fn idle(&mut self) {
            self.log.push("idle".into());
        }
        fn is_visible(&self, h: Hwnd) -> bool {
            !self.hidden.contains(&h)
        }
        fn place(&mut self, h: Hwnd, r: Rect) {
            self.log.push(format!("place {h} {} {}", r.x, r.y));
        }
        fn arrived(&self, h: Hwnd, _: Rect) -> bool {
            !self.slow.contains(&h)
        }
    }

    fn animator() -> Animator<Fake> {
        Animator::with(Fake::default(), || {})
    }

    fn take(a: &mut Animator<Fake>) -> Vec<String> {
        std::mem::take(&mut a.backend.log)
    }

    const MS: Duration = Duration::from_millis(1);

    #[test]
    fn a_move_glides_the_preview_then_moves_the_window_once_then_shows_it() {
        let mut a = animator();
        let t = timing(AnimStyle::Fast, Kind::Move);
        let (from, to) = (Rect::new(0, 0, 100, 100), Rect::new(400, 0, 100, 100));
        assert!(a.glide(1, Glide::moving(from, to, t)));
        assert_eq!(take(&mut a), ["preview 1", "fade 1", "show 1 0 0"]);
        let t0 = a.anims[&1].start;
        assert!(a.tick(t0 + t.dur / 2).is_empty());
        let mid = take(&mut a);
        assert_eq!(mid.len(), 1, "only the preview moves mid-glide: {mid:?}");
        let x: i32 = mid[0].split(' ').nth(2).unwrap().parse().unwrap();
        assert!(x > 200 && x < 400, "ease-out is past halfway at half time: {x}");
        // Near the end, the window's one move, while the preview finishes.
        assert!(a.tick(t0 + t.dur.mul_f32(0.8)).is_empty());
        assert_eq!(take(&mut a).last().map(String::as_str), Some("place 1 400 0"));
        // Lands, and it's arrived: opaque; the preview goes a frame later.
        assert_eq!(a.tick(t0 + t.dur), vec![(1, After::Nothing)]);
        assert_eq!(take(&mut a), ["show 1 400 0", "unfade 1"]);
        assert!(a.busy());
        a.tick(t0 + t.dur + 8 * MS);
        assert_eq!(take(&mut a), ["drop 1", "idle"]);
        assert!(!a.busy());
    }

    #[test]
    fn a_landed_preview_waits_for_a_slow_window_but_not_forever() {
        let mut a = animator();
        a.backend.slow.push(1);
        let t = timing(AnimStyle::Fast, Kind::Move);
        a.glide(1, Glide::moving(Rect::new(0, 0, 9, 9), Rect::new(50, 0, 9, 9), t));
        let t0 = a.anims[&1].start;
        take(&mut a);
        assert!(a.tick(t0 + t.dur).is_empty());
        assert_eq!(take(&mut a), ["show 1 50 0", "place 1 50 0"]);
        assert!(a.tick(t0 + t.dur + 100 * MS).is_empty());
        assert!(take(&mut a).is_empty(), "still transparent, preview held");
        assert_eq!(a.tick(t0 + t.dur + SETTLE_MAX + MS), vec![(1, After::Nothing)]);
        assert_eq!(take(&mut a), ["unfade 1"]);
    }

    #[test]
    fn a_window_that_cant_fade_isnt_animated() {
        let mut a = animator();
        a.backend.stubborn.push(5);
        let t = timing(AnimStyle::Fancy, Kind::Move);
        assert!(!a.glide(5, Glide::moving(Rect::new(0, 0, 9, 9), Rect::new(50, 0, 9, 9), t)));
        assert_eq!(take(&mut a), ["preview 5", "fade 5", "drop 1", "idle"]);
        assert!(!a.busy());
    }

    #[test]
    fn retargeting_continues_from_where_the_preview_is_and_still_moves_once() {
        let mut a = animator();
        let t = timing(AnimStyle::Fancy, Kind::Move);
        a.glide(7, Glide::moving(Rect::new(0, 0, 10, 10), Rect::new(1000, 0, 10, 10), t));
        let t0 = a.anims[&7].start;
        a.tick(t0 + t.dur / 3);
        let here = a.current(7).unwrap();
        take(&mut a);
        // No second preview or fade, and the `from` passed is ignored: it's mid-flight.
        assert!(a.glide(7, Glide::moving(Rect::new(0, 0, 10, 10), Rect::new(0, 500, 10, 10), t)));
        assert!(take(&mut a).is_empty());
        assert_eq!(a.anims[&7].from, here);
        assert_eq!(a.target(7), Some(Rect::new(0, 500, 10, 10)));
        let t1 = a.anims[&7].start;
        a.tick(t1 + t.dur);
        let log = take(&mut a);
        assert_eq!(log.iter().filter(|l| l.starts_with("place")).collect::<Vec<_>>(), ["place 7 0 500"], "one move, to the latest target");
    }

    #[test]
    fn cancelling_a_move_still_puts_the_window_where_it_was_going() {
        let mut a = animator();
        let t = timing(AnimStyle::Fast, Kind::Move);
        a.glide(8, Glide::moving(Rect::new(0, 0, 9, 9), Rect::new(70, 0, 9, 9), t));
        take(&mut a);
        assert_eq!(a.cancel(8, false), Some(After::Nothing));
        assert_eq!(take(&mut a), ["drop 1", "place 8 70 0", "unfade 8", "idle"]);
    }

    #[test]
    fn a_window_that_slid_away_turns_opaque_only_once_hidden() {
        let mut a = animator();
        let t = timing(AnimStyle::Fast, Kind::Slide);
        let r = Rect::new(0, 0, 50, 50);
        a.glide(3, Glide::sliding(r, r.offset(0, -1080), t, None).hiding());
        assert!(a.is_hiding(3));
        let t0 = a.anims[&3].start;
        take(&mut a);
        assert_eq!(a.tick(t0 + t.dur), vec![(3, After::Hide)]);
        assert_eq!(take(&mut a), ["show 1 0 -1080", "drop 1"], "never moved");
        // Still visible (the hide hasn't landed): stays transparent.
        a.tick(t0 + t.dur + 10 * MS);
        assert!(take(&mut a).is_empty());
        a.backend.hidden.push(3);
        a.tick(t0 + t.dur + 20 * MS);
        assert_eq!(take(&mut a), ["unfade 3", "idle"]);
    }

    #[test]
    fn coming_back_mid_slide_reuses_the_fade_and_the_hide_is_called_off() {
        let mut a = animator();
        let t = timing(AnimStyle::Fancy, Kind::Slide);
        let r = Rect::new(0, 0, 50, 50);
        a.glide(4, Glide::sliding(r, r.offset(0, 1080), t, None).hiding());
        assert!(a.take_hide(4));
        assert!(!a.is_hiding(4));
        take(&mut a);
        assert!(a.glide(4, Glide::sliding(r.offset(0, 500), r, t, None)));
        assert!(take(&mut a).is_empty(), "same preview, same fade");
    }

    #[test]
    fn cancel_while_hiding_defers_the_unfade() {
        let mut a = animator();
        let t = timing(AnimStyle::Fast, Kind::Slide);
        let r = Rect::new(0, 0, 1, 1);
        a.glide(9, Glide::sliding(r, r.offset(10, 0), t, None));
        take(&mut a);
        assert_eq!(a.cancel(9, true), Some(After::Nothing));
        assert_eq!(take(&mut a), ["drop 1"]);
        a.unfade_now(9);
        assert_eq!(take(&mut a), ["unfade 9", "idle"]);
        assert_eq!(a.cancel(9, false), None);
    }

    #[test]
    fn delayed_start_holds_the_preview_where_it_starts() {
        let mut a = animator();
        let t = timing(AnimStyle::Fancy, Kind::Slide);
        let r = Rect::new(5, 5, 50, 50);
        a.glide(2, Glide::sliding(r.offset(0, 1080), r, t, None).delay(100 * MS));
        let t0 = a.anims[&2].start;
        take(&mut a);
        a.tick(t0 - 50 * MS);
        assert!(take(&mut a).is_empty(), "nothing moves before the delay is up");
        assert_eq!(a.finish_all(), vec![(2, After::Nothing)]);
        assert_eq!(take(&mut a), ["drop 1", "unfade 2", "idle"]);
    }
}
