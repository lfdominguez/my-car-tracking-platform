//! Geometry helpers for OD clustering and path signatures.

use chrono::{DateTime, Utc};

const EARTH_RADIUS_M: f64 = 6_371_000.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LatLon {
    pub lat: f64,
    pub lon: f64,
}

#[derive(Debug, Clone)]
pub struct TimedPoint {
    pub at: DateTime<Utc>,
    pub lat: f64,
    pub lon: f64,
    pub speed_kph: Option<f64>,
}

pub fn haversine_m(a: LatLon, b: LatLon) -> f64 {
    let rlat1 = a.lat.to_radians();
    let rlat2 = b.lat.to_radians();
    let dlat = (b.lat - a.lat).to_radians();
    let dlon = (b.lon - a.lon).to_radians();
    let h = (dlat / 2.0).sin().powi(2) + rlat1.cos() * rlat2.cos() * (dlon / 2.0).sin().powi(2);
    2.0 * EARTH_RADIUS_M * h.sqrt().asin()
}

/// Median of coordinates in the first/last `window_secs` of the track.
pub fn median_endpoint(points: &[TimedPoint], start: bool, window_secs: i64) -> Option<LatLon> {
    if points.is_empty() {
        return None;
    }
    let anchor = if start {
        points.first()?.at
    } else {
        points.last()?.at
    };
    let mut lats = Vec::new();
    let mut lons = Vec::new();
    for p in points {
        let dt = if start {
            (p.at - anchor).num_seconds()
        } else {
            (anchor - p.at).num_seconds()
        };
        if dt < 0 || dt > window_secs {
            if start && dt > window_secs {
                break;
            }
            if !start && dt > window_secs {
                continue;
            }
            if !start && dt < 0 {
                break;
            }
            continue;
        }
        if p.lat.is_finite() && p.lon.is_finite() {
            lats.push(p.lat);
            lons.push(p.lon);
        }
    }
    if lats.is_empty() {
        // fallback: first/last point
        let p = if start {
            points.first()?
        } else {
            points.last()?
        };
        return Some(LatLon {
            lat: p.lat,
            lon: p.lon,
        });
    }
    lats.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    lons.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mid = lats.len() / 2;
    Some(LatLon {
        lat: lats[mid],
        lon: lons[mid],
    })
}

pub fn path_length_m(coords: &[LatLon]) -> f64 {
    coords
        .windows(2)
        .map(|w| haversine_m(w[0], w[1]))
        .sum()
}

/// Approximate cell size in degrees from meters at mid latitude.
fn meters_to_deg(cell_m: f64, lat: f64) -> (f64, f64) {
    let dlat = cell_m / 111_320.0;
    let dlon = cell_m / (111_320.0 * lat.to_radians().cos().max(0.2));
    (dlat, dlon)
}

pub fn cell_key(lat: f64, lon: f64, cell_m: f64) -> (i32, i32) {
    let (dlat, dlon) = meters_to_deg(cell_m, lat);
    let iy = (lat / dlat).floor() as i32;
    let ix = (lon / dlon).floor() as i32;
    (iy, ix)
}

/// Downsample path into ordered unique cells (~cell_m spacing).
pub fn path_signature(coords: &[LatLon], cell_m: f64) -> String {
    if coords.is_empty() {
        return String::new();
    }
    let mut cells: Vec<(i32, i32)> = Vec::new();
    let mut last: Option<LatLon> = None;
    for &c in coords {
        if let Some(prev) = last {
            if haversine_m(prev, c) < cell_m * 0.4 {
                continue;
            }
        }
        let key = cell_key(c.lat, c.lon, cell_m);
        if cells.last().copied() != Some(key) {
            cells.push(key);
        }
        last = Some(c);
    }
    cells
        .iter()
        .map(|(y, x)| format!("{y}:{x}"))
        .collect::<Vec<_>>()
        .join("|")
}

/// Jaccard similarity on cell sets encoded in signatures.
pub fn signature_similarity(a: &str, b: &str) -> f64 {
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    use std::collections::HashSet;
    let sa: HashSet<&str> = a.split('|').collect();
    let sb: HashSet<&str> = b.split('|').collect();
    let inter = sa.intersection(&sb).count() as f64;
    let uni = sa.union(&sb).count() as f64;
    if uni == 0.0 {
        0.0
    } else {
        inter / uni
    }
}

pub fn od_matches(a_start: LatLon, a_end: LatLon, b_start: LatLon, b_end: LatLon, radius_m: f64) -> bool {
    haversine_m(a_start, b_start) <= radius_m && haversine_m(a_end, b_end) <= radius_m
}

