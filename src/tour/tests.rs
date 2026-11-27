use super::*;
use crate::layout::Rect;
use crate::shared::{TourStatus, pack_rect, unpack_rect};

#[test]
fn rects_survive_the_trip_from_lattice() {
    for r in [Rect::new(0, 0, 1920, 30), Rect::new(-2560, -40, 800, 600), Rect::new(3840, 2160, 1, 1)] {
        assert_eq!(unpack_rect(pack_rect(r)), Some(r));
    }
    assert_eq!(unpack_rect(pack_rect(Rect::new(10, 10, 0, 5))), None);
}

#[test]
fn status_survives_the_trip_from_lattice() {
    let s = TourStatus { workspace: 7, layout: 2, launcher: true, escapes: 65535 };
    assert_eq!(TourStatus::unpack(s.pack()), s);
}

fn tour_at(step: usize, anchor: R) -> Tour {
    let pal = Palette { bg: Color::BLACK, fg: Color::WHITE, muted: Color::WHITE, accent: Color::WHITE, surface: Color::BLACK };
    let mut t = Tour::new(pal, Screen { x: 0, y: 0, scale: 1.0, w: 1600.0, h: 900.0 }, true);
    t.step = step;
    let a = STEPS[step].anchor.expect("step has an anchor");
    t.anchors[a as usize] = Some(anchor);
    t
}

#[test]
fn the_card_sits_below_a_bar_item_and_inside_a_full_window() {
    // The workspace pills at the top left: the card goes under them, kept on screen.
    let t = tour_at(2, R::new(4.0, 4.0, 200.0, 22.0));
    let c = view::card_rect(&t);
    assert!(c.y > 26.0 && c.x >= 20.0);
    // A window filling the screen: the card goes inside it, near its bottom.
    let t = tour_at(3, R::new(0.0, 30.0, 1600.0, 870.0));
    let c = view::card_rect(&t);
    assert!(c.y + c.h <= 900.0 && c.y > 400.0);
}

#[test]
fn launcher_steps_sit_between_opening_and_closing_it() {
    let open = STEPS.iter().position(|s| s.wait == Wait::LauncherOpen).expect("a step opens the launcher");
    let close = STEPS.iter().position(|s| s.wait == Wait::LauncherClosed).expect("a step closes it");
    assert!(open < close);
    for (i, s) in STEPS.iter().enumerate() {
        if s.anchor == Some(Anchor::Launcher) {
            assert!(open < i && i <= close, "step {i} ({}) points at the launcher outside it", s.title);
        }
    }
    for mode in ["Apps", "Files", "Commands", "Windows"] {
        assert!(STEPS[open..close].iter().any(|s| s.eyebrow == mode), "no {mode} step");
    }
}
