//! Build human-readable route optimization insights (no LLM).
//!
//! Produces both **strong** comparative insights (multi-variant, enough samples)
//! and **soft** baseline insights so a corridor is never a blank "Insights" panel
//! once it has at least one finished trip.

use chrono::{DateTime, Datelike, Timelike, Utc};
use serde_json::json;
use std::collections::HashMap;
use uuid::Uuid;

use super::stats::{
    aggregate_by_variant, aggregate_by_variant_context, aggregate_samples, best_variant_id,
    AggregateStats, ContextKey, VariantSample,
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

fn fmt_km(distance_m: f64) -> String {
    if distance_m >= 1000.0 {
        format!("{:.1} km", distance_m / 1000.0)
    } else {
        format!("{:.0} m", distance_m.max(0.0))
    }
}

fn label_of(labels: &HashMap<Uuid, String>, id: Uuid, fallback: &str) -> String {
    labels
        .get(&id)
        .cloned()
        .unwrap_or_else(|| fallback.to_string())
}

/// Build ranked insights for a corridor.
///
/// `min_n` is the sample threshold for **strong** comparative insights
/// (prefer / avoid). Soft baseline cards still appear with fewer trips.
pub fn build_insights(
    variant_labels: &HashMap<Uuid, String>,
    samples: &[VariantSample],
    ors_alts: &[OrsAltRef],
    now: DateTime<Utc>,
    min_n: usize,
) -> Vec<InsightDraft> {
    let mut out = Vec::new();
    if samples.is_empty() || variant_labels.is_empty() {
        return out;
    }

    let by_var = aggregate_by_variant(samples);
    let by_ctx = aggregate_by_variant_context(samples);
    let overall = aggregate_samples(samples);
    let soft_n = 1.max(min_n.saturating_sub(1)); // e.g. min_n=3 → soft_n=2

    // —— Soft: corridor baseline (always when any samples) ——
    if overall.n >= 1 {
        let best_id = best_variant_id(&by_var, 1);
        let path = best_id
            .map(|id| label_of(variant_labels, id, "your usual path"))
            .unwrap_or_else(|| "this corridor".into());
        let variant_n = by_var.len();
        let title = if overall.n < min_n {
            format!("Forming baseline · {}", fmt_mins(overall.median_duration_secs))
        } else {
            format!("Typical drive · {}", fmt_mins(overall.median_duration_secs))
        };
        let more = (min_n as i32 - overall.n as i32).max(0);
        let body = if overall.n < min_n {
            format!(
                "Median {} over {} across {} trip{} (mostly via {path}). \
                 About {more} more finished trip{} on this OD will unlock stronger comparisons.",
                fmt_mins(overall.median_duration_secs),
                fmt_km(overall.median_distance_m),
                overall.n,
                if overall.n == 1 { "" } else { "s" },
                if more == 1 { "" } else { "s" },
            )
        } else {
            format!(
                "Across {} trips and {variant_n} path variant{}, median time is {} covering {}. \
                 Primary path: {path}.",
                overall.n,
                if variant_n == 1 { "" } else { "s" },
                fmt_mins(overall.median_duration_secs),
                fmt_km(overall.median_distance_m),
            )
        };
        out.push(InsightDraft {
            kind: if overall.n < min_n {
                "forming".into()
            } else {
                "typical_pace".into()
            },
            title,
            body,
            score: 0.35 + (overall.n as f64 * 0.05).min(1.0),
            context: json!({
                "n": overall.n,
                "median_duration_secs": overall.median_duration_secs,
                "median_distance_m": overall.median_distance_m,
                "variant_count": variant_n,
            }),
        });
    }

    // —— Soft: single path only ——
    if by_var.len() == 1 && overall.n >= 1 {
        if let Some((vid, stats)) = by_var.iter().next() {
            let bl = label_of(variant_labels, *vid, "this path");
            out.push(InsightDraft {
                kind: "single_path".into(),
                title: format!("One recorded path · {bl}"),
                body: format!(
                    "Every trip so far used {bl} (n={}, median {}). \
                     Drive an alternate line on the same origin→destination to compare which is faster by time of day.",
                    stats.n,
                    fmt_mins(stats.median_duration_secs)
                ),
                score: 0.55,
                context: json!({ "variant_id": vid, "n": stats.n }),
            });
        }
    }

    // —— Strong (+ soft) prefer_variant when ≥2 variants ——
    let ranked_strong: Vec<(Uuid, &AggregateStats)> = by_var
        .iter()
        .filter(|(_, s)| s.n >= min_n)
        .map(|(id, s)| (*id, s))
        .collect();
    let ranked_soft: Vec<(Uuid, &AggregateStats)> = by_var
        .iter()
        .filter(|(_, s)| s.n >= soft_n)
        .map(|(id, s)| (*id, s))
        .collect();

    let push_prefer = |out: &mut Vec<InsightDraft>,
                       ranked: Vec<(Uuid, &AggregateStats)>,
                       min_saved: f64,
                       kind: &str,
                       score_scale: f64| {
        if ranked.len() < 2 {
            return;
        }
        let mut sorted = ranked;
        sorted.sort_by(|a, b| {
            a.1.median_duration_secs
                .partial_cmp(&b.1.median_duration_secs)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let (best_id, best) = sorted[0];
        let (worst_id, worst) = sorted[sorted.len() - 1];
        let saved = worst.median_duration_secs - best.median_duration_secs;
        if saved < min_saved {
            return;
        }
        let bl = label_of(variant_labels, best_id, "best path");
        let wl = label_of(variant_labels, worst_id, "other path");
        let hedge = if kind == "prefer_variant_soft" {
            "Early signal — "
        } else {
            ""
        };
        out.push(InsightDraft {
            kind: kind.into(),
            title: format!("{bl} is usually faster"),
            body: format!(
                "{hedge}Across similar trips, {bl} takes about {} (median, n={}) vs {} for {wl} (n={}). Choosing {bl} saves roughly {}.",
                fmt_mins(best.median_duration_secs),
                best.n,
                fmt_mins(worst.median_duration_secs),
                worst.n,
                fmt_mins(saved)
            ),
            score: (saved / 60.0) * score_scale,
            context: json!({
                "best_variant_id": best_id,
                "worst_variant_id": worst_id,
                "saved_secs": saved,
                "soft": kind.ends_with("_soft"),
            }),
        });
    };

    push_prefer(
        &mut out,
        ranked_strong,
        60.0,
        "prefer_variant",
        1.0,
    );
    // Soft comparison only if strong did not already fire
    if !out.iter().any(|i| i.kind == "prefer_variant") {
        push_prefer(
            &mut out,
            ranked_soft,
            45.0,
            "prefer_variant_soft",
            0.65,
        );
    }

    // —— Time-of-day: current hour + weekday/weekend ——
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
                .filter(|s| s.n >= soft_n)
                .map(|s| (*vid, s.clone()))
        })
        .collect();

    // Fallback: same weekend flag, hour nearby ±1
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
            .filter(|(_, d)| d.len() >= soft_n)
            .filter_map(|(vid, mut d)| {
                let n = d.len();
                let med = super::stats::median(&mut d)?;
                Some((
                    vid,
                    AggregateStats {
                        n,
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
        let strong_enough = best.n >= min_n && worst.n >= min_n && saved >= 45.0;
        let soft_enough = saved >= 45.0;
        if strong_enough || soft_enough {
            let bl = label_of(variant_labels, *best_id, "best path");
            let when = if is_weekend {
                format!("weekend around {hour:02}:00")
            } else {
                format!("weekdays around {hour:02}:00")
            };
            out.push(InsightDraft {
                kind: if strong_enough {
                    "avoid_variant_now".into()
                } else {
                    "avoid_variant_now_soft".into()
                },
                title: format!("For {when}, prefer {bl}"),
                body: format!(
                    "At this time of day, {bl} has a median of {} (n={}). The slower option averages {}. Difference ~{}.",
                    fmt_mins(best.median_duration_secs),
                    best.n,
                    fmt_mins(worst.median_duration_secs),
                    fmt_mins(saved)
                ),
                score: saved / 60.0 + if strong_enough { 0.5 } else { 0.2 },
                context: json!({
                    "hour_bin": hour,
                    "is_weekend": is_weekend,
                    "best_variant_id": best_id,
                    "worst_variant_id": worst_id,
                    "saved_secs": saved,
                }),
            });
        }
    } else if ctx_ranked.len() == 1 && overall.n >= soft_n {
        // Single variant with time-local samples — still useful
        let (vid, stats) = &ctx_ranked[0];
        let bl = label_of(variant_labels, *vid, "your path");
        let when = if is_weekend {
            format!("weekends around {hour:02}:00")
        } else {
            format!("weekdays around {hour:02}:00")
        };
        out.push(InsightDraft {
            kind: "time_window".into(),
            title: format!("{when}: ~{}", fmt_mins(stats.median_duration_secs)),
            body: format!(
                "For {when}, {bl} runs about {} based on {} similar trip{}. \
                 Overall corridor median is {}.",
                fmt_mins(stats.median_duration_secs),
                stats.n,
                if stats.n == 1 { "" } else { "s" },
                fmt_mins(overall.median_duration_secs)
            ),
            score: 0.7,
            context: json!({
                "hour_bin": hour,
                "is_weekend": is_weekend,
                "variant_id": vid,
                "median_duration_secs": stats.median_duration_secs,
            }),
        });
    }

    // —— Peak vs off-peak on the dominant path ——
    if let Some(best_id) = best_variant_id(&by_var, 1) {
        let mut hour_meds: Vec<(u8, f64, usize)> = Vec::new();
        for h in 0u8..24 {
            let mut durs: Vec<f64> = samples
                .iter()
                .filter(|s| s.variant_id == best_id && s.hour_bin == h)
                .map(|s| s.duration_secs)
                .collect();
            if durs.len() >= soft_n {
                if let Some(med) = super::stats::median(&mut durs) {
                    hour_meds.push((h, med, durs.len()));
                }
            }
        }
        if hour_meds.len() >= 2 {
            let fastest = hour_meds
                .iter()
                .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
            let slowest = hour_meds
                .iter()
                .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
            if let (Some(f), Some(s)) = (fastest, slowest) {
                let delta = s.1 - f.1;
                if delta >= 90.0 {
                    let bl = label_of(variant_labels, best_id, "your path");
                    out.push(InsightDraft {
                        kind: "peak_vs_offpeak".into(),
                        title: format!(
                            "Fastest around {:02}:00 · slowest {:02}:00",
                            f.0, s.0
                        ),
                        body: format!(
                            "On {bl}, trips near {:02}:00 median {} (n={}), while {:02}:00 runs about {} (n={}). \
                             Leaving in the faster window can save ~{}.",
                            f.0,
                            fmt_mins(f.1),
                            f.2,
                            s.0,
                            fmt_mins(s.1),
                            s.2,
                            fmt_mins(delta)
                        ),
                        score: delta / 60.0 + 0.3,
                        context: json!({
                            "fast_hour": f.0,
                            "slow_hour": s.0,
                            "saved_secs": delta,
                            "variant_id": best_id,
                        }),
                    });
                }
            }
        }
    }

    // —— Weekday vs weekend ——
    let mut wd: Vec<f64> = samples
        .iter()
        .filter(|s| !s.is_weekend)
        .map(|s| s.duration_secs)
        .collect();
    let mut we: Vec<f64> = samples
        .iter()
        .filter(|s| s.is_weekend)
        .map(|s| s.duration_secs)
        .collect();
    if wd.len() >= soft_n && we.len() >= soft_n {
        if let (Some(med_wd), Some(med_we)) = (
            super::stats::median(&mut wd),
            super::stats::median(&mut we),
        ) {
            let delta = (med_wd - med_we).abs();
            if delta >= 90.0 {
                let (faster, slower, fast_med, slow_med) = if med_we < med_wd {
                    ("weekends", "weekdays", med_we, med_wd)
                } else {
                    ("weekdays", "weekends", med_wd, med_we)
                };
                out.push(InsightDraft {
                    kind: "weekend_vs_weekday".into(),
                    title: format!("{faster} run quicker on this OD"),
                    body: format!(
                        "{faster} median {} vs {slower} {} — about {} difference (n_wd={}, n_we={}).",
                        fmt_mins(fast_med),
                        fmt_mins(slow_med),
                        fmt_mins(delta),
                        wd.len(),
                        we.len()
                    ),
                    score: delta / 60.0 + 0.25,
                    context: json!({
                        "weekday_median_secs": med_wd,
                        "weekend_median_secs": med_we,
                    }),
                });
            }
        }
    }

    // —— Stop dwell ——
    if overall.n >= soft_n && overall.median_duration_secs > 0.0 {
        let stop_share = overall.median_stop_time_secs / overall.median_duration_secs;
        if overall.median_stop_time_secs >= 120.0 && stop_share >= 0.12 {
            out.push(InsightDraft {
                kind: "high_stops".into(),
                title: format!(
                    "Stops add ~{}",
                    fmt_mins(overall.median_stop_time_secs)
                ),
                body: format!(
                    "Median stopped/dwell time is {} — about {:.0}% of the {} trip. \
                     Traffic lights, errands, or congestion may be eating the clock more than distance.",
                    fmt_mins(overall.median_stop_time_secs),
                    stop_share * 100.0,
                    fmt_mins(overall.median_duration_secs)
                ),
                score: (overall.median_stop_time_secs / 60.0) * 0.35,
                context: json!({
                    "median_stop_time_secs": overall.median_stop_time_secs,
                    "stop_share": stop_share,
                }),
            });
        }
    }

    // —— ORS reference vs best recorded ——
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
                let bl = label_of(variant_labels, best_id, "your usual path");
                if delta >= 120.0 && best.n >= soft_n {
                    out.push(InsightDraft {
                        kind: "ors_reference".into(),
                        title: "Router suggests a faster line".into(),
                        body: format!(
                            "OpenRouteService ({}) estimates about {} for this corridor, while {bl} averages {} from your drives (n={}). \
                             Router times are freeflow estimates—not live traffic.",
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
                } else if delta.abs() < 90.0 && best.n >= 1 {
                    out.push(InsightDraft {
                        kind: "ors_matches".into(),
                        title: "Your path matches the router".into(),
                        body: format!(
                            "{bl} averages {} vs ORS {} estimate of {}. \
                             You're already close to the freeflow suggestion.",
                            fmt_mins(best.median_duration_secs),
                            alt.preference,
                            fmt_mins(alt.duration_secs)
                        ),
                        score: 0.4,
                        context: json!({
                            "ors_preference": alt.preference,
                            "ors_duration_secs": alt.duration_secs,
                            "best_variant_id": best_id,
                        }),
                    });
                } else if delta <= -120.0 && best.n >= soft_n {
                    // Recorded faster than router — rare but nice
                    out.push(InsightDraft {
                        kind: "beats_router".into(),
                        title: format!("{bl} beats the router estimate"),
                        body: format!(
                            "Your drives on {bl} median {} — about {} quicker than ORS {} ({}). \
                             Local knowledge or light traffic may be helping.",
                            fmt_mins(best.median_duration_secs),
                            fmt_mins(-delta),
                            alt.preference,
                            fmt_mins(alt.duration_secs)
                        ),
                        score: (-delta / 60.0) * 0.35,
                        context: json!({
                            "ors_preference": alt.preference,
                            "delta_secs": delta,
                            "best_variant_id": best_id,
                        }),
                    });
                }
            }
        }
    }

    // De-dupe by kind (keep highest score)
    let mut best_by_kind: HashMap<String, InsightDraft> = HashMap::new();
    for d in out {
        best_by_kind
            .entry(d.kind.clone())
            .and_modify(|e| {
                if d.score > e.score {
                    *e = d.clone();
                }
            })
            .or_insert(d);
    }
    let mut out: Vec<InsightDraft> = best_by_kind.into_values().collect();
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

    fn sample(vid: Uuid, hour: u8, weekend: bool, dur: f64) -> VariantSample {
        VariantSample {
            variant_id: vid,
            hour_bin: hour,
            is_weekend: weekend,
            month: 6,
            duration_secs: dur,
            distance_m: 12_000.0,
            stop_time_secs: 30.0,
            elev_gain_m: Some(40.0),
        }
    }

    #[test]
    fn prefer_faster_variant_insight() {
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let mut labels = HashMap::new();
        labels.insert(a, "Variant A".into());
        labels.insert(b, "Variant B".into());
        let mut samples = Vec::new();
        for _ in 0..4 {
            samples.push(sample(a, 18, false, 600.0));
            samples.push(sample(b, 18, false, 900.0));
        }
        let now = Utc.with_ymd_and_hms(2026, 6, 3, 18, 30, 0).unwrap(); // Wed
        let insights = build_insights(&labels, &samples, &[], now, 3);
        assert!(insights.iter().any(|i| i.kind == "prefer_variant"));
        assert!(insights.iter().any(|i| i.kind == "avoid_variant_now"));
        assert!(insights.iter().any(|i| i.kind == "typical_pace"));
    }

    #[test]
    fn single_trip_still_yields_soft_insights() {
        let a = Uuid::new_v4();
        let mut labels = HashMap::new();
        labels.insert(a, "Path A".into());
        let samples = vec![sample(a, 8, false, 720.0)];
        let now = Utc.with_ymd_and_hms(2026, 6, 3, 8, 0, 0).unwrap();
        let insights = build_insights(&labels, &samples, &[], now, 3);
        assert!(
            !insights.is_empty(),
            "expected soft insights for a single trip"
        );
        assert!(insights.iter().any(|i| i.kind == "forming"));
        assert!(insights.iter().any(|i| i.kind == "single_path"));
    }

    #[test]
    fn ors_match_and_peak_insights() {
        let a = Uuid::new_v4();
        let mut labels = HashMap::new();
        labels.insert(a, "Main".into());
        let mut samples = Vec::new();
        for _ in 0..3 {
            samples.push(sample(a, 8, false, 900.0));
            samples.push(sample(a, 14, false, 600.0));
        }
        let ors = vec![OrsAltRef {
            preference: "fastest".into(),
            duration_secs: 610.0,
            distance_m: 11_500.0,
        }];
        let now = Utc.with_ymd_and_hms(2026, 6, 3, 10, 0, 0).unwrap();
        let insights = build_insights(&labels, &samples, &ors, now, 3);
        assert!(insights.iter().any(|i| i.kind == "peak_vs_offpeak"));
        assert!(insights.iter().any(|i| {
            i.kind == "ors_matches" || i.kind == "ors_reference" || i.kind == "beats_router"
        }));
    }

    #[test]
    fn empty_samples_empty_insights() {
        let labels = HashMap::new();
        let now = Utc::now();
        assert!(build_insights(&labels, &[], &[], now, 3).is_empty());
    }
}
