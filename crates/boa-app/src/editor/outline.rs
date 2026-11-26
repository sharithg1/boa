//! The statement outline, painted by us over the editor rather than as a
//! gpui-component `RangeDecoration` frame. The editor clips its decorations
//! to the text area, which starts exactly at column 0, so a frame's left edge
//! landed under the caret (or was cut off). Ours sits 3 px left of the text,
//! in the gap after the gutter's divider, so the caret at the start of a
//! statement is inside it, and it steps line by line around the text like
//! the Tauri app's (lib/statements.ts `rangePath`).

use std::ops::Range;

use gpui::{
    Bounds, ContentMask, Entity, Hsla, IntoElement, PathBuilder, Pixels, Point, Styled as _,
    Window, canvas, point, px,
};
use gpui_component::input::EditorState;

/// Space between the outline's left edge and column 0.
const LEFT_PAD: Pixels = px(3.);
/// Space between the outline's right edge and the end of a line (`pad`).
const RIGHT_PAD: Pixels = px(3.);
/// How far left of the text area the outline may paint (12 px to the divider).
const CLIP_SLACK: Pixels = px(5.);

/// One laid-out line of the outlined range, in window coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Row {
    pub left: Pixels,
    pub right: Pixels,
    pub top: Pixels,
    pub bottom: Pixels,
}

/// The rows of `range` from the editor's last layout and the x where its text
/// area starts. `None` when nothing of it is laid out.
///
/// Each row spans its own text (lib/statements.ts `rangePath`): the first
/// from the statement's start, the others from column 0, each to where the
/// statement ends on it, and at least one cell wide (blank lines).
pub(crate) fn rows(state: &EditorState, range: &Range<usize>) -> Option<(Vec<Row>, Pixels)> {
    if range.is_empty() {
        return None;
    }
    let text_left = state.input_bounds().left() + state.text_origin_offset()?;
    let text = state.value();
    let mut rows: Vec<Row> = Vec::new();
    let mut hidden_above = false;
    let mut cell = None; // a cell's width, from a visible line with text
    let mut line_start = 0;
    for line in text.split('\n') {
        let line_end = line_start + line.len();
        let next = line_end + 1;
        if line_end < range.start {
            line_start = next;
            continue;
        }
        if line_start >= range.end {
            break;
        }
        let from = range.start.max(line_start);
        let to = range.end.min(line_end);
        match state.range_to_bounds(&(from..to)) {
            Some(b) => {
                if cell.is_none() && to > from {
                    let cols = text[from..to].chars().count() as f32;
                    cell = Some((b.right() - b.left()) / cols);
                }
                rows.push(Row {
                    left: b.left(),
                    right: b.right(),
                    top: b.top(),
                    bottom: b.bottom(),
                });
            }
            None => hidden_above |= rows.is_empty(),
        }
        line_start = next;
    }
    let first = *rows.first()?;
    let line_height = first.bottom - first.top;
    // `Math.max(col(b), x0 + 1)`: a blank line still gets one cell.
    let cell = cell.unwrap_or(line_height * 0.4);
    for row in &mut rows {
        row.right = row.right.max(row.left + cell);
    }
    // Lines scrolled out above: carry the shape past the top so the clip,
    // not a false top edge, ends it there. (Below needs nothing: the
    // bottom edge is then out of view already.)
    if hidden_above {
        rows[0].top -= line_height;
    }
    Some((rows, text_left))
}

