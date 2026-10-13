//! 2D convex polygons in the build plate plane (Cura's `Polygon`: x, z of the scene).

pub type P2 = [f64; 2];

/// Andrew's monotone chain convex hull (counter-clockwise, no duplicate end point).
pub fn convex_hull(points: &[P2]) -> Vec<P2> {
    let mut pts: Vec<P2> = points.iter().copied().filter(|p| p[0].is_finite() && p[1].is_finite()).collect();
    pts.sort_by(|a, b| a.partial_cmp(b).unwrap());
    pts.dedup();
    if pts.len() < 3 {
        return pts;
    }
    let cross = |o: P2, a: P2, b: P2| (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0]);
    let mut lower: Vec<P2> = Vec::new();
    for &p in &pts {
        while lower.len() >= 2 && cross(lower[lower.len() - 2], lower[lower.len() - 1], p) <= 0.0 {
            lower.pop();
        }
        lower.push(p);
    }
    let mut upper: Vec<P2> = Vec::new();
    for &p in pts.iter().rev() {
        while upper.len() >= 2 && cross(upper[upper.len() - 2], upper[upper.len() - 1], p) <= 0.0 {
            upper.pop();
        }
        upper.push(p);
    }
    lower.pop();
    upper.pop();
    lower.extend(upper);
    lower
}

/// Minkowski sum of two convex polygons (`Polygon.getMinkowskiHull`).
pub fn minkowski(a: &[P2], b: &[P2]) -> Vec<P2> {
    if a.is_empty() {
        return b.to_vec();
    }
    if b.is_empty() {
        return a.to_vec();
    }
    let mut pts = Vec::with_capacity(a.len() * b.len());
    for p in a {
        for q in b {
            pts.push([p[0] + q[0], p[1] + q[1]]);
        }
    }
    convex_hull(&pts)
}

/// `Polygon.approximatedCircle(radius)`: an 8-gon around the origin.
pub fn approximated_circle(radius: f64) -> Vec<P2> {
    let step = std::f64::consts::PI / 4.0;
    (0..8).map(|i| [radius * (i as f64 * step).cos(), radius * (i as f64 * step).sin()]).collect()
}

/// Separating axis test: do two convex polygons overlap (more than touching)?
pub fn intersects(a: &[P2], b: &[P2]) -> bool {
    if a.len() < 3 || b.len() < 3 {
        return false;
    }
    for poly in [a, b] {
        for i in 0..poly.len() {
            let p = poly[i];
            let q = poly[(i + 1) % poly.len()];
            let axis = [-(q[1] - p[1]), q[0] - p[0]];
            let proj = |s: &[P2]| {
                let mut lo = f64::INFINITY;
                let mut hi = f64::NEG_INFINITY;
                for v in s {
                    let d = v[0] * axis[0] + v[1] * axis[1];
                    lo = lo.min(d);
                    hi = hi.max(d);
                }
                (lo, hi)
            };
            let (a0, a1) = proj(a);
            let (b0, b1) = proj(b);
            let len = (axis[0] * axis[0] + axis[1] * axis[1]).sqrt().max(1e-12);
            if (a1.min(b1) - a0.max(b0)) / len <= 1e-6 {
                return false;
            }
        }
    }
    true
}

