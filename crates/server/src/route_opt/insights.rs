//! Build human-readable route optimization insights (no LLM).

use chrono::{DateTime, Datelike, Timelike, Utc};
use serde_json::json;
use std::collections::HashMap;
use uuid::Uuid;

use super::stats::{
    aggregate_by_variant, aggregate_by_variant_context, best_variant_id, AggregateStats, ContextKey,
    VariantSample,
};

#[derive(Debug, Clone)]
pub struct OrsAltRef {
    pub preference: String,
    pub duration_secs: f64,
    pub distance_m: f64,
}

#[derive(Debug, Clone)]
pub struct InsightDraft {
    pub kind: String,
    pub title: String,
    pub body: String,
    pub score: f64,
    pub context: serde_json::Value,
}

fn fmt_mins(secs: f64) -> String {
    let m = (secs / 60.0).round().max(0.0) as i64;
    if m < 60 {
        format!("{m} min")
    } else {
        format!("{}h {}m", m / 60, m % 60)
    }
}

/// Build ranked insights for a corridor.
pub fn build_insights(
    variant_labels: &HashMap<Uuid, String>,
    samples: &[VariantSample],
    ors_alts: &[OrsAltRef],
    now: DateTime<Utc>,
    min_n: usize,
) -> Vec<InsightDraft> {
    let mut out = Vec::new();
    if samples.is_empty() || variant_labels.len() < 1 {
        return out;
    }

    let by_var = aggregate_by_variant(samples);
    let by_ctx = aggregate_by_variant_context(samples);

    // Overall prefer_variant when ≥2 variants with enough samples
    let ranked: Vec<(Uuid, &AggregateStats)> = by_var
        .iter()
        .filter(|(_, s)| s.n >= min_n)
        .map(|(id, s)| (*id, s))
        .collect();
    if ranked.len() >= 2 {
        let mut sorted = ranked;
        sorted.sort_by(|a, b| {
            a.1.median_duration_secs
                .partial_cmp(&b.1.median_duration_secs)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let (best_id, best) = sorted[0];
        let (worst_id, worst) = sorted[sorted.len() - 1];
        let saved = worst.median_duration_secs - best.median_duration_secs;
        if saved >= 60.0 {
            let bl = variant_labels
                .get(&best_id)
                .cloned()
                .unwrap_or_else(|| "best path".into());
            let wl = variant_labels
                .get(&worst_id)
                .cloned()
                .unwrap_or_else(|| "other path".into());
            out.push(InsightDraft {
                kind: "prefer_variant".into(),
                title: format!("{bl} is usually faster"),
                body: format!(
                    "Across similar trips, {bl} takes about {} (median, n={}) vs {} for {wl} (n={}). Choosing {bl} saves roughly {}.",
                    fmt_mins(best.median_duration_secs),
                    best.n,
                    fmt_mins(worst.median_duration_secs),
                    worst.n,
                    fmt_mins(saved)
                ),
                score: saved / 60.0,
                context: json!({
                    "best_variant_id": best_id,
                    "worst_variant_id": worst_id,
                    "saved_secs": saved,
                }),
            });
        }
    }

    // Time-of-day: current hour + weekday/weekend
    let hour = now.hour() as u8;
    let is_weekend = matches!(now.weekday(), chrono::Weekday::Sat | chrono::Weekday::Sun);
    let ctx = ContextKey {
        hour_bin: hour,
        is_weekend,
    };

    let mut ctx_ranked: Vec<(Uuid, AggregateStats)> = variant_labels
        .keys()
        .filter_map(|vid| {
            by_ctx
                .get(&(*vid, ctx))
                .filter(|s| s.n >= min_n)
                .map(|s| (*vid, s.clone()))
        })
        .collect();

    // Fallback: same weekend flag, any hour nearby ±1, or overall by weekend
    if ctx_ranked.len() < 2 {
        let mut near: HashMap<Uuid, Vec<f64>> = HashMap::new();
        for s in samples {
            if s.is_weekend != is_weekend {
                continue;
            }
            let dh = (s.hour_bin as i16 - hour as i16).abs();
            if dh <= 1 || dh >= 23 {
                near.entry(s.variant_id).or_default().push(s.duration_secs);
            }
        }
        ctx_ranked = near
            .into_iter()
            .filter(|(_, d)| d.len() >= min_n)
            .filter_map(|(vid, mut d)| {
                let med = super::stats::median(&mut d)?;
                Some((
                    vid,
                    AggregateStats {
                        n: d.len(),
                        median_duration_secs: med,
                        median_distance_m: 0.0,
                        median_stop_time_secs: 0.0,
                        median_elev_gain_m: None,
                    },
                ))
            })
            .collect();
    }

    if ctx_ranked.len() >= 2 {
        ctx_ranked.sort_by(|a, b| {
            a.1.median_duration_secs
                .partial_cmp(&b.1.median_duration_secs)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let (best_id, best) = &ctx_ranked[0];
        let (worst_id, worst) = &ctx_ranked[ctx_ranked.len() - 1];
        let saved = worst.median_duration_secs - best.median_duration_secs;
        if saved >= 45.0 {
            let bl = variant_labels
                .get(best_id)
                .cloned()
                .unwrap_or_else(|| "best path".into());
            let when = if is_weekend {
                format!("weekend around {hour:02}:00")
            } else {
                format!("weekdays around {hour:02}:00")
            };
            out.push(InsightDraft {
                kind: "avoid_variant_now".into(),
                title: format!("For {when}, prefer {bl}"),
                body: format!(
                    "At this time of day, {bl} has a median of {} (n={}). The slower option averages {}. Difference ~{}.",
                    fmt_mins(best.median_duration_secs),
                    best.n,
                    fmt_mins(worst.median_duration_secs),
                    fmt_mins(saved)
                ),
                score: saved / 60.0 + 0.5,
                context: json!({
                    "hour_bin": hour,
                    "is_weekend": is_weekend,
                    "best_variant_id": best_id,
                    "worst_variant_id": worst_id,
                    "saved_secs": saved,
                }),
            });
        }
    }

    // ORS reference vs best recorded overall
    if let Some(best_id) = best_variant_id(&by_var, 1) {
        if let Some(best) = by_var.get(&best_id) {
            if let Some(alt) = ors_alts
                .iter()
                .filter(|a| a.duration_secs.is_finite() && a.duration_secs > 0.0)
                .min_by(|a, b| {
                    a.duration_secs
                        .partial_cmp(&b.duration_secs)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
            {
                let delta = best.median_duration_secs - alt.duration_secs;
                // Only mention if router is meaningfully faster (freeflow estimate)
                if delta >= 120.0 && best.n >= min_n {
                    let bl = variant_labels
                        .get(&best_id)
                        .cloned()
                        .unwrap_or_else(|| "your usual path".into());
                    out.push(InsightDraft {
                        kind: "ors_reference".into(),
                        title: "Router suggests a faster line".into(),
                        body: format!(
                            "OpenRouteService ({}) estimates about {} for this corridor, while {bl} averages {} from your drives (n={}). Router times are freeflow estimates—not live traffic.",
                            alt.preference,
                            fmt_mins(alt.duration_secs),
                            fmt_mins(best.median_duration_secs),
                            best.n
                        ),
                        score: (delta / 60.0) * 0.4,
                        context: json!({
                            "ors_preference": alt.preference,
                            "ors_duration_secs": alt.duration_secs,
                            "best_variant_id": best_id,
                        }),
                    });
                }
            }
        }
    }

    out.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    out.truncate(8);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn prefer_faster_variant_insight() {
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let mut labels = HashMap::new();
        labels.insert(a, "Variant A".into());
        labels.insert(b, "Variant B".into());
        let mut samples = Vec::new();
        for _ in 0..4 {
            samples.push(VariantSample {
                variant_id: a,
                hour_bin: 18,
                is_weekend: false,
                month: 6,
                duration_secs: 600.0,
                distance_m: 12_000.0,
                stop_time_secs: 30.0,
                elev_gain_m: Some(40.0),
            });
            samples.push(VariantSample {
                variant_id: b,
                hour_bin: 18,
                is_weekend: false,
                month: 6,
                duration_secs: 900.0,
                distance_m: 11_000.0,
                stop_time_secs: 60.0,
                elev_gain_m: Some(20.0),
            });
        }
        let now = Utc.with_ymd_and_hms(2026, 6, 3, 18, 30, 0).unwrap(); // Wed
        let insights = build_insights(&labels, &samples, &[], now, 3);
        assert!(insights.iter().any(|i| i.kind == "prefer_variant"));
        assert!(insights.iter().any(|i| i.kind == "avoid_variant_now"));
    }
}
