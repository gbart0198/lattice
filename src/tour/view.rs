//! The tour's look: a dim over the whole screen with a glowing cut-out around the step's target,
//! and a card beside it. The cut-out is one rounded rectangle whose huge spread shadow is the
//! dim, so the hole, its corners and its glow move together on one spring from target to target.

use rune::{Color, Cx, El, Shadow, Spring, TextStyle, Transition, Tx, col, el, row, text};

use super::{CHEATS, Phase, R, STEPS, Tour, Wait};
use crate::shared::Anchor;

const CARD_W: f32 = 368.0;
const CARD_W_WIDE: f32 = 440.0;
const GAP: f32 = 16.0;
const MARGIN: f32 = 20.0;
/// Room around a target inside the spotlight.
const PAD: f32 = 6.0;

/// One spring for everything that travels between steps.
const TRAVEL: Spring = Spring::new(0.55, 0.12);
const FADE: Spring = Spring::new(0.35, 0.0);

fn spot_padding(anchor: Option<Anchor>) -> f32 {
    match anchor {
        // A whole window: hug its frame, where lattice's own glow sits.
        Some(Anchor::Focused) | Some(Anchor::Launcher) => 2.0,
        _ => PAD,
    }
}

/// Where the spotlight goes: around the target, or shrunk to a point under the card's middle
/// (an even dim) when there is none.
fn spot(s: &Tour) -> (R, bool) {
    match s.anchor_rect() {
        Some(r) => (r.inflate(spot_padding(s.current().anchor)), true),
        None => {
            let (x, y) = card_rect(s).center();
            (R::new(x, y, 0.0, 0.0), false)
        }
    }
}

fn card_width(s: &Tour) -> f32 {
    if s.step == 0 || s.step + 1 == STEPS.len() { CARD_W_WIDE } else { CARD_W }
}

/// Below the target when it fits (the bar's parts are at the top), else above it, else inside
/// it near its bottom (a window that fills the screen); centred when there's no target.
pub fn card_rect(s: &Tour) -> R {
    let (sw, sh) = (s.screen.w, s.screen.h);
    let w = card_width(s);
    let h = s.card_h;
    let clamp_x = |x: f32| x.clamp(MARGIN, (sw - w - MARGIN).max(MARGIN));
    let Some(t) = s.anchor_rect() else {
        return R::new(clamp_x((sw - w) / 2.0), ((sh - h) / 2.0 - sh * 0.04).max(MARGIN), w, h);
    };
    let t = t.inflate(spot_padding(s.current().anchor));
    let x = clamp_x(t.center().0 - w / 2.0);
    if t.bottom() + GAP + h + MARGIN <= sh {
        R::new(x, t.bottom() + GAP, w, h)
    } else if t.y - GAP - h >= MARGIN {
        R::new(x, t.y - GAP - h, w, h)
    } else {
        R::new(x, (t.bottom() - h - 40.0).max(MARGIN), w, h)
    }
}

pub fn view(s: &Tour, _cx: &mut Cx<Tour>) -> El {
    let p = &s.pal;
    let on = !matches!(s.phase, Phase::Entering | Phase::Leaving(_));
    let (hole, has_target) = spot(s);
    let radius = if has_target { (hole.h / 2.0).min(12.0) } else { 0.0 };

    let spotlight = el()
        .key("spot")
        .absolute(hole.x, hole.y)
        .size(hole.w, hole.h)
        .radius(radius)
        .motion(TRAVEL)
        .border(1.5, p.accent.alpha(if on && has_target { 0.95 } else { 0.0 }))
        // The dim: everything outside the hole.
        .box_shadow(Shadow { spread: 4000.0, color: Color::BLACK.alpha(if on { 0.52 } else { 0.0 }), ..Shadow::default() });

    // The glow: fading rings outside the hole. (A blurred shadow would also tint the inside of
    // the hole, which has no fill to cover it.)
    let mut layer = el().w_full().h_full().child(spotlight);
    for (i, (d, a)) in [(3.0_f32, 0.42_f32), (6.5, 0.2), (11.0, 0.09), (17.0, 0.035)].into_iter().enumerate() {
        let r = hole.inflate(d);
        layer = layer.child(
            el().key(format!("glow-{i}"))
                .absolute(r.x, r.y)
                .size(r.w, r.h)
                .radius(if has_target { radius + d } else { 0.0 })
                .motion(TRAVEL)
                .border(if i == 0 { 2.0 } else { 3.0 }, p.accent.alpha(if on && has_target { a } else { 0.0 })),
        );
    }

    let c = card_rect(s);
    let card = card(s)
        .key("card")
        .absolute(c.x, c.y)
        .w(c.w)
        .motion(TRAVEL)
        .opacity(if on { 1.0 } else { 0.0 })
        .offset(0.0, if on { 0.0 } else { 10.0 })
        .on_layout(|s: &mut Tour, [_, _, _, h]: [f32; 4]| {
            if (s.card_h - h).abs() > 0.5 {
                s.card_h = h;
            }
        });

    layer.child(card)
}

// ---------------------------------------------------------------------------------------------
// The card

