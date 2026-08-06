//! Path frames (“trames”) for congestion scoring.

use chrono::{DateTime, Utc};

use crate::route_opt::{haversine_m, LatLon};

use super::score::{level_from_ratio, TrafficLevel};

pub const FRAME_DIST_M: f64 = 80.0;
pub const FRAME_TIME_SECS: f64 = 10.0;
pub const STOP_SPEED_KPH: f64 = 5.0;
const MAX_ACCEL_ABS: f64 = 15.0;

#[derive(Debug, Clone)]
pub struct RawPoint {
    pub t: DateTime<Utc>,
    pub lat: f64,
    pub lon: f64,
    pub speed_kph: Option<f64>,
    pub pedal: Option<f64>,
}

#[derive(Debug, Clone)]
pub struct TrafficFrame {
    pub seq: i32,
    pub t_start: DateTime<Utc>,
    pub t_end: DateTime<Utc>,
    pub lat: f64,
    pub lon: f64,
    pub speed_kph: f64,
    pub distance_m: f64,
    pub mean_accel_mps2: f64,
    pub mean_pedal: Option<f64>,
    pub speed_std: f64,
}

#[derive(Debug, Clone)]
pub struct ScoredFrame {
    pub frame: TrafficFrame,
    pub v_ff_kph: f64,
    pub osm_way_id: Option<i64>,
    pub level: TrafficLevel,
}

pub fn build_frames(points: &[RawPoint]) -> Vec<TrafficFrame> {
    if points.len() < 2 {
        return Vec::new();
    }

    let speeds = resolve_speeds(points);
    let accels = resolve_accels(points, &speeds);

    let mut frames = Vec::new();
    let mut start = 0usize;
    let mut seq = 0i32;

    while start < points.len() {
        let mut end = start;
        let mut dist = 0.0_f64;
        let t0 = points[start].t;

        while end + 1 < points.len() {
            let d = haversine_m(
                LatLon {
                    lat: points[end].lat,
                    lon: points[end].lon,
                },
                LatLon {
                    lat: points[end + 1].lat,
                    lon: points[end + 1].lon,
                },
            );
            let next_dist = dist + d;
            let dt = (points[end + 1].t - t0).num_milliseconds() as f64 / 1000.0;
            end += 1;
            dist = next_dist;
            if dist >= FRAME_DIST_M || dt >= FRAME_TIME_SECS {
                break;
            }
        }

        if end == start {
            // single trailing point
            if start + 1 < points.len() {
                end = start + 1;
                dist = haversine_m(
                    LatLon {
                        lat: points[start].lat,
                        lon: points[start].lon,
                    },
                    LatLon {
                        lat: points[end].lat,
                        lon: points[end].lon,
                    },
                );
            } else {
                break;
            }
        }

        let slice_speeds: Vec<f64> = speeds[start..=end].to_vec();
        let median_speed = median_f64(&slice_speeds).unwrap_or(0.0);
        let speed_std = std_f64(&slice_speeds);

        let mut accel_sum = 0.0;
        let mut accel_n = 0usize;
        for a in accels.iter().take(end).skip(start) {
            if let Some(v) = a {
                accel_sum += *v;
                accel_n += 1;
            }
        }
        let mean_accel = if accel_n > 0 {
            accel_sum / accel_n as f64
        } else {
            0.0
        };

        let mut pedal_sum = 0.0;
        let mut pedal_n = 0usize;
        for p in &points[start..=end] {
            if let Some(ped) = p.pedal.filter(|v| v.is_finite()) {
                pedal_sum += ped;
                pedal_n += 1;
            }
        }
        let mean_pedal = if pedal_n > 0 {
            Some(pedal_sum / pedal_n as f64)
        } else {
            None
        };

        let n = (end - start + 1) as f64;
        let lat = points[start..=end].iter().map(|p| p.lat).sum::<f64>() / n;
        let lon = points[start..=end].iter().map(|p| p.lon).sum::<f64>() / n;

        frames.push(TrafficFrame {
            seq,
            t_start: points[start].t,
            t_end: points[end].t,
            lat,
            lon,
            speed_kph: median_speed,
            distance_m: dist,
            mean_accel_mps2: mean_accel,
            mean_pedal,
            speed_std,
        });
        seq += 1;

        if end + 1 >= points.len() {
            break;
        }
        start = end;
    }

    frames
}

