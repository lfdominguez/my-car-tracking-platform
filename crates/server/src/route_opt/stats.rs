//! Aggregate trip samples for corridor / variant comparison.

use serde::Serialize;
use std::collections::HashMap;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct VariantSample {
    pub variant_id: Uuid,
    pub hour_bin: u8,
    pub is_weekend: bool,
    pub month: u8,
    pub duration_secs: f64,
    pub distance_m: f64,
    pub stop_time_secs: f64,
    pub elev_gain_m: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ContextKey {
    pub hour_bin: u8,
    pub is_weekend: bool,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct AggregateStats {
    pub n: usize,
    pub median_duration_secs: f64,
    pub median_distance_m: f64,
    pub median_stop_time_secs: f64,
    pub median_elev_gain_m: Option<f64>,
}

pub fn median(xs: &mut [f64]) -> Option<f64> {
    if xs.is_empty() {
        return None;
    }
    xs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mid = xs.len() / 2;
    if xs.len() % 2 == 1 {
        Some(xs[mid])
    } else {
        Some((xs[mid - 1] + xs[mid]) / 2.0)
    }
}

pub fn aggregate_samples(samples: &[VariantSample]) -> AggregateStats {
    let mut durs: Vec<f64> = samples.iter().map(|s| s.duration_secs).collect();
    let mut dists: Vec<f64> = samples.iter().map(|s| s.distance_m).collect();
    let mut stops: Vec<f64> = samples.iter().map(|s| s.stop_time_secs).collect();
    let mut elevs: Vec<f64> = samples.iter().filter_map(|s| s.elev_gain_m).collect();
    AggregateStats {
        n: samples.len(),
        median_duration_secs: median(&mut durs).unwrap_or(0.0),
        median_distance_m: median(&mut dists).unwrap_or(0.0),
        median_stop_time_secs: median(&mut stops).unwrap_or(0.0),
        median_elev_gain_m: median(&mut elevs),
    }
}

/// Group by variant + hour + weekend.
pub fn aggregate_by_variant_context(
    samples: &[VariantSample],
) -> HashMap<(Uuid, ContextKey), AggregateStats> {
    let mut groups: HashMap<(Uuid, ContextKey), Vec<&VariantSample>> = HashMap::new();
    for s in samples {
        let key = (
            s.variant_id,
            ContextKey {
                hour_bin: s.hour_bin,
                is_weekend: s.is_weekend,
            },
        );
        groups.entry(key).or_default().push(s);
    }
    groups
        .into_iter()
        .map(|(k, vs)| {
            let owned: Vec<VariantSample> = vs.into_iter().cloned().collect();
            (k, aggregate_samples(&owned))
        })
        .collect()
}

pub fn aggregate_by_variant(samples: &[VariantSample]) -> HashMap<Uuid, AggregateStats> {
    let mut groups: HashMap<Uuid, Vec<&VariantSample>> = HashMap::new();
    for s in samples {
        groups.entry(s.variant_id).or_default().push(s);
    }
    groups
        .into_iter()
        .map(|(k, vs)| {
            let owned: Vec<VariantSample> = vs.into_iter().cloned().collect();
            (k, aggregate_samples(&owned))
        })
        .collect()
}

/// Best variant id by lowest median duration among those with n >= min_n.
pub fn best_variant_id(by_variant: &HashMap<Uuid, AggregateStats>, min_n: usize) -> Option<Uuid> {
    by_variant
        .iter()
        .filter(|(_, s)| s.n >= min_n)
        .min_by(|a, b| {
            a.1.median_duration_secs
                .partial_cmp(&b.1.median_duration_secs)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|(id, _)| *id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(vid: Uuid, hour: u8, weekend: bool, dur: f64) -> VariantSample {
        VariantSample {
            variant_id: vid,
            hour_bin: hour,
            is_weekend: weekend,
            month: 3,
            duration_secs: dur,
            distance_m: 10_000.0,
            stop_time_secs: 0.0,
            elev_gain_m: None,
        }
    }

    #[test]
    fn median_even_odd() {
        assert_eq!(median(&mut [3.0, 1.0, 2.0]), Some(2.0));
        assert_eq!(median(&mut [4.0, 1.0, 2.0, 3.0]), Some(2.5));
    }

    #[test]
    fn best_variant_picks_faster() {
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let samples = vec![
            sample(a, 18, false, 600.0),
            sample(a, 18, false, 620.0),
            sample(a, 18, false, 610.0),
            sample(b, 18, false, 900.0),
            sample(b, 18, false, 880.0),
            sample(b, 18, false, 910.0),
        ];
        let agg = aggregate_by_variant(&samples);
        assert_eq!(best_variant_id(&agg, 3), Some(a));
    }
}