fn card(s: &Tour) -> El {
    let p = &s.pal;
    let step = s.current();
    let last = s.step + 1 == STEPS.len();
    let card_fill = p.bg.mix(p.surface, 0.35);

    let header = row()
        .key("header")
        .center_y()
        .justify_between()
        .child(
            text(step.eyebrow.to_uppercase())
                .key(format!("eyebrow-{}", s.step))
                .font(TextStyle::new(11.0, 700.0).tracking(0.6))
                .color(p.accent),
        )
        .child(dots(s));

    let mut content = col()
        .key(format!("content-{}", s.step))
        .gap(10.0)
        .transition(Transition::offset(0.0, 6.0).combined(Transition::opacity()))
        .child(text(step.title).font(TextStyle::new(19.0, 680.0)).color(p.fg))
        .child(text(step.body).wrap().font(TextStyle::new(13.5, 420.0).lh(20.0)).color(p.fg.alpha(0.72)));

    if s.step == 0 {
        content = content.child(legend(s));
    } else if last {
        content = content.child(cheat_sheet(s));
    } else if !step.keys.is_empty() {
        content = content.child(shortcut(s));
    }

    let mut card = col()
        .hit()
        .role("dialog")
        .label(format!("lattice tour: {}", step.title))
        .padding(20.0)
        .gap(16.0)
        .radius(16.0)
        .fill(card_fill)
        .border_gradient(1.0, Color::WHITE.alpha(0.13), Color::WHITE.alpha(0.04))
        .box_shadow(Shadow { y: 24.0, blur: 64.0, color: Color::BLACK.alpha(0.5), ..Shadow::default() })
        .box_shadow(Shadow { y: 2.0, blur: 6.0, color: Color::BLACK.alpha(0.3), ..Shadow::default() });
    if s.step == 0 {
        card = card.child(mark(s));
    }
    card.child(header).child(content).child(footer(s))
}

/// Progress: a pill for this step, dots for the others.
fn dots(s: &Tour) -> El {
    let p = &s.pal;
    (0..STEPS.len()).fold(row().key("dots").gap(5.0).center_y(), |r, i| {
        let here = i == s.step;
        r.child(
            el().key(format!("dot-{i}"))
                .h(6.0)
                .w(if here { 18.0 } else { 6.0 })
                .radius(3.0)
                .motion(TRAVEL)
                .fill(if here {
                    p.accent
                } else if i < s.step {
                    p.accent.alpha(0.45)
                } else {
                    p.fg.alpha(0.18)
                }),
        )
    })
}

fn keycap(s: &Tour, k: &str) -> El {
    let p = &s.pal;
    row()
        .h(26.0)
        .min_w(26.0)
        .px(8.0)
        .center()
        .radius(7.0)
        .fill(p.surface.mix(Color::WHITE, 0.06))
        .border_gradient(1.0, Color::WHITE.alpha(0.16), Color::BLACK.alpha(0.35))
        .box_shadow(Shadow { y: 2.0, blur: 0.0, color: Color::BLACK.alpha(0.35), ..Shadow::default() })
        .child(text(k.to_string()).font(TextStyle::new(12.0, 650.0)).color(p.fg))
}

fn keys(s: &Tour, ks: &[&str]) -> El {
    ks.iter().enumerate().fold(row().gap(5.0).center_y(), |r, (i, k)| {
        let r = if i > 0 { r.child(text("+").font(TextStyle::new(11.0, 600.0)).color(s.pal.fg.alpha(0.35))) } else { r };
        r.child(keycap(s, k))
    })
}

/// The step's shortcut: "Try it" while the tour waits for it, then what happened.
fn shortcut(s: &Tour) -> El {
    let p = &s.pal;
    let step = s.current();
    if let Phase::Done(_) = s.phase {
        return row()
            .key("done")
            .gap(10.0)
            .center_y()
            .px(12.0)
            .h(42.0)
            .radius(10.0)
            .fill(p.accent.alpha(0.14))
            .transition(Transition::scale(0.96).combined(Transition::opacity()))
            .child(
                row().size(20.0, 20.0).center().radius(10.0).fill(p.accent).child(
                    text("\u{2713}").font(TextStyle::new(12.0, 800.0)).color(p.bg),
                ),
            )
            .child(text(step.done).wrap().shrink().font(TextStyle::new(13.0, 550.0)).color(p.fg));
    }
    let waits = s.live && step.wait != Wait::Next;
    // A word to type in the launcher ("workspaces") rather than keys to press.
    let typed = matches!(step.keys, [w] if w.chars().count() > 1 && w.chars().all(|c| c.is_ascii_lowercase()));
    row()
        .key("try")
        .gap(10.0)
        .center_y()
        .px(12.0)
        .h(42.0)
        .radius(10.0)
        .fill(p.fg.alpha(0.05))
        .border(1.0, if waits { p.accent.alpha(0.35) } else { p.fg.alpha(0.06) })
        .child(
            text(if waits { "TRY IT" } else if typed { "TYPE" } else { "SHORTCUT" })
                .font(TextStyle::new(10.0, 750.0))
                .color(if waits { p.accent } else { p.fg.alpha(0.45) }),
        )
        .child(keys(s, step.keys))
        .child(text(step.keys_label).truncate().shrink().font(TextStyle::new(12.5, 450.0)).color(p.fg.alpha(0.6)))
}

