use super::collapse::Rect;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// A retargetable fade always starts at its currently displayed value.
#[derive(Clone)]
pub struct Fade {
    from: f64,
    target: f64,
    started: Instant,
    duration: Duration,
}
impl Fade {
    pub fn new(from: f64, target: f64, started: Instant, duration: Duration) -> Self {
        Self {
            from,
            target,
            started,
            duration,
        }
    }
    pub fn value(&self, now: Instant) -> f64 {
        if self.duration.is_zero() {
            return self.target;
        }
        let t =
            now.saturating_duration_since(self.started).as_secs_f64() / self.duration.as_secs_f64();
        self.from + (self.target - self.from) * ease_out(t)
    }
    pub fn retarget(&mut self, target: f64, now: Instant, duration: Duration) {
        if target != self.target {
            *self = Self::new(self.value(now), target, now, duration);
        }
    }
    pub fn active(&self, now: Instant) -> bool {
        self.from != self.target && now.saturating_duration_since(self.started) < self.duration
    }
}

pub fn ease_out(t: f64) -> f64 {
    1. - (1. - t.clamp(0., 1.)).powi(3)
}

/// A shallow arc keeps large desktop-to-card transforms from feeling rigid.
pub fn transform(from: Rect, to: Rect, t: f64) -> Rect {
    let mut rect = from.interpolate(to, t);
    let distance = (to.x - from.x).hypot(to.y - from.y);
    rect.y -= distance.min(240.) * 0.2 * 4. * t * (1. - t);
    rect
}

/// Estimate release speed from the same pixel deltas used to move the camera.
/// GTK releases have used different units for ::decelerate despite the API docs.
#[derive(Default)]
pub struct ScrollHistory {
    samples: VecDeque<(f64, f64, f64)>,
    total: (f64, f64),
}
impl ScrollHistory {
    pub fn clear(&mut self) {
        self.samples.clear();
        self.total = (0., 0.);
    }
    pub fn push(&mut self, time: f64, dx: f64, dy: f64) {
        if !time.is_finite() || !dx.is_finite() || !dy.is_finite() {
            return;
        }
        self.total.0 += dx;
        self.total.1 += dy;
        self.samples.push_back((time, self.total.0, self.total.1));
        while self.samples.len() > 2 && self.samples.front().unwrap().0 < time - 0.080 {
            self.samples.pop_front();
        }
    }
    pub fn velocity(&self, now: f64) -> Option<(f64, f64)> {
        let first = self.samples.front()?;
        let last = self.samples.back()?;
        let dt = last.0 - first.0;
        if dt < 0.008 || now - last.0 > 0.080 {
            return None;
        }
        Some(((last.1 - first.1) / dt, (last.2 - first.2) / dt))
    }
}

pub fn coast(velocity: (f64, f64), dt: f64, decay: f64) -> ((f64, f64), (f64, f64)) {
    let remaining = (-decay * dt).exp();
    let travel = (1. - remaining) / decay;
    (
        (velocity.0 * travel, velocity.1 * travel),
        (velocity.0 * remaining, velocity.1 * remaining),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fades_preserve_unchanged_borders_and_retarget_without_a_jump() {
        let start = Instant::now();
        let duration = Duration::from_millis(200);
        for from in [0., 1.] {
            for to in [0., 1.] {
                let mut fade = Fade::new(from, to, start, duration);
                assert_eq!(fade.value(start), from);
                assert_eq!(fade.value(start + duration), to);
                for ms in [10, 50, 100, 150] {
                    let value = fade.value(start + Duration::from_millis(ms));
                    assert!((0. ..=1.).contains(&value));
                    if from == to {
                        assert_eq!(value, from);
                        assert!(!fade.active(start));
                    }
                }
                let middle = start + duration / 2;
                let shown = fade.value(middle);
                fade.retarget(from, middle, duration);
                assert_eq!(fade.value(middle), shown);
                assert_eq!(fade.value(middle + duration), from);
            }
        }
        let disabled = Fade::new(0., 1., start, Duration::ZERO);
        assert_eq!(disabled.value(start), 1.);
        assert!(!disabled.active(start));
    }
    #[test]
    fn finger_release_preserves_speed_and_stale_samples_do_not_fling() {
        let mut history = ScrollHistory::default();
        for i in 0..12 {
            history.push(i as f64 * 0.01, 2., -1.);
        }
        let speed = history.velocity(0.115).unwrap();
        assert!((speed.0 - 200.).abs() < 1e-9);
        assert!((speed.1 + 100.).abs() < 1e-9);
        let (travel, remaining) = coast(speed, 1. / 120., 10.);
        assert!(travel.0 > 0. && travel.0 < 200. / 120.);
        assert!(remaining.0 < speed.0);
        assert!(history.velocity(0.3).is_none());
        history.clear();
        history.push(1., 100., 10.);
        assert!(history.velocity(1.).is_none());
    }
    #[test]
    fn curved_transform_has_exact_endpoints_and_no_overshoot_in_size() {
        let from = Rect {
            x: 20.,
            y: 30.,
            width: 1000.,
            height: 800.,
        };
        let to = Rect {
            x: 600.,
            y: 400.,
            width: 220.,
            height: 180.,
        };
        assert_eq!(transform(from, to, 0.), from);
        assert_eq!(transform(from, to, 1.), to);
        let middle = transform(from, to, 0.5);
        assert!(middle.y < from.interpolate(to, 0.5).y);
        assert!(middle.width > to.width && middle.width < from.width);
    }
}