/// The closed, stepped outline around consecutive rows (each padded).
pub(crate) fn outline_points(rows: &[Row]) -> Vec<Point<Pixels>> {
    let Some((first, last)) = rows.first().zip(rows.last()) else {
        return Vec::new();
    };
    let l = |r: &Row| r.left - LEFT_PAD;
    let r = |r: &Row| r.right + RIGHT_PAD;
    let mut points = Vec::with_capacity(rows.len() * 4 + 2);
    // Down the right side.
    points.push(point(l(first), first.top));
    points.push(point(r(first), first.top));
    for pair in rows.windows(2) {
        let (cur, next) = (&pair[0], &pair[1]);
        points.push(point(r(cur), cur.bottom));
        points.push(point(r(next), cur.bottom));
    }
    points.push(point(r(last), last.bottom));
    // Back up the left side.
    points.push(point(l(last), last.bottom));
    for pair in rows.windows(2).rev() {
        let (prev, cur) = (&pair[0], &pair[1]);
        points.push(point(l(cur), cur.top));
        points.push(point(l(prev), cur.top));
    }
    points.dedup();
    points
}

/// A 1 px stroke on whole device pixels (as the library snaps its frames).
fn snap(points: &[Point<Pixels>], scale: f32) -> (Pixels, Vec<Point<Pixels>>) {
    let physical = (scale - 0.5).ceil().max(1.);
    let center = if physical % 2. == 0. { 0. } else { 0.5 };
    let snap = |v: Pixels| px(((v.as_f32() * scale - center).round() + center) / scale);
    (
        px(physical / scale),
        points.iter().map(|p| p.map(snap)).collect(),
    )
}

/// An element over the editor that paints the outline of `range`: a frame in
/// `stroke` and, when given, a `fill` of the same shape.
pub(crate) fn outline(
    state: Entity<EditorState>,
    range: Range<usize>,
    stroke: Hsla,
    fill: Option<Hsla>,
) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |bounds, (), window: &mut Window, cx| {
            let Some((rows, text_left)) = rows(state.read(cx), &range) else {
                return;
            };
            let points = outline_points(&rows);
            let clip = Bounds::from_corners(
                point(text_left - CLIP_SLACK, bounds.top()),
                bounds.bottom_right(),
            );
            window.with_content_mask(Some(ContentMask { bounds: clip }), |window| {
                if let Some(fill) = fill {
                    let mut path = PathBuilder::fill();
                    path.add_polygon(&points, true);
                    if let Ok(path) = path.build() {
                        window.paint_path(path, fill);
                    }
                }
                let (width, points) = snap(&points, window.scale_factor());
                let mut path = PathBuilder::stroke(width);
                path.add_polygon(&points, true);
                if let Ok(path) = path.build() {
                    window.paint_path(path, stroke);
                }
            });
        },
    )
    .absolute()
    .inset_0()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(left: f32, right: f32, line: f32) -> Row {
        Row {
            left: px(left),
            right: px(right),
            top: px(line * 10.),
            bottom: px(line * 10. + 10.),
        }
    }

    #[test]
    fn one_line_is_a_padded_box() {
        let pts = outline_points(&[row(10., 50., 0.)]);
        let (l, r) = (10. - LEFT_PAD.as_f32(), 50. + RIGHT_PAD.as_f32());
        assert_eq!(
            pts,
            vec![
                point(px(l), px(0.)),
                point(px(r), px(0.)),
                point(px(r), px(10.)),
                point(px(l), px(10.)),
            ]
        );
    }

    #[test]
    fn lines_of_different_widths_step_on_both_sides() {
        // A statement starting mid-line, then a longer and a shorter line.
        let pts = outline_points(&[row(30., 50., 0.), row(10., 70., 1.), row(10., 40., 2.)]);
        let l = |x: f32| px(x - LEFT_PAD.as_f32());
        let r = |x: f32| px(x + RIGHT_PAD.as_f32());
        assert_eq!(
            pts,
            vec![
                point(l(30.), px(0.)),
                point(r(50.), px(0.)),
                point(r(50.), px(10.)),
                point(r(70.), px(10.)),
                point(r(70.), px(20.)),
                point(r(40.), px(20.)),
                point(r(40.), px(30.)),
                point(l(10.), px(30.)),
                point(l(10.), px(20.)),
                point(l(10.), px(10.)),
                point(l(30.), px(10.)),
            ]
        );
    }
}