/// The first step's hint for getting around.
fn legend(s: &Tour) -> El {
    let p = &s.pal;
    let item = |k: &str, label: &str| row().gap(6.0).center_y().child(keycap(s, k)).child(
        text(label.to_string()).font(TextStyle::new(12.0, 450.0)).color(p.fg.alpha(0.55)),
    );
    let mut r = row().gap(16.0).center_y().pt(2.0);
    if !s.live {
        r = r.child(item("\u{2192}", "next")).child(item("\u{2190}", "back"));
    }
    r.child(item("Esc", "skip any time"))
}

fn cheat_sheet(s: &Tour) -> El {
    let p = &s.pal;
    CHEATS.iter().enumerate().fold(col().gap(2.0).pt(2.0), |c, (i, (ks, label))| {
        c.child(
            row()
                .key(format!("cheat-{i}"))
                .h(36.0)
                .px(10.0)
                .radius(8.0)
                .center_y()
                .justify_between()
                .fill(if i % 2 == 0 { p.fg.alpha(0.04) } else { Color::TRANSPARENT })
                .child(text(label.to_string()).font(TextStyle::new(13.0, 500.0)).color(p.fg.alpha(0.85)))
                .child(keys(s, ks)),
        )
    })
}

/// lattice's mark (the dwindle tiles) for the welcome card.
fn mark(s: &Tour) -> El {
    let p = &s.pal;
    let (w, h, g) = (34.0_f32, 26.0_f32, 3.0_f32);
    let hw = (w - g) / 2.0;
    let rx = hw + g;
    let th = (h - g) / 2.0;
    let bw = (w - rx - g) / 2.0;
    let tiles = [
        (0.0, 0.0, hw, h, 1.0),
        (rx, 0.0, w - rx, th, 0.75),
        (rx, th + g, bw, th, 0.55),
        (rx + bw + g, th + g, bw, th, 0.4),
    ];
    let glyph = tiles.into_iter().fold(el().size(w, h), |d, (x, y, tw, tht, a)| {
        d.child(el().absolute(x, y).size(tw, tht).radius(3.0).fill(p.accent.alpha(a)))
    });
    row()
        .key("mark")
        .size(56.0, 56.0)
        .center()
        .radius(14.0)
        .fill(p.accent.alpha(0.12))
        .border(1.0, p.accent.alpha(0.3))
        .box_shadow(Shadow { blur: 30.0, color: p.accent.alpha(0.35), ..Shadow::default() })
        .child(glyph)
}

fn footer(s: &Tour) -> El {
    let p = &s.pal;
    let first = s.step == 0;
    let last = s.step + 1 == STEPS.len();
    let mut right = row().gap(8.0).center_y();
    if !first && !last {
        right = right.child(button(s, "back", "Back", false, |s, tx| s.back(tx)));
    }
    let next_label = if first {
        "Take the tour"
    } else if last {
        "Done"
    } else if s.live && s.current().wait != Wait::Next && !matches!(s.phase, Phase::Done(_)) {
        "Skip step"
    } else {
        "Next"
    };
    // While a step waits for its keys, those are the call to action; skipping is quieter.
    let primary = next_label != "Skip step";
    right = right.child(button(s, "next", next_label, primary, |s, tx| s.next(tx)));
    let left: El = if last {
        text(String::new())
    } else {
        row()
            .key("skip")
            .role("button")
            .label("Skip the tour")
            .hit()
            .h(30.0)
            .px(4.0)
            .center_y()
            .cursor(rune::Cursor::Pointer)
            .on_press_tx(|s: &mut Tour, tx: &mut Tx| s.leave(tx))
            .child(text(if first { "Not now" } else { "Skip tour" }).font(TextStyle::new(12.5, 500.0)).color(p.fg.alpha(0.5)))
    };
    row().key("footer").center_y().justify_between().child(left).child(right)
}

fn button(s: &Tour, key: &str, label: &str, primary: bool, on: fn(&mut Tour, &mut Tx)) -> El {
    let p = &s.pal;
    let (fill, hover, ink) = if primary {
        (p.accent, p.accent.mix(Color::WHITE, 0.12), p.bg)
    } else {
        (p.fg.alpha(0.07), p.fg.alpha(0.12), p.fg)
    };
    row()
        .key(key.to_string())
        .role("button")
        .label(label.to_string())
        .hit()
        .h(32.0)
        .px(14.0)
        .center()
        .radius(9.0)
        .fill(fill)
        .hover_fill(hover)
        .press_fill(hover.mix(Color::BLACK, 0.1))
        .cursor(rune::Cursor::Pointer)
        .motion(FADE)
        .on_press_tx(move |s: &mut Tour, tx: &mut Tx| on(s, tx))
        .child(text(label.to_string()).font(TextStyle::new(13.0, 620.0)).color(ink))
}
