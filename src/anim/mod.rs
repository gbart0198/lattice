//! Window animations: moves, swaps, fullscreen and reflows glide to their new tile, and a
//! workspace switch slides one workspace out and the next one in.
//!
//! No process can draw another one's windows, so lattice animates the real windows: each frame
//! it moves every animating window one step with `SetWindowPos`, and DWM composites the result
//! as it does any move. That needs no GPU device, overlay or snapshot in this process: an idle
//! animator is an empty map (no allocation) and no frame clock thread exists. With animations
//! off, nothing here is ever touched.
//!
//! Frames: while something is moving, a small thread waits for each DWM composition
//! (`DwmFlush`, so steps land on vsync) and wakes the WM thread, which calls `tick`. It's capped
//! at about 144 Hz (apps that resize can't relayout faster, and a 240 Hz panel would only double
//! the work), and it exits as soon as the last animation ends.

pub mod preview;

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::config::AnimStyle;
use crate::layout::Rect;
use crate::win::Hwnd;

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
    /// Hide it (it slid off with its workspace), then put it back at `home` (its visible frame
    /// before the slide), so a hidden window always sits where it belongs.
    Hide { home: Rect },
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

fn lerp(a: i32, b: i32, k: f32) -> i32 {
    (a as f32 + (b - a) as f32 * k).round() as i32
}

/// The rect `k` of the way from `a` to `b` (k may overshoot); never smaller than 1x1.
pub fn lerp_rect(a: Rect, b: Rect, k: f32) -> Rect {
    Rect::new(lerp(a.x, b.x, k), lerp(a.y, b.y, k), lerp(a.w, b.w, k).max(1), lerp(a.h, b.h, k).max(1))
}

struct Anim {
    from: Rect,
    to: Rect,
    start: Instant,
    dur: Duration,
    curve: Bezier,
    /// Where the window was last put (what's on screen), for retargeting and skipping no-ops.
    shown: Rect,
    after: After,
}

impl Anim {
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

/// Frames are at least this far apart, whatever the refresh rate (about 144 Hz).
const MIN_FRAME: Duration = Duration::from_micros(6900);

pub struct Animator {
    anims: HashMap<Hwnd, Anim>,
    /// The running frame clock's keep-going flag; None when no clock thread exists.
    clock: Option<Arc<AtomicBool>>,
    /// Puts a window at a rect: `win::place` for real windows (the preview passes its own).
    place: fn(Hwnd, Rect),
    /// Wakes whoever calls `tick` (the WM thread); the preview ticks on its own and passes a no-op.
    wake: fn(),
}

impl Animator {
    pub fn new() -> Self {
        Self::with(crate::win::place, crate::shared::wake_wm)
    }

    pub fn with(place: fn(Hwnd, Rect), wake: fn()) -> Self {
        Self { anims: HashMap::new(), clock: None, place, wake }
    }

    pub fn busy(&self) -> bool {
        !self.anims.is_empty()
    }

    pub fn is_animating(&self, h: Hwnd) -> bool {
        self.anims.contains_key(&h)
    }

    /// Where an animating window is headed.
    pub fn target(&self, h: Hwnd) -> Option<Rect> {
        self.anims.get(&h).map(|a| a.to)
    }

    /// Where an animating window is right now.
    pub fn current(&self, h: Hwnd) -> Option<Rect> {
        self.anims.get(&h).map(|a| a.shown)
    }

    /// Animates `h` from `from` to `to`, starting after `delay`. A window that's already moving
    /// starts from where it is now instead, so a retarget mid-flight never jumps.
    pub fn start(&mut self, h: Hwnd, from: Rect, to: Rect, t: &Timing, delay: Duration, after: After) {
        let from = self.current(h).unwrap_or(from);
        let anim = Anim { from, to, start: Instant::now() + delay, dur: t.dur, curve: t.curve, shown: from, after };
        self.anims.insert(h, anim);
        self.ensure_clock();
    }

    /// Stops animating `h` where it is; returns what it was going to do at the end.
    pub fn cancel(&mut self, h: Hwnd) -> Option<After> {
        let a = self.anims.remove(&h)?;
        if self.anims.is_empty() {
            self.stop_clock();
        }
        Some(a.after)
    }

    /// Whether `h` is sliding away to be hidden.
    pub fn is_hiding(&self, h: Hwnd) -> bool {
        self.hiding_home(h).is_some()
    }

    /// Where a window sliding away to be hidden belongs (it goes back there once hidden).
    pub fn hiding_home(&self, h: Hwnd) -> Option<Rect> {
        match self.anims.get(&h)?.after {
            After::Hide { home } => Some(home),
            After::Nothing => None,
        }
    }

    /// Drops a pending end-of-slide hide (the window's workspace came back before it finished
    /// sliding away). True if there was one: the window is still on screen.
    pub fn take_hide(&mut self, h: Hwnd) -> bool {
        match self.anims.get_mut(&h) {
            Some(a) if matches!(a.after, After::Hide { .. }) => {
                a.after = After::Nothing;
                true
            }
            _ => false,
        }
    }

    /// Moves every animating window one frame on; returns the ones that finished, with what to
    /// do about them now.
    pub fn tick(&mut self, now: Instant) -> Vec<(Hwnd, After)> {
        let place = self.place;
        let mut done = Vec::new();
        self.anims.retain(|h, a| {
            let (r, finished) = a.at(now);
            if r != a.shown {
                place(*h, r);
                a.shown = r;
            }
            if finished {
                done.push((*h, a.after));
            }
            !finished
        });
        if self.anims.is_empty() {
            self.stop_clock();
        }
        done
    }

