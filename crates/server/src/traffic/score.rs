//! Congestion scoring from speed vs free-flow.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrafficLevel {
    Free,
    Light,
    Moderate,
    Heavy,
    Jam,
    SignalStop,
}

impl TrafficLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Free => "free",
            Self::Light => "light",
            Self::Moderate => "moderate",
            Self::Heavy => "heavy",
            Self::Jam => "jam",
            Self::SignalStop => "signal_stop",
        }
    }
}

/// Parse OSM maxspeed tag to km/h.
pub fn parse_maxspeed_kph(raw: &str) -> Option<f64> {
    let s = raw.trim().to_lowercase();
    if s.is_empty() || s == "none" || s == "signals" {
        return None;
    }
    match s.as_str() {
        "walk" | "walking" => return Some(5.0),
        "urban" => return Some(50.0),
        "rural" => return Some(80.0),
        _ => {}
    }
    let mut parts = s.split_whitespace();
    let num: f64 = parts.next()?.parse().ok()?;
    if num <= 0.0 {
        return None;
    }
    let unit = parts.next().unwrap_or("km/h");
    if unit.contains("mph") {
        Some(num * 1.60934)
    } else {
        Some(num)
    }
}

pub fn highway_default_kph(highway: &str) -> f64 {
    match highway {
        "motorway" | "motorway_link" => 100.0,
        "trunk" | "trunk_link" => 80.0,
        "primary" | "primary_link" => 60.0,
        "secondary" | "secondary_link" => 50.0,
        "tertiary" | "tertiary_link" => 40.0,
        "unclassified" | "road" => 40.0,
        "residential" => 30.0,
        "living_street" => 15.0,
        "service" | "track" => 20.0,
        _ => 50.0,
    }
}

/// Moving-frame level from speed / v_ff (not signal).
pub fn level_from_ratio(speed_kph: f64, v_ff_kph: f64) -> TrafficLevel {
    let vff = v_ff_kph.max(5.0);
    let r = (speed_kph.max(0.0) / vff).clamp(0.0, 2.0);
    if r >= 0.85 {
        TrafficLevel::Free
    } else if r >= 0.65 {
        TrafficLevel::Light
    } else if r >= 0.45 {
        TrafficLevel::Moderate
    } else if r >= 0.25 {
        TrafficLevel::Heavy
    } else {
        TrafficLevel::Jam
    }
}

/// Free-flow from OSM tags (maxspeed or highway class default).
pub fn v_ff_from_osm(maxspeed_kph: Option<f64>, highway: &str) -> f64 {
    maxspeed_kph
        .filter(|v| v.is_finite() && *v > 0.0)
        .unwrap_or_else(|| highway_default_kph(highway))
}

/// Optional history boost: raise free-flow toward off-peak p85, capped.
pub fn apply_history_boost(v_ff: f64, p85_offpeak: Option<f64>, has_explicit_maxspeed: bool) -> f64 {
    let Some(p85) = p85_offpeak.filter(|v| v.is_finite() && *v > 0.0) else {
        return v_ff;
    };
    let boosted = (p85 * 0.95).max(v_ff);
    if has_explicit_maxspeed {
        boosted.min(v_ff * 1.1)
    } else {
        boosted
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_numeric_and_mph() {
        assert_eq!(parse_maxspeed_kph("50"), Some(50.0));
        assert!((parse_maxspeed_kph("30 mph").unwrap() - 48.28).abs() < 0.1);
    }

    #[test]
    fn parse_known_words() {
        assert_eq!(parse_maxspeed_kph("walk"), Some(5.0));
        assert_eq!(parse_maxspeed_kph("none"), None);
    }

    #[test]
    fn highway_defaults() {
        assert_eq!(highway_default_kph("motorway"), 100.0);
        assert_eq!(highway_default_kph("residential"), 30.0);
        assert_eq!(highway_default_kph("unknown_class"), 50.0);
    }

    #[test]
    fn levels_by_ratio() {
        assert_eq!(level_from_ratio(90.0, 100.0), TrafficLevel::Free);
        assert_eq!(level_from_ratio(70.0, 100.0), TrafficLevel::Light);
        assert_eq!(level_from_ratio(50.0, 100.0), TrafficLevel::Moderate);
        assert_eq!(level_from_ratio(30.0, 100.0), TrafficLevel::Heavy);
        assert_eq!(level_from_ratio(10.0, 100.0), TrafficLevel::Jam);
    }
}
