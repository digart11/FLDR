//! Shared silhouette, painter partition and pointer policy in logical pixels.
pub(crate) type Point = (f32, f32);
pub(crate) fn polygon(x: f32, y: f32, w: f32, h: f32, slant: f32) -> Vec<Point> {
    let s = slant.min(w * 0.2);
    vec![(x, y + h), (x + s, y), (x + w - s, y), (x + w, y + h)]
}
fn side(a: Point, b: Point, p: Point) -> f32 {
    (b.0 - a.0) * (p.1 - a.1) - (b.1 - a.1) * (p.0 - a.0)
}
fn has_area(poly: &[Point]) -> bool {
    let area: f64 = poly.iter().enumerate().map(|(i, p)| {
        let q = poly[(i + 1) % poly.len()];
        f64::from(p.0) * f64::from(q.1) - f64::from(p.1) * f64::from(q.0)
    }).sum();
    poly.len() >= 3 && area.abs() > 0.00001
}
pub(crate) fn contains(poly: &[Point], p: Point) -> bool {
    !poly.is_empty() && (0..poly.len()).all(|i| side(poly[i], poly[(i + 1) % poly.len()], p) >= 0.)
}
fn clip(poly: &[Point], a: Point, b: Point, inside: bool) -> Vec<Point> {
    let mut result = Vec::new();
    for i in 0..poly.len() {
        let p = poly[i];
        let q = poly[(i + 1) % poly.len()];
        let sp = side(a, b, p);
        let sq = side(a, b, q);
        let pin = (sp >= 0.) == inside;
        let qin = (sq >= 0.) == inside;
        if pin {
            result.push(p);
        }
        if pin != qin {
            let t = sp / (sp - sq);
            result.push((p.0 + (q.0 - p.0) * t, p.1 + (q.1 - p.1) * t));
        }
    }
    result
}
/// Partition outside a convex foreground silhouette. Glass receives one material
/// coat per surface instead of a dark background under its translucent tab.
pub(crate) fn outside(poly: Vec<Point>, foreground: &[Point]) -> Vec<Vec<Point>> {
    if !has_area(&poly) { return Vec::new(); }
    if !has_area(foreground) {
        return vec![poly];
    }
    let mut remaining = poly;
    let mut pieces = Vec::new();
    for i in 0..foreground.len() {
        let a = foreground[i];
        let b = foreground[(i + 1) % foreground.len()];
        let piece = clip(&remaining, a, b, false);
        // Clipping coincident edges can emit repeated/collinear vertices.
        // They own no pixels and must not become phantom material regions.
        if has_area(&piece) {
            pieces.push(piece);
        }
        remaining = clip(&remaining, a, b, true);
    }
    pieces
}

/// Back-to-front material ownership: the strip and every tab receive disjoint
/// pieces, including inactive/inactive overlaps. Empty silhouettes are ignored.
pub(crate) fn partition(bar: Vec<Point>, layers: &[Vec<Point>]) -> (Vec<Vec<Point>>, Vec<Vec<Vec<Point>>>) {
    let subtract = |poly: Vec<Point>, front: &[Vec<Point>]| {
        if !has_area(&poly) { return Vec::new(); }
        front.iter().fold(vec![poly], |pieces, shape| {
            pieces.into_iter().flat_map(|piece| outside(piece, shape)).collect::<Vec<_>>()
        })
    };
    let background = subtract(bar, layers);
    let tabs = layers.iter().enumerate().map(|(i, shape)| subtract(shape.clone(), &layers[i + 1..])).collect();
    (background, tabs)
}

/// Active surfaces stay connected to navigation even while the pointer moves.
pub(crate) fn surface_hover(active: bool, amount: f32) -> f32 {
    if active || !amount.is_finite() { 0.0 } else { amount.clamp(0.0, 1.0) }
}