/// Total dwell seconds where speed ≤ max_kph for contiguous spans ≥ min_span_secs.
pub fn stop_time_secs(points: &[TimedPoint], max_kph: f64, min_span_secs: i64) -> f64 {
    if points.len() < 2 {
        return 0.0;
    }
    let mut total = 0.0_f64;
    let mut run_start: Option<DateTime<Utc>> = None;
    let mut last_t = points[0].at;
    for p in points {
        let stopped = p
            .speed_kph
            .map(|s| s.is_finite() && s <= max_kph)
            .unwrap_or(false);
        if stopped {
            if run_start.is_none() {
                run_start = Some(p.at);
            }
            last_t = p.at;
        } else if let Some(start) = run_start.take() {
            let secs = (last_t - start).num_seconds();
            if secs >= min_span_secs {
                total += secs as f64;
            }
        }
    }
    if let Some(start) = run_start {
        let secs = (last_t - start).num_seconds();
        if secs >= min_span_secs {
            total += secs as f64;
        }
    }
    total
}

pub fn variant_label(index: usize) -> String {
    // A, B, … Z, AA, …
    let mut n = index;
    let mut s = String::new();
    loop {
        s.insert(0, (b'A' + (n % 26) as u8) as char);
        if n < 26 {
            break;
        }
        n = n / 26 - 1;
    }
    format!("Variant {s}")
}


/// True when start≈end relative to driven path (round trip / loop).
pub fn is_circular(start: LatLon, end: LatLon, path_m: f64) -> bool {
    if path_m < 500.0 {
        return false;
    }
    let od = haversine_m(start, end);
    let threshold = 150.0_f64.max(0.15 * path_m);
    od < threshold
}

#[derive(Debug, Clone)]
pub struct DwellSegment {
    pub start_idx: usize,
    pub end_idx: usize,
    pub centroid: LatLon,
    pub duration_secs: f64,
}

/// Interior dwells (speed ≤ max_kph for ≥ min_span_secs), excluding edge windows.
pub fn interior_dwells(
    points: &[TimedPoint],
    min_span_secs: f64,
    max_kph: f64,
    exclude_edge_secs: f64,
) -> Vec<DwellSegment> {
    if points.len() < 3 {
        return vec![];
    }
    let t0 = points[0].at;
    let t1 = points[points.len() - 1].at;
    let total = (t1 - t0).num_milliseconds() as f64 / 1000.0;
    if total <= 2.0 * exclude_edge_secs {
        return vec![];
    }

    let mut out = Vec::new();
    let mut i = 0usize;
    while i < points.len() {
        let speed = points[i].speed_kph.unwrap_or(f64::MAX);
        let from_start = (points[i].at - t0).num_milliseconds() as f64 / 1000.0;
        let to_end = (t1 - points[i].at).num_milliseconds() as f64 / 1000.0;
        if speed > max_kph || from_start < exclude_edge_secs || to_end < exclude_edge_secs {
            i += 1;
            continue;
        }
        let start_i = i;
        let mut j = i + 1;
        while j < points.len() {
            let s = points[j].speed_kph.unwrap_or(f64::MAX);
            let fs = (points[j].at - t0).num_milliseconds() as f64 / 1000.0;
            let te = (t1 - points[j].at).num_milliseconds() as f64 / 1000.0;
            if s > max_kph || fs < exclude_edge_secs || te < exclude_edge_secs {
                break;
            }
            j += 1;
        }
        let end_i = j - 1;
        let span = (points[end_i].at - points[start_i].at).num_milliseconds() as f64 / 1000.0;
        if span >= min_span_secs {
            let slice = &points[start_i..=end_i];
            let lat = slice.iter().map(|p| p.lat).sum::<f64>() / slice.len() as f64;
            let lon = slice.iter().map(|p| p.lon).sum::<f64>() / slice.len() as f64;
            out.push(DwellSegment {
                start_idx: start_i,
                end_idx: end_i,
                centroid: LatLon { lat, lon },
                duration_secs: span,
            });
        }
        i = j.max(i + 1);
    }
    out
}

/// Pick split dwell: longest, preferring farther from home when close.
pub fn best_split_dwell(start: LatLon, dwells: &[DwellSegment], min_away_m: f64) -> Option<&DwellSegment> {
    let mut best: Option<&DwellSegment> = None;
    for d in dwells {
        let away = haversine_m(start, d.centroid);
        if away < min_away_m {
            continue;
        }
        best = Some(match best {
            None => d,
            Some(b) => {
                if d.duration_secs > b.duration_secs + 30.0 {
                    d
                } else if (d.duration_secs - b.duration_secs).abs() <= 30.0
                    && away > haversine_m(start, b.centroid)
                {
                    d
                } else if d.duration_secs > b.duration_secs {
                    d
                } else {
                    b
                }
            }
        });
    }
    best
}