/// Assign ratio-based levels, then promote clear signal stops.
pub fn label_frames(frames: &mut [ScoredFrame]) {
    for f in frames.iter_mut() {
        f.level = level_from_ratio(f.frame.speed_kph, f.v_ff_kph);
    }

    if frames.len() < 2 {
        return;
    }

    // Merge consecutive near-stop frames; signal stops often span multiple ~10s frames.
    let mut i = 0usize;
    while i < frames.len() {
        if frames[i].frame.speed_kph >= STOP_SPEED_KPH {
            i += 1;
            continue;
        }
        let start = i;
        while i < frames.len() && frames[i].frame.speed_kph < STOP_SPEED_KPH {
            i += 1;
        }
        let end = i; // exclusive
        let dur = (frames[end - 1].frame.t_end - frames[start].frame.t_start)
            .num_milliseconds() as f64
            / 1000.0;
        if !(15.0..=180.0).contains(&dur) {
            continue;
        }

        let prev_crawl = start
            .checked_sub(1)
            .map(|j| {
                let r = frames[j].frame.speed_kph / frames[j].v_ff_kph.max(5.0);
                r < 0.45 && frames[j].frame.speed_kph >= STOP_SPEED_KPH
            })
            .unwrap_or(false);
        if prev_crawl {
            continue;
        }

        let leave_ok = frames.get(end).is_some_and(|n| {
            n.frame.mean_accel_mps2 > 0.8
                || n.frame.mean_pedal.is_some_and(|p| p > 20.0)
                || n.frame.speed_kph > STOP_SPEED_KPH + 5.0
        });
        if leave_ok {
            for f in &mut frames[start..end] {
                f.level = TrafficLevel::SignalStop;
            }
        }
    }
}

fn resolve_speeds(points: &[RawPoint]) -> Vec<f64> {
    let mut out = Vec::with_capacity(points.len());
    for (i, p) in points.iter().enumerate() {
        if let Some(s) = p.speed_kph.filter(|v| v.is_finite() && *v >= 0.0) {
            out.push(s);
            continue;
        }
        if i == 0 {
            out.push(0.0);
            continue;
        }
        let prev = &points[i - 1];
        let dt = (p.t - prev.t).num_milliseconds() as f64 / 1000.0;
        if dt <= 0.05 {
            out.push(out[i - 1]);
            continue;
        }
        let d = haversine_m(
            LatLon {
                lat: prev.lat,
                lon: prev.lon,
            },
            LatLon {
                lat: p.lat,
                lon: p.lon,
            },
        );
        out.push((d / dt) * 3.6);
    }
    out
}

fn resolve_accels(points: &[RawPoint], speeds: &[f64]) -> Vec<Option<f64>> {
    let mut out = vec![None; points.len()];
    for i in 1..points.len() {
        let dt = (points[i].t - points[i - 1].t).num_milliseconds() as f64 / 1000.0;
        if dt <= 0.05 {
            continue;
        }
        let v0 = speeds[i - 1] / 3.6;
        let v1 = speeds[i] / 3.6;
        let a = (v1 - v0) / dt;
        if a.is_finite() && a.abs() <= MAX_ACCEL_ABS {
            out[i] = Some(a);
        }
    }
    out
}

fn median_f64(vals: &[f64]) -> Option<f64> {
    if vals.is_empty() {
        return None;
    }
    let mut v = vals.to_vec();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mid = v.len() / 2;
    Some(if v.len() % 2 == 1 {
        v[mid]
    } else {
        (v[mid - 1] + v[mid]) / 2.0
    })
}