/// Clip open top/side outlines too: an inactive outline must not show through
/// the translucent active tab or darken another inactive tab's interior.
pub(crate) fn visible_edges(poly: &[Point], front: &[Vec<Point>]) -> Vec<(Point, Point)> {
    let mut lines: Vec<_> = poly.windows(2).map(|edge| (edge[0], edge[1])).collect();
    for shape in front.iter().filter(|shape| has_area(shape)) {
        lines = lines.into_iter().flat_map(|(a, b)| {
            let mut enter = 0.0_f32;
            let mut leave = 1.0_f32;
            for i in 0..shape.len() {
                let s = side(shape[i], shape[(i + 1) % shape.len()], a);
                let delta = side(shape[i], shape[(i + 1) % shape.len()], b) - s;
                if delta.abs() < 0.00001 {
                    if s < 0.0 { return vec![(a, b)]; }
                } else if delta > 0.0 { enter = enter.max(-s / delta); }
                else { leave = leave.min(-s / delta); }
            }
            if enter >= leave { return vec![(a, b)]; }
            let at = |t: f32| (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t);
            let mut result = Vec::new();
            if enter > 0.0 { result.push((a, at(enter))); }
            if leave < 1.0 { result.push((at(leave), b)); }
            result
        }).collect();
    }
    lines
}

/// Inset outlines stay inside their own silhouette and outside every tab in
/// front. No bottom edge: the active surface remains joined to navigation.
pub(crate) fn border_pieces(poly: &[Point], front: &[Vec<Point>], thickness: f32) -> Vec<Vec<Point>> {
    if !has_area(poly) || thickness <= 0. { return Vec::new(); }
    let mut result = Vec::new();
    for edge in poly.windows(2) {
        let (a, b) = (edge[0], edge[1]);
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let length = dx.hypot(dy);
        if length <= f32::EPSILON { continue; }
        let (nx, ny) = (-dy / length * thickness, dx / length * thickness);
        let mut ribbon = vec![a, b, (b.0 + nx, b.1 + ny), (a.0 + nx, a.1 + ny)];
        for i in 0..poly.len() { ribbon = clip(&ribbon, poly[i], poly[(i + 1) % poly.len()], true); }
        let pieces = front.iter().fold(vec![ribbon], |pieces, shape| {
            pieces.into_iter().flat_map(|piece| outside(piece, shape)).collect()
        });
        result.extend(pieces.into_iter().filter(|piece| has_area(piece)));
    }
    result
}