/// Farthest point from `home` along the path (for via/turnaround).
pub fn farthest_via(home: LatLon, coords: &[LatLon], min_away_m: f64) -> Option<LatLon> {
    let mut best: Option<(f64, LatLon)> = None;
    for &c in coords {
        let d = haversine_m(home, c);
        if d < min_away_m {
            continue;
        }
        if best.map(|(bd, _)| d > bd).unwrap_or(true) {
            best = Some((d, c));
        }
    }
    best.map(|(_, p)| p)
}

#[derive(Debug, Clone)]
pub struct PlannedLeg {
    pub leg_index: i16,
    pub start: LatLon,
    pub end: LatLon,
    pub via: Option<LatLon>,
    pub is_round_trip: bool,
    /// Indices into original timed points [start_idx, end_idx] inclusive.
    pub point_start: usize,
    pub point_end: usize,
}

/// Plan legs for a finished track: normal OD, split circular, or via circular.
pub fn plan_legs(
    points: &[TimedPoint],
    coords: &[LatLon],
    start: LatLon,
    end: LatLon,
    path_m: f64,
    min_leg_m: f64,
) -> Vec<PlannedLeg> {
    const SPLIT_DWELL_SECS: f64 = 240.0;
    const EXCLUDE_EDGE_SECS: f64 = 180.0;
    const MAX_STOP_KPH: f64 = 2.0;
    const MIN_VIA_AWAY_M: f64 = 400.0;
    const MIN_SPLIT_AWAY_M: f64 = 200.0;

    if !is_circular(start, end, path_m) {
        return vec![PlannedLeg {
            leg_index: 0,
            start,
            end,
            via: None,
            is_round_trip: false,
            point_start: 0,
            point_end: points.len().saturating_sub(1),
        }];
    }

    let dwells = interior_dwells(points, SPLIT_DWELL_SECS, MAX_STOP_KPH, EXCLUDE_EDGE_SECS);
    if let Some(d) = best_split_dwell(start, &dwells, MIN_SPLIT_AWAY_M) {
        let mid = d.start_idx;
        let mid_end = d.end_idx;
        // Outbound: start .. dwell start; return: dwell end .. finish
        let mut legs = Vec::new();
        let out_end = coords.get(mid).copied().unwrap_or(d.centroid);
        let out_path = path_length_m(&coords[..=mid.min(coords.len() - 1)]);
        if out_path >= min_leg_m {
            legs.push(PlannedLeg {
                leg_index: 0,
                start,
                end: out_end,
                via: None,
                is_round_trip: false,
                point_start: 0,
                point_end: mid,
            });
        }
        let ret_start = coords.get(mid_end).copied().unwrap_or(d.centroid);
        let ret_slice_start = mid_end.min(coords.len() - 1);
        let ret_path = path_length_m(&coords[ret_slice_start..]);
        if ret_path >= min_leg_m {
            legs.push(PlannedLeg {
                leg_index: legs.len() as i16,
                start: ret_start,
                end,
                via: None,
                is_round_trip: false,
                point_start: mid_end,
                point_end: points.len().saturating_sub(1),
            });
        }
        if !legs.is_empty() {
            return legs;
        }
    }

    // Via / turnaround fallback — single round-trip corridor
    let via = farthest_via(start, coords, MIN_VIA_AWAY_M).or_else(|| {
        // weaker: any farthest even if closer
        farthest_via(start, coords, 100.0)
    });
    vec![PlannedLeg {
        leg_index: 0,
        start,
        end: start, // normalize circular home
        via,
        is_round_trip: true,
        point_start: 0,
        point_end: points.len().saturating_sub(1),
    }]
}