fn std_f64(vals: &[f64]) -> f64 {
    if vals.len() < 2 {
        return 0.0;
    }
    let mean = vals.iter().sum::<f64>() / vals.len() as f64;
    let var = vals.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / (vals.len() - 1) as f64;
    var.sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn t(sec: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(1_700_000_000 + sec, 0).unwrap()
    }

    /// ~20 m north steps (approx) at 1 Hz with given speed.
    fn path_north(n: usize, speed_kph: f64) -> Vec<RawPoint> {
        // 20 m in lat ≈ 20 / 111_320 deg
        let dlat = 20.0 / 111_320.0;
        (0..n)
            .map(|i| RawPoint {
                t: t(i as i64),
                lat: -23.5 + dlat * i as f64,
                lon: -46.6,
                speed_kph: Some(speed_kph),
                pedal: None,
            })
            .collect()
    }

    #[test]
    fn frames_split_by_distance_or_time() {
        let pts = path_north(20, 50.0);
        let frames = build_frames(&pts);
        assert!(frames.len() >= 3, "expected several frames, got {}", frames.len());
        for f in &frames {
            let dur = (f.t_end - f.t_start).num_milliseconds() as f64 / 1000.0;
            assert!(
                f.distance_m + 1.0 >= FRAME_DIST_M || dur + 0.5 >= FRAME_TIME_SECS || f.seq == frames.last().unwrap().seq,
                "frame seq={} dist={} dur={}",
                f.seq,
                f.distance_m,
                dur
            );
        }
    }

    #[test]
    fn signal_stop_when_stationary_then_leave() {
        let mut pts = Vec::new();
        // Approach
        for i in 0..5 {
            pts.push(RawPoint {
                t: t(i),
                lat: -23.5,
                lon: -46.6,
                speed_kph: Some(40.0),
                pedal: Some(10.0),
            });
        }
        // Stop ~40s
        for i in 5..45 {
            pts.push(RawPoint {
                t: t(i),
                lat: -23.5,
                lon: -46.6,
                speed_kph: Some(0.0),
                pedal: Some(0.0),
            });
        }
        // Leave
        for i in 45..55 {
            pts.push(RawPoint {
                t: t(i),
                lat: -23.5 + 0.0001 * (i - 44) as f64,
                lon: -46.6,
                speed_kph: Some(20.0 + (i - 45) as f64),
                pedal: Some(40.0),
            });
        }

        let built = build_frames(&pts);
        let mut scored: Vec<ScoredFrame> = built
            .into_iter()
            .map(|frame| ScoredFrame {
                frame,
                v_ff_kph: 50.0,
                osm_way_id: None,
                level: TrafficLevel::Free,
            })
            .collect();
        label_frames(&mut scored);
        assert!(
            scored.iter().any(|f| f.level == TrafficLevel::SignalStop),
            "levels: {:?}",
            scored.iter().map(|f| f.level).collect::<Vec<_>>()
        );
    }

    #[test]
    fn crawl_is_congestion_not_signal() {
        let pts: Vec<RawPoint> = (0..60)
            .map(|i| RawPoint {
                t: t(i),
                lat: -23.5 + (8.0 / 3.6) * i as f64 / 111_320.0, // ~8 kph
                lon: -46.6,
                speed_kph: Some(8.0),
                pedal: Some(5.0),
            })
            .collect();
        let built = build_frames(&pts);
        let mut scored: Vec<ScoredFrame> = built
            .into_iter()
            .map(|frame| ScoredFrame {
                frame,
                v_ff_kph: 50.0,
                osm_way_id: None,
                level: TrafficLevel::Free,
            })
            .collect();
        label_frames(&mut scored);
        assert!(
            scored.iter().all(|f| f.level != TrafficLevel::SignalStop),
            "crawl should not be signal_stop: {:?}",
            scored.iter().map(|f| f.level.as_str()).collect::<Vec<_>>()
        );
        assert!(
            scored
                .iter()
                .any(|f| matches!(f.level, TrafficLevel::Heavy | TrafficLevel::Jam | TrafficLevel::Moderate)),
            "expected congested levels"
        );
    }
}
