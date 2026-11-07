use serde::Serialize;
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{Arc, Mutex},
    time::Duration,
};

#[derive(Clone, Default)]
pub struct Performance(Arc<Mutex<BTreeMap<&'static str, Samples>>>);
#[derive(Default)]
struct Samples {
    count: u64,
    total_ms: f64,
    recent: VecDeque<f64>,
}
#[derive(Serialize)]
pub struct Summary {
    pub count: u64,
    pub mean_ms: f64,
    pub p50_ms: f64,
    pub p95_ms: f64,
    pub max_ms: f64,
}
impl Performance {
    pub fn record(&self, name: &'static str, duration: Duration) {
        let mut stages = self.0.lock().unwrap();
        let sample = stages.entry(name).or_default();
        let ms = duration.as_secs_f64() * 1000.0;
        sample.count += 1;
        sample.total_ms += ms;
        if sample.recent.len() == 120 {
            sample.recent.pop_front();
        }
        sample.recent.push_back(ms);
    }
    pub fn snapshot(&self) -> BTreeMap<&'static str, Summary> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .map(|(&name, s)| {
                let mut sorted: Vec<_> = s.recent.iter().copied().collect();
                sorted.sort_by(f64::total_cmp);
                (
                    name,
                    Summary {
                        count: s.count,
                        mean_ms: s.total_ms / s.count as f64,
                        p50_ms: sorted[sorted.len() / 2],
                        p95_ms: sorted[(sorted.len() * 95 / 100).min(sorted.len() - 1)],
                        max_ms: *sorted.last().unwrap(),
                    },
                )
            })
            .collect()
    }
    pub fn reset(&self) {
        self.0.lock().unwrap().clear();
    }
}