#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn haversine_known_distance() {
        // ~1 degree lat ≈ 111 km
        let a = LatLon { lat: 0.0, lon: 0.0 };
        let b = LatLon { lat: 1.0, lon: 0.0 };
        let d = haversine_m(a, b);
        assert!((d - 111_195.0).abs() < 500.0);
    }

    #[test]
    fn od_match_within_radius() {
        let a = LatLon {
            lat: 23.05,
            lon: -82.35,
        };
        let b = LatLon {
            lat: 23.0505,
            lon: -82.3505,
        };
        let c = LatLon {
            lat: 23.12,
            lon: -82.40,
        };
        let d = LatLon {
            lat: 23.1205,
            lon: -82.4005,
        };
        assert!(od_matches(a, c, b, d, 200.0));
        assert!(!od_matches(a, c, c, a, 200.0)); // reversed
    }

    #[test]
    fn signature_similar_paths() {
        let mut path1 = Vec::new();
        let mut path2 = Vec::new();
        for i in 0..20 {
            let lat = 23.0 + i as f64 * 0.001;
            path1.push(LatLon {
                lat,
                lon: -82.0,
            });
            path2.push(LatLon {
                lat: lat + 0.00005,
                lon: -82.0,
            });
        }
        let s1 = path_signature(&path1, 75.0);
        let s2 = path_signature(&path2, 75.0);
        assert!(signature_similarity(&s1, &s2) > 0.5);
    }

    #[test]
    fn signature_different_paths() {
        let path1: Vec<_> = (0..20)
            .map(|i| LatLon {
                lat: 23.0 + i as f64 * 0.001,
                lon: -82.0,
            })
            .collect();
        let path2: Vec<_> = (0..20)
            .map(|i| LatLon {
                lat: 23.0 + i as f64 * 0.001,
                lon: -82.05,
            })
            .collect();
        let s1 = path_signature(&path1, 75.0);
        let s2 = path_signature(&path2, 75.0);
        assert!(signature_similarity(&s1, &s2) < 0.3);
    }

    #[test]
    fn stop_time_detects_dwell() {
        let t0 = Utc.with_ymd_and_hms(2026, 1, 1, 12, 0, 0).unwrap();
        let mut pts = Vec::new();
        for i in 0..10 {
            pts.push(TimedPoint {
                at: t0 + chrono::Duration::seconds(i * 10),
                lat: 23.0,
                lon: -82.0,
                speed_kph: Some(30.0),
            });
        }
        // 90s stopped
        for i in 0..10 {
            pts.push(TimedPoint {
                at: t0 + chrono::Duration::seconds(100 + i * 10),
                lat: 23.0,
                lon: -82.0,
                speed_kph: Some(0.0),
            });
        }
        let st = stop_time_secs(&pts, 2.0, 60);
        assert!(st >= 60.0);
    }

    #[test]
    fn variant_labels() {
        assert_eq!(variant_label(0), "Variant A");
        assert_eq!(variant_label(1), "Variant B");
        assert_eq!(variant_label(25), "Variant Z");
    }

    #[test]
    fn circular_when_od_small_vs_path() {
        let home = LatLon { lat: 23.0, lon: -82.0 };
        assert!(is_circular(home, home, 12_000.0));
        let near = LatLon { lat: 23.0005, lon: -82.0 }; // ~55m
        assert!(is_circular(home, near, 12_000.0));
        let far = LatLon { lat: 23.1, lon: -82.0 }; // ~11km
        assert!(!is_circular(home, far, 12_000.0));
    }

    #[test]
    fn plan_legs_split_on_long_interior_stop() {
        use chrono::Duration;
        let t0 = Utc::now();
        let mut pts = Vec::new();
        // drive out 10 min
        for i in 0..20 {
            pts.push(TimedPoint {
                at: t0 + Duration::seconds(i * 30),
                lat: 23.0 + i as f64 * 0.001,
                lon: -82.0,
                speed_kph: Some(40.0),
            });
        }
        // stop 5 min at destination
        let base = 20 * 30;
        for i in 0..10 {
            pts.push(TimedPoint {
                at: t0 + Duration::seconds(base + i * 30),
                lat: 23.02,
                lon: -82.0,
                speed_kph: Some(0.0),
            });
        }
        // drive back 10 min
        let base2 = base + 10 * 30;
        for i in 0..20 {
            pts.push(TimedPoint {
                at: t0 + Duration::seconds(base2 + i * 30),
                lat: 23.02 - i as f64 * 0.001,
                lon: -82.0,
                speed_kph: Some(40.0),
            });
        }
        let coords: Vec<_> = pts
            .iter()
            .map(|p| LatLon {
                lat: p.lat,
                lon: p.lon,
            })
            .collect();
        let start = coords[0];
        let end = *coords.last().unwrap();
        let path = path_length_m(&coords);
        assert!(is_circular(start, end, path));
        let legs = plan_legs(&pts, &coords, start, end, path, 500.0);
        assert!(legs.len() >= 2, "expected split legs, got {:?}", legs.len());
        assert!(!legs[0].is_round_trip);
        assert!(legs[0].via.is_none());
    }

    #[test]
    fn plan_legs_via_when_no_long_stop() {
        use chrono::Duration;
        let t0 = Utc::now();
        let mut pts = Vec::new();
        // out and back without long stop
        for i in 0..30 {
            let lat = if i <= 15 {
                23.0 + i as f64 * 0.002
            } else {
                23.0 + (30 - i) as f64 * 0.002
            };
            pts.push(TimedPoint {
                at: t0 + Duration::seconds(i * 20),
                lat,
                lon: -82.0,
                speed_kph: Some(50.0),
            });
        }
        let coords: Vec<_> = pts
            .iter()
            .map(|p| LatLon {
                lat: p.lat,
                lon: p.lon,
            })
            .collect();
        let start = coords[0];
        let end = *coords.last().unwrap();
        let path = path_length_m(&coords);
        let legs = plan_legs(&pts, &coords, start, end, path, 500.0);
        assert_eq!(legs.len(), 1);
        assert!(legs[0].is_round_trip);
        assert!(legs[0].via.is_some());
    }

}