    /// Ends every animation now, each window at its destination (shutdown, animations turned off).
    pub fn finish_all(&mut self) -> Vec<(Hwnd, After)> {
        let place = self.place;
        let done = self
            .anims
            .drain()
            .map(|(h, a)| {
                if a.shown != a.to {
                    place(h, a.to);
                }
                (h, a.after)
            })
            .collect();
        self.stop_clock();
        done
    }

    fn ensure_clock(&mut self) {
        if self.clock.is_some() {
            return;
        }
        let run = Arc::new(AtomicBool::new(true));
        let flag = run.clone();
        let wake = self.wake;
        let spawned = std::thread::Builder::new().name("lattice-frames".into()).stack_size(64 * 1024).spawn(move || {
            let mut last = Instant::now() - MIN_FRAME;
            while flag.load(Ordering::Relaxed) {
                // Blocks until DWM composes the next frame; sleep if it can't (no compositor).
                if unsafe { windows::Win32::Graphics::Dwm::DwmFlush() }.is_err() {
                    std::thread::sleep(Duration::from_millis(8));
                }
                if last.elapsed() >= MIN_FRAME {
                    last = Instant::now();
                    wake();
                }
            }
        });
        match spawned {
            Ok(_) => self.clock = Some(run),
            // No clock: the next event still ticks, and `finish_all` lands everything.
            Err(e) => log::warn!("animation clock: {e}"),
        }
    }

    fn stop_clock(&mut self) {
        if let Some(run) = self.clock.take() {
            run.store(false, Ordering::Relaxed);
        }
    }
}

impl Drop for Animator {
    fn drop(&mut self) {
        self.stop_clock();
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

    // Tests run with a recording placer and no clock wakeups.
    thread_local! {
        static PLACED: std::cell::RefCell<Vec<(Hwnd, Rect)>> = const { std::cell::RefCell::new(Vec::new()) };
    }
    fn record(h: Hwnd, r: Rect) {
        PLACED.with(|p| p.borrow_mut().push((h, r)));
    }
    fn placed() -> Vec<(Hwnd, Rect)> {
        PLACED.with(|p| std::mem::take(&mut *p.borrow_mut()))
    }

    #[test]
    fn timeline_moves_then_finishes_at_the_target() {
        let mut a = Animator::with(record, || {});
        let t = timing(AnimStyle::Fast, Kind::Move);
        let (from, to) = (Rect::new(0, 0, 100, 100), Rect::new(400, 0, 100, 100));
        a.start(1, from, to, &t, Duration::ZERO, After::Nothing);
        assert_eq!(a.target(1), Some(to));
        let t0 = a.anims[&1].start;
        assert!(a.tick(t0 + t.dur / 2).is_empty());
        let mid = placed();
        assert_eq!(mid.len(), 1);
        assert!(mid[0].1.x > 200 && mid[0].1.x < 400, "ease-out is past halfway at half time: {:?}", mid[0].1);
        assert_eq!(a.tick(t0 + t.dur), vec![(1, After::Nothing)]);
        assert_eq!(placed(), vec![(1, to)]);
        assert!(!a.busy());
    }

    #[test]
    fn retargeting_continues_from_where_the_window_is() {
        let mut a = Animator::with(record, || {});
        let t = timing(AnimStyle::Fancy, Kind::Move);
        a.start(7, Rect::new(0, 0, 10, 10), Rect::new(1000, 0, 10, 10), &t, Duration::ZERO, After::Nothing);
        let t0 = a.anims[&7].start;
        a.tick(t0 + t.dur / 3);
        let here = a.current(7).unwrap();
        placed();
        // The `from` passed here is ignored: the window is mid-flight.
        a.start(7, Rect::new(0, 0, 10, 10), Rect::new(0, 500, 10, 10), &t, Duration::ZERO, After::Nothing);
        assert_eq!(a.anims[&7].from, here);
    }

    #[test]
    fn delayed_start_holds_the_window_and_take_hide_cancels_a_hide() {
        let mut a = Animator::with(record, || {});
        let t = timing(AnimStyle::Fancy, Kind::Slide);
        let home = Rect::new(5, 5, 50, 50);
        a.start(3, home, Rect::new(5, 905, 50, 50), &t, Duration::from_millis(100), After::Hide { home });
        let t0 = a.anims[&3].start;
        a.tick(t0 - Duration::from_millis(50));
        assert!(placed().is_empty(), "nothing moves before the delay is up");
        assert!(a.is_hiding(3));
        assert_eq!(a.hiding_home(3), Some(home));
        assert!(a.take_hide(3));
        assert!(!a.take_hide(3));
        assert!(!a.is_hiding(3));
        assert_eq!(a.finish_all(), vec![(3, After::Nothing)]);
        assert_eq!(placed(), vec![(3, Rect::new(5, 905, 50, 50))]);
    }

    #[test]
    fn cancel_reports_the_pending_action() {
        let mut a = Animator::with(record, || {});
        let t = timing(AnimStyle::Fast, Kind::Slide);
        let home = Rect::new(0, 0, 1, 1);
        a.start(9, home, home, &t, Duration::ZERO, After::Hide { home });
        assert_eq!(a.cancel(9), Some(After::Hide { home }));
        assert_eq!(a.cancel(9), None);
        assert!(!a.busy());
    }
}
