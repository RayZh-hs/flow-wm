//! One rounded route is shared by rendering and connection cutting.
pub fn route(from: (f64, f64), to: (f64, f64), horizontal: bool) -> Vec<(f64, f64)> {
    let (a, b) = if horizontal {
        ((from.1, from.0), (to.1, to.0))
    } else {
        (from, to)
    };
    let lead = 42.0;
    let corners = if b.1 >= a.1 + lead * 2. {
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