/// One-physical-pixel exterior contour along the open top and angled sides.
/// The tab silhouette itself remains unchanged.
pub(crate) fn outer_border_pieces(
    poly: &[Point],
    front: &[Vec<Point>],
    thickness: f32,
) -> Vec<Vec<Point>> {
    if !has_area(poly) || thickness <= 0.0 {
        return Vec::new();
    }

    let mut result = Vec::new();
    for edge in poly.windows(2) {
        let (a, b) = (edge[0], edge[1]);
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let length = dx.hypot(dy);
        if length <= f32::EPSILON {
            continue;
        }

        // Outside is opposite the existing inward-facing ribbon normal.
        let (nx, ny) = (dy / length * thickness, -dx / length * thickness);
        let ribbon = vec![a, b, (b.0 + nx, b.1 + ny), (a.0 + nx, a.1 + ny)];

        let pieces = front.iter().fold(vec![ribbon], |pieces, shape| {
            pieces
                .into_iter()
                .flat_map(|piece| outside(piece, shape))
                .collect()
        });
        result.extend(pieces.into_iter().filter(|piece| has_area(piece)));
    }
    result
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn inset_borders_respect_foreground_at_windows_scalings() {
        for scale in [1., 1.25, 1.5] {
            let back = polygon(0., 4., 160., 36., 5.);
            let front = polygon(150., 4., 160., 36., 5.);
            let pieces = border_pieces(&back, &[front.clone()], 1. / scale);
            assert!(!pieces.is_empty());
            for piece in pieces {
                let center = piece.iter().fold((0., 0.), |sum, p| (sum.0 + p.0, sum.1 + p.1));
                let center = (center.0 / piece.len() as f32, center.1 / piece.len() as f32);
                assert!(contains(&back, center));
                assert!(!contains(&front, center));
            }
            assert!(border_pieces(&polygon(0., 0., 0., 36., 5.), &[], 1. / scale).is_empty());
        }
    }
    #[test]
    fn every_tab_position_has_one_material_coat_even_in_overlaps() {
        for zoom in [0.8, 1.0, 1.25, 2.0] {
            for width in [12.0, 80.0, 160.0] {
                for active in 0..3 {
                    let mut order = vec![0, 1, 2];
                    order.sort_by_key(|i| *i == active);
                    let layers: Vec<_> = order.iter().map(|i| polygon((10.0 + *i as f32 * (width - 8.0)) * zoom,
                        4.0 * zoom, width * zoom, 36.0 * zoom, 5.0 * zoom)).collect();
                    let (bar, tabs) = partition(vec![(0.,0.), (500. * zoom,0.), (500. * zoom,40. * zoom), (0.,40. * zoom)], &layers);
                    for x in 1..499 {
                        for y in 1..39 {
                            let p = ((x as f32 + 0.173) * zoom, (y as f32 + 0.219) * zoom);
                            let coats = bar.iter().filter(|piece| contains(piece, p)).count()
                                + tabs.iter().flatten().filter(|piece| contains(piece, p)).count();
                            assert_eq!(coats, 1, "active={active}, width={width}, zoom={zoom}, p={p:?}");
                            for (i, pieces) in tabs.iter().enumerate() {
                                let expected = layers.iter().rposition(|shape| contains(shape, p)) == Some(i);
                                assert_eq!(pieces.iter().any(|piece| contains(piece, p)), expected);
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn covered_inactive_outlines_cannot_tint_a_front_surface() {
        let back = polygon(0., 4., 160., 36., 5.);
        let front = polygon(152., 4., 160., 36., 5.);
        for (a, b) in visible_edges(&back, &[front.clone()]) {
            for t in [0.2, 0.5, 0.8] {
                assert!(!contains(&front, (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t)));
            }
        }
        assert_eq!(visible_edges(&front, &[]).len(), 3); // open bottom
    }

    #[test]
    fn active_material_is_unchanged_by_hover_and_empty_strip_is_fully_painted() {
        for amount in [0., 0.5, 1., f32::NAN] { assert_eq!(surface_hover(true, amount), 0.); }
        assert_eq!(surface_hover(false, 0.5), 0.5);
        let bar = vec![(0.,0.), (100.,0.), (100.,40.), (0.,40.)];
        let (pieces, tabs) = partition(bar.clone(), &[]);
        assert_eq!(pieces, [bar]);
        assert!(tabs.is_empty());
    }

    #[test]
    fn zero_width_closing_tabs_cannot_erase_or_recoat_the_strip() {
        let bar = vec![(0.,0.), (200.,0.), (200.,40.), (0.,40.)];
        let closing = polygon(50., 4., 0., 36., 5.);
        let (pieces, tabs) = partition(bar.clone(), &[closing.clone()]);
        assert_eq!(pieces, [bar]);
        assert!(tabs[0].is_empty());
        let front = polygon(0., 4., 160., 36., 5.);
        assert_eq!(visible_edges(&front, &[closing]), visible_edges(&front, &[]));
    }

    #[test]
    fn slanted_corners_and_overlap_share_the_visible_boundary() {
        let a = polygon(0., 0., 160., 32., 10.);
        let b = polygon(152., 0., 160., 32., 10.);
        assert!(!contains(&a, (1., 1.)));
        assert!(contains(&a, (156., 31.)) && contains(&b, (156., 31.)));
        assert!(!contains(&a, (156., 1.)));
        assert!(contains(&polygon(0., 0., 12., 32., 10.), (6., 1.)));
    }
    #[test]
    fn background_partition_excludes_foreground_interior() {
        let active = polygon(50., 4., 100., 36., 8.);
        let pieces = outside(vec![(0., 0.), (200., 0.), (200., 40.), (0., 40.)], &active);
        for x in 1..200 {
            for y in 1..40 {
                let p = (x as f32 + 0.13, y as f32 + 0.17);
                let coats = pieces.iter().filter(|poly| contains(poly, p)).count();
                assert_eq!(coats, usize::from(!contains(&active, p)));
            }
        }
    }
}

pub(crate) fn active_child<T: Copy + PartialEq>(children: &[T], active: Option<T>, focused: T) -> Option<T> {
    if children.contains(&focused) { Some(focused) } else { active.filter(|id| children.contains(id)).or_else(|| children.first().copied()) }
}
