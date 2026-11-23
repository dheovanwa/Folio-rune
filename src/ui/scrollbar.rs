//! A scroll bar that is always there.
//!
//! `rune-kit`'s bar is an overlay: it draws over the viewport's right edge, fades out at rest,
//! and `attach` ORs the desktop's overlay-scroller preference into the style, so on a desktop
//! that prefers overlay scrollers it never shows until you touch it. A reading app wants the
//! opposite — a quiet, permanent track that says how much is left — so this draws its own: a
//! gutter beside the viewport, a faint track, and a thumb sized from the viewport's own scroll
//! report. Everything is a live value, so a scroll frame costs no view rebuild.

use rune::{Align, Color, El, Live, Phase, ScrollPos, col, el, row};

use crate::theme::Palette;

/// The gutter's width and the thumb's, in points. The thumb is centred in the gutter so the
/// track reads as a groove rather than a line against the window edge.
const GUTTER: f32 = 12.0;
const THUMB_W: f32 = 5.0;
/// A thumb never shrinks below this, so a very long article still leaves something to grab.
const MIN_THUMB: f32 = 32.0;

/// Everything the thumb's geometry is derived from.
///
/// The track's height and the viewport's come from different layout passes and arrive in either
/// order, so both write here and both call [`geometry`] — whichever lands second gets a
/// correctly sized thumb instead of a 1 pt stub.
#[derive(Clone)]
pub struct Bar {
    /// The track's height, reported by its own layout pass.
    pub track_h: Live<f32>,
    /// The viewport's visible height, from the scroll report.
    pub view_h: Live<f32>,
    /// The content's total height, from the scroll report.
    pub content_h: Live<f32>,
    /// The current scroll offset, from the scroll report.
    pub offset: Live<f32>,
    /// The scrollable range in points, which a drag maps onto.
    pub max: Live<f32>,
    /// The thumb's rect: `[x, y, w, h]`.
    pub thumb: Live<[f32; 4]>,
}

impl Default for Bar {
    fn default() -> Self {
        Bar {
            track_h: Live::new(0.0),
            view_h: Live::new(0.0),
            content_h: Live::new(0.0),
            offset: Live::new(0.0),
            max: Live::new(0.0),
            thumb: Live::new([(GUTTER - THUMB_W) / 2.0, 0.0, THUMB_W, MIN_THUMB]),
        }
    }
}

impl Bar {
    /// Take a viewport's scroll report.
    pub fn watch(&self, at: ScrollPos) {
        let view_h = at.viewport[1].max(1.0);
        self.view_h.set(view_h);
        self.content_h.set((at.max[1] + view_h).max(1.0));
        self.offset.set(at.offset[1]);
        self.max.set(at.max[1]);
        self.geometry();
    }

    /// Recompute the thumb from whatever is currently known. Cheap and idempotent, so both the
    /// scroll report and the track's layout can call it.
    pub fn geometry(&self) {
        let track = self.track_h.get();
        if track <= 0.0 {
            return;
        }
        let visible = (self.view_h.get() / self.content_h.get().max(1.0)).clamp(0.0, 1.0);
        let len = (track * visible).max(MIN_THUMB).min(track);
        let free = (track - len).max(0.0);
        let frac = (self.offset.get() / self.max.get().max(1.0)).clamp(0.0, 1.0);
        self.thumb
            .set([(GUTTER - THUMB_W) / 2.0, frac * free, THUMB_W, len]);
    }
}

/// Build the gutter: a faint track with a thumb over it.
///
/// `viewport_key` is the scrolling column the thumb drives; dragging it scrolls that column.
pub fn bar(p: Palette, viewport_key: &str, b: &Bar) -> El {
    let target = viewport_key.to_string();

    let (drag_thumb, drag_max, drag_track, drag_offset) =
        (b.thumb.clone(), b.max.clone(), b.track_h.clone(), b.offset.clone());
    let handle = el()
        .key("scroll-thumb")
        .absolute(0.0, 0.0)
        .live_rect(&b.thumb)
        .radius(THUMB_W / 2.0)
        .fill(p.meta.alpha(0.55))
        .hover(p.meta.alpha(0.30))
        .on_drag(move |_s: &mut crate::State, d: &mut rune::Drag, tx: &mut rune::Tx| {
            let (track, range) = (drag_track.get(), drag_max.get());
            if track <= 0.0 || range <= 0.0 {
                return;
            }
            // `d.y` is relative to the thumb, which itself moves as the article scrolls, so it
            // counts the travel twice. `d.dy` is raw pointer travel and is the only sane input.
            let free = (track - drag_thumb.get()[3].max(1.0)).max(1.0);
            if d.phase == Phase::Down {
                // Remember the offset the grab started from, so the thumb doesn't jump.
                d.data = drag_offset.get();
                return;
            }
            let to = (d.data + d.dy * range / free).clamp(0.0, range);
            tx.scroll_to_instant(&target, 0.0, to);
        });

    let layout = b.clone();
    col()
        .key("scroll-gutter")
        .w(GUTTER)
        .h_full()
        .fill(Color::TRANSPARENT)
        .a11y_hidden()
        .child(
            // The track is the faint well the thumb sits in. It reports its own height, then
            // asks for the thumb to be sized against it.
            el()
                .key("scroll-track")
                .w(THUMB_W)
                .h_full()
                .align(Align::Center)
                .radius(THUMB_W / 2.0)
                .fill(p.meta.alpha(0.12))
                .on_layout(move |_: &mut crate::State, r: [f32; 4]| {
                    layout.track_h.set(r[3]);
                    layout.geometry();
                }),
        )
        .child(handle)
}

/// Puts a scrolling viewport and its bar side by side, so the bar takes a gutter of its own
/// instead of covering the text.
pub fn with_bar(p: Palette, viewport_key: &str, b: &Bar, viewport: El) -> El {
    row()
        .key(format!("{viewport_key}-with-bar"))
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .child(viewport)
        .child(bar(p, viewport_key, b))
}
