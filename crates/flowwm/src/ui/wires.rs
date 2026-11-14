//! One rounded route is shared by rendering and connection cutting.
pub fn route(from: (f64, f64), to: (f64, f64), horizontal: bool) -> Vec<(f64, f64)> {
    let (a, b) = if horizontal {
        ((from.1, from.0), (to.1, to.0))
    } else {
        (from, to)
    };
    let forward = b.1 - a.1;
    // A short forward gap still has room for a direct span. Reserve detours
    // for endpoints that actually run backwards or share the same flow axis.
    let lead = if forward > 0. {
        42.0_f64.min(forward / 2.)
    } else {
        42.0
    };
    let corners = if forward > 0. {
        vec![a, (a.0, a.1 + lead), (b.0, b.1 - lead), b]
    } else {
        let side = if (b.0 - a.0).abs() > 90. {
            (a.0 + b.0) / 2.
        } else {
            a.0.min(b.0) - 70.
        };
        vec![
            a,
            (a.0, a.1 + lead),
            (side, a.1 + lead),
            (side, b.1 - lead),
            (b.0, b.1 - lead),
            b,
        ]
    };
    let mut points = vec![a];
    for triple in corners.windows(3) {
        let (prev, corner, next) = (triple[0], triple[1], triple[2]);
        let d1 = (prev.0 - corner.0).hypot(prev.1 - corner.1);
        let d2 = (next.0 - corner.0).hypot(next.1 - corner.1);
        if d1 < 0.001 || d2 < 0.001 {
            points.push(corner);
            continue;
        }
        let r = 24.0_f64.min(d1 / 2.).min(d2 / 2.);
        let enter = (
            corner.0 + (prev.0 - corner.0) * r / d1,
            corner.1 + (prev.1 - corner.1) * r / d1,
        );
        let exit = (
            corner.0 + (next.0 - corner.0) * r / d2,
            corner.1 + (next.1 - corner.1) * r / d2,
        );
        points.push(enter);
        for i in 1..=12 {
            let t = i as f64 / 12.;
            let u = 1. - t;
            points.push((
                u * u * enter.0 + 2. * u * t * corner.0 + t * t * exit.0,
                u * u * enter.1 + 2. * u * t * corner.1 + t * t * exit.1,
            ));
        }
    }
    points.push(b);
    if horizontal {
        for p in &mut points {
            *p = (p.1, p.0);
        }
    }
    points
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forward_connections_stay_between_their_endpoints_at_any_distance() {
        for horizontal in [false, true] {
            for gap in [0.01, 1., 24., 48., 83., 84., 200.] {
                for offset in [-200., -4., 0., 4., 200.] {
                    let from = (120., 230.);
                    let to = if horizontal {
                        (from.0 + gap, from.1 + offset)
                    } else {
                        (from.0 + offset, from.1 + gap)
                    };
                    let points = route(from, to, horizontal);
                    assert_eq!(points.first(), Some(&from));
                    assert_eq!(points.last(), Some(&to));
                    for p in &points {
                        assert!(p.0 >= from.0.min(to.0) - 1e-9);
                        assert!(p.0 <= from.0.max(to.0) + 1e-9);
                        assert!(p.1 >= from.1.min(to.1) - 1e-9);
                        assert!(p.1 <= from.1.max(to.1) + 1e-9);
                    }
                    assert!(points.windows(2).all(|p| if horizontal {
                        p[0].0 <= p[1].0 + 1e-9
                    } else {
                        p[0].1 <= p[1].1 + 1e-9
                    }));
                }
            }
        }
    }

    #[test]
    fn short_aligned_connections_are_straight() {
        for horizontal in [false, true] {
            let from = (100., 200.);
            let to = if horizontal {
                (148., 200.)
            } else {
                (100., 248.)
            };
            assert!(route(from, to, horizontal).iter().all(|p| if horizontal {
                p.1 == from.1
            } else {
                p.0 == from.0
            }));
        }
    }

    #[test]
    fn backward_connections_keep_their_endpoint_clearance() {
        for horizontal in [false, true] {
            for delta in [-200., 0.] {
                let from = (100., 200.);
                let to = if horizontal {
                    (100. + delta, 200.)
                } else {
                    (100., 200. + delta)
                };
                let points = route(from, to, horizontal);
                assert_eq!(points.first(), Some(&from));
                assert_eq!(points.last(), Some(&to));
                assert!(points.iter().all(|p| p.0.is_finite() && p.1.is_finite()));
                assert!(points.iter().any(|p| if horizontal {
                    p.0 > from.0
                } else {
                    p.1 > from.1
                }));
                assert!(
                    points
                        .iter()
                        .any(|p| if horizontal { p.0 < to.0 } else { p.1 < to.1 })
                );
            }
        }
    }
}
