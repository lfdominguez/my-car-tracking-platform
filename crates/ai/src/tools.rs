//! Typed Rig tools over a prebuilt [`TripAnalysisContext`].

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::de::Error as DeError;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{json, Value};

use crate::context::{SamplePoint, TripAnalysisContext};
use crate::error::AiError;
use crate::math::evaluate_expression;
use crate::report::AnalysisReport;

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct ToolErr(pub String);

impl From<ToolErr> for AiError {
    fn from(e: ToolErr) -> Self {
        AiError::Tool(e.0)
    }
}

fn dump<T: Serialize>(v: &T) -> Result<String, ToolErr> {
    serde_json::to_string_pretty(v).map_err(|e| ToolErr(e.to_string()))
}

// --- shared handles ---

#[derive(Clone)]
pub struct CtxHandle(pub Arc<TripAnalysisContext>);

#[derive(Clone, Default)]
pub struct ReportSlot(pub Arc<Mutex<Option<AnalysisReport>>>);

impl ReportSlot {
    pub fn take(&self) -> Option<AnalysisReport> {
        self.0.lock().ok().and_then(|mut g| g.take())
    }
}

// --- get_trip_overview ---

#[derive(Deserialize, Serialize)]
pub struct EmptyArgs {}

#[derive(Clone)]
pub struct GetTripOverview {
    pub ctx: CtxHandle,
}

impl Tool for GetTripOverview {
    const NAME: &'static str = "get_trip_overview";
    type Error = ToolErr;
    type Args = EmptyArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.into(),
            description: "Trip/car overview: distance, duration, speeds, fuel used, fuel type, engine snapshot, unit labels.".into(),
            parameters: json!({
                "type": "object",
                "properties": {},
            }),
        }
    }

    async fn call(&self, _args: Self::Args) -> Result<Self::Output, Self::Error> {
        let o = &self.ctx.0.overview;
        let payload = json!({
            "overview": o,
            "units": self.ctx.0.units,
            "prior_report_available": self.ctx.0.prior_markdown.is_some(),
        });
        dump(&payload)
    }
}

// --- get_speed_profile ---

#[derive(Clone)]
pub struct GetSpeedProfile {
    pub ctx: CtxHandle,
}

impl Tool for GetSpeedProfile {
    const NAME: &'static str = "get_speed_profile";
    type Error = ToolErr;
    type Args = EmptyArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.into(),
            description: "Speed percentiles, hard accel/brake counts, moving share.".into(),
            parameters: json!({ "type": "object", "properties": {} }),
        }
    }

    async fn call(&self, _args: Self::Args) -> Result<Self::Output, Self::Error> {
        dump(&self.ctx.0.speed)
    }
}

// --- get_engine_stats ---

#[derive(Clone)]
pub struct GetEngineStats {
    pub ctx: CtxHandle,
}

impl Tool for GetEngineStats {
    const NAME: &'static str = "get_engine_stats";
    type Error = ToolErr;
    type Args = EmptyArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.into(),
            description: "RPM/load/MAF/MAP aggregates and high-RPM share.".into(),
            parameters: json!({ "type": "object", "properties": {} }),
        }
    }

    async fn call(&self, _args: Self::Args) -> Result<Self::Output, Self::Error> {
        dump(&self.ctx.0.engine)
    }
}

// --- get_fuel_mixture_stats ---

#[derive(Clone)]
pub struct GetFuelMixtureStats {
    pub ctx: CtxHandle,
}

impl Tool for GetFuelMixtureStats {
    const NAME: &'static str = "get_fuel_mixture_stats";
    type Error = ToolErr;
    type Args = EmptyArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.into(),
            description: "Fuel rate, level, short/long term fuel trims, lambda ranges.".into(),
            parameters: json!({ "type": "object", "properties": {} }),
        }
    }

    async fn call(&self, _args: Self::Args) -> Result<Self::Output, Self::Error> {
        dump(&self.ctx.0.fuel)
    }
}

// --- get_thermal_electrical_stats ---

#[derive(Clone)]
pub struct GetThermalElectricalStats {
    pub ctx: CtxHandle,
}

impl Tool for GetThermalElectricalStats {
    const NAME: &'static str = "get_thermal_electrical_stats";
    type Error = ToolErr;
    type Args = EmptyArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.into(),
            description: "Coolant, IAT, ambient, module voltage, atmospheric pressure ranges.".into(),
            parameters: json!({ "type": "object", "properties": {} }),
        }
    }

    async fn call(&self, _args: Self::Args) -> Result<Self::Output, Self::Error> {
        dump(&self.ctx.0.thermal)
    }
}

// --- get_stop_summary ---

#[derive(Clone)]
pub struct GetStopSummary {
    pub ctx: CtxHandle,
}

impl Tool for GetStopSummary {
    const NAME: &'static str = "get_stop_summary";
    type Error = ToolErr;
    type Args = EmptyArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.into(),
            description: "Stops where speed ~0 for >= 60s (count, total/longest duration, list).".into(),
            parameters: json!({ "type": "object", "properties": {} }),
        }
    }

    async fn call(&self, _args: Self::Args) -> Result<Self::Output, Self::Error> {
        dump(&self.ctx.0.stops)
    }
}

// --- get_traffic_summary ---

#[derive(Clone)]
pub struct GetTrafficSummary {
    pub ctx: CtxHandle,
}

impl Tool for GetTrafficSummary {
    const NAME: &'static str = "get_traffic_summary";
    type Error = ToolErr;
    type Args = EmptyArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.into(),
            description: "Road congestion / traffic analysis summary for this trip if it was already analyzed (overall index, time and distance shares by congestion class, frame count). If unavailable, available=false.".into(),
            parameters: json!({ "type": "object", "properties": {} }),
        }
    }

    async fn call(&self, _args: Self::Args) -> Result<Self::Output, Self::Error> {
        dump(&self.ctx.0.traffic)
    }
}

// --- get_route_position_profile ---

#[derive(Clone)]
pub struct GetRoutePositionProfile {
    pub ctx: CtxHandle,
}

impl Tool for GetRoutePositionProfile {
    const NAME: &'static str = "get_route_position_profile";
    type Error = ToolErr;
    type Args = EmptyArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.into(),
            description: "Place/road type along the route at every ~5% of trip duration (OSM highway match). Use this BEFORE interpreting slow speeds as city traffic jams — residential_street / living_street / service_access often mean housing complexes, driveways, or private roads, not urban congestion. Returns samples with pct, lat/lon, speed, osm_highway, position_type, plus type_counts. available=false if no OSM matches.".into(),
            parameters: json!({ "type": "object", "properties": {} }),
        }
    }

    async fn call(&self, _args: Self::Args) -> Result<Self::Output, Self::Error> {
        dump(&self.ctx.0.route_positions)
    }
}

// --- get_point_window ---

/// Default number of slim timeline anchors in a window summary.
pub const DEFAULT_POINT_WINDOW_ANCHORS: usize = 5;
/// Hard cap on anchors (keeps tool results small for the model).
pub const MAX_POINT_WINDOW_ANCHORS: usize = 8;

#[derive(Debug, Deserialize, Serialize)]
pub struct PointWindowArgs {
    /// ISO-8601 start (inclusive).
    pub start: DateTime<Utc>,
    /// ISO-8601 end (inclusive).
    pub end: DateTime<Utc>,
    /// Number of slim anchor samples (default 5, max 8). Not a raw dump size.
    #[serde(default = "default_point_window_limit")]
    pub limit: usize,
}

fn default_point_window_limit() -> usize {
    DEFAULT_POINT_WINDOW_ANCHORS
}

#[derive(Clone)]
pub struct GetPointWindow {
    pub ctx: CtxHandle,
}

impl Tool for GetPointWindow {
    const NAME: &'static str = "get_point_window";
    type Error = ToolErr;
    type Args = PointWindowArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.into(),
            description: "Summarize telemetry in a time window [start, end] (ISO-8601). Returns window aggregates (min/avg/max for speed, RPM, load, fuel rate, coolant, voltage; min/max trims/lambda) plus a few slim anchors (default 5, max 8: time, lat/lon, speed, rpm, load, fuel_rate, coolant) — not a full raw point dump. Prefer trip-level tools for whole-trip stats; use this only for local drill-down.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "start": { "type": "string", "description": "ISO-8601 start time (inclusive)" },
                    "end": { "type": "string", "description": "ISO-8601 end time (inclusive)" },
                    "limit": {
                        "type": "integer",
                        "description": "Number of slim anchor points (default 5, max 8). Does not return dense raw series."
                    }
                },
                "required": ["start", "end"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let payload = build_point_window_payload(
            &self.ctx.0.samples,
            args.start,
            args.end,
            args.limit,
        );
        dump(&payload)
    }
}

/// Build the compact window payload (summary + slim anchors). Public for tests.
pub fn build_point_window_payload(
    samples: &[SamplePoint],
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    anchor_limit: usize,
) -> Value {
    let matched: Vec<&SamplePoint> = samples
        .iter()
        .filter(|p| p.recorded_at >= start && p.recorded_at <= end)
        .collect();

    let limit = anchor_limit
        .clamp(1, MAX_POINT_WINDOW_ANCHORS)
        .min(matched.len().max(1));

    if matched.is_empty() {
        return json!({
            "matched_count": 0,
            "anchor_count": 0,
            "start": start,
            "end": end,
            "duration_secs": (end - start).num_milliseconds() as f64 / 1000.0,
            "summary": {},
            "anchors": [],
            "note": "No samples in this time window.",
        });
    }

    let first = matched[0];
    let last = matched[matched.len() - 1];
    let duration_secs = (last.recorded_at - first.recorded_at)
        .num_milliseconds() as f64
        / 1000.0;

    let mut summary = serde_json::Map::new();
    if let Some(s) = metric_avg_stats(matched.iter().filter_map(|p| p.speed_kph)) {
        summary.insert("speed_kph".into(), s);
    }
    if let Some(s) = metric_avg_stats(matched.iter().filter_map(|p| p.rpm)) {
        summary.insert("rpm".into(), s);
    }
    if let Some(s) = metric_avg_stats(matched.iter().filter_map(|p| p.engine_load_pct)) {
        summary.insert("engine_load_pct".into(), s);
    }
    if let Some(s) = metric_avg_stats(matched.iter().filter_map(|p| p.fuel_rate_lph)) {
        summary.insert("fuel_rate_lph".into(), s);
    }
    if let Some(s) = metric_avg_stats(matched.iter().filter_map(|p| p.coolant_c)) {
        summary.insert("coolant_c".into(), s);
    }
    if let Some(s) = metric_avg_stats(matched.iter().filter_map(|p| p.voltage)) {
        summary.insert("voltage".into(), s);
    }
    if let Some(s) = metric_minmax_stats(matched.iter().filter_map(|p| p.stft_pct)) {
        summary.insert("stft_pct".into(), s);
    }
    if let Some(s) = metric_minmax_stats(matched.iter().filter_map(|p| p.ltft_pct)) {
        summary.insert("ltft_pct".into(), s);
    }
    if let Some(s) = metric_minmax_stats(matched.iter().filter_map(|p| p.lambda)) {
        summary.insert("lambda".into(), s);
    }

    // Start/end coordinates from first/last point that has them.
    if let Some(p) = matched.iter().find(|p| p.lat.is_some() && p.lon.is_some()) {
        summary.insert("start_lat".into(), json!(p.lat));
        summary.insert("start_lon".into(), json!(p.lon));
    }
    if let Some(p) = matched.iter().rev().find(|p| p.lat.is_some() && p.lon.is_some()) {
        summary.insert("end_lat".into(), json!(p.lat));
        summary.insert("end_lon".into(), json!(p.lon));
    }

    let idxs = even_anchor_indices(matched.len(), limit);
    let anchors: Vec<Value> = idxs
        .iter()
        .map(|&i| slim_anchor(matched[i]))
        .collect();

    json!({
        "matched_count": matched.len(),
        "anchor_count": anchors.len(),
        "start": first.recorded_at,
        "end": last.recorded_at,
        "duration_secs": duration_secs,
        "summary": Value::Object(summary),
        "anchors": anchors,
    })
}

fn metric_avg_stats<I>(iter: I) -> Option<Value>
where
    I: Iterator<Item = f64>,
{
    let vals: Vec<f64> = iter.filter(|v| v.is_finite()).collect();
    if vals.is_empty() {
        return None;
    }
    let min = vals.iter().cloned().fold(f64::INFINITY, f64::min);
    let max = vals.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let avg = vals.iter().sum::<f64>() / vals.len() as f64;
    Some(json!({ "min": min, "avg": avg, "max": max }))
}

fn metric_minmax_stats<I>(iter: I) -> Option<Value>
where
    I: Iterator<Item = f64>,
{
    let vals: Vec<f64> = iter.filter(|v| v.is_finite()).collect();
    if vals.is_empty() {
        return None;
    }
    let min = vals.iter().cloned().fold(f64::INFINITY, f64::min);
    let max = vals.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    Some(json!({ "min": min, "max": max }))
}

/// Evenly spaced indices in `0..n`, always including first and last when `limit >= 2`.
pub fn even_anchor_indices(n: usize, limit: usize) -> Vec<usize> {
    if n == 0 {
        return Vec::new();
    }
    let limit = limit.clamp(1, n);
    if limit == 1 {
        return vec![0];
    }
    let mut out = Vec::with_capacity(limit);
    for i in 0..limit {
        let idx = i * (n - 1) / (limit - 1);
        if out.last().copied() != Some(idx) {
            out.push(idx);
        }
    }
    // If rounding collapsed indices, backfill unused slots from remaining positions.
    if out.len() < limit {
        for idx in 0..n {
            if !out.contains(&idx) {
                out.push(idx);
                if out.len() == limit {
                    break;
                }
            }
        }
        out.sort_unstable();
    }
    out
}

fn slim_anchor(p: &SamplePoint) -> Value {
    json!({
        "recorded_at": p.recorded_at,
        "lat": p.lat,
        "lon": p.lon,
        "speed_kph": p.speed_kph,
        "rpm": p.rpm,
        "engine_load_pct": p.engine_load_pct,
        "fuel_rate_lph": p.fuel_rate_lph,
        "coolant_c": p.coolant_c,
    })
}

// --- evaluate_math ---

#[derive(Debug, Deserialize, Serialize)]
pub struct EvaluateMathArgs {
    /// Arithmetic / helper expression, e.g. `l_per_100km(1.2, 15)` or `fuel_l / dist_km * 100`.
    pub expression: String,
    /// Optional named numeric bindings usable inside the expression.
    ///
    /// Tolerant of common model mistakes: JSON-encoded object strings and numeric strings.
    /// Arithmetic must live in `expression`, not inside variable values.
    #[serde(default, deserialize_with = "deserialize_math_variables")]
    pub variables: BTreeMap<String, f64>,
}

/// Hint appended when `evaluate_math` args fail to parse (agent → model).
pub const EVALUATE_MATH_ARGS_HINT: &str = "evaluate_math expects {\"expression\":\"...\",\"variables\":{\"name\": number, ...}}. \
`variables` must be a JSON object of finite numbers (not a string). Put all arithmetic in `expression` \
(e.g. expression=\"a / (b * c)\", variables={\"a\":113,\"b\":0.42,\"c\":0.63}). Example: \
{\"expression\":\"hard_accel_events / moving_hours\",\"variables\":{\"hard_accel_events\":113,\"moving_hours\":0.27}}.";

/// Stateless safe math evaluator (free-form + trip helpers).
#[derive(Clone, Default)]
pub struct EvaluateMath;

impl Tool for EvaluateMath {
    const NAME: &'static str = "evaluate_math";
    type Error = ToolErr;
    type Args = EvaluateMathArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.into(),
            description: "Evaluate a safe math expression. Use for L/100km, MPG, unit conversions, and general arithmetic. Helpers: l_per_100km(liters,km), mpg_us(liters,km), kph_to_mph, mph_to_kph, km_to_mi, mi_to_km, m_to_mi, mi_to_m, l_to_gal_us, gal_us_to_l, seconds_to_hours, pow, log, sqrt, min, max, abs, ln, exp, floor, ceil, round. Optional `variables` is a JSON object mapping names to numbers only (never a stringified JSON blob; never put formulas in values — put math in `expression`). Returns JSON {expression, result, error}. Example args: {\"expression\":\"hard_accel_events / moving_hours\",\"variables\":{\"hard_accel_events\":113,\"moving_hours\":0.27}}.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "expression": {
                        "type": "string",
                        "description": "Math expression (max 500 chars). Put operators and formulas here (e.g. a/(b*c))."
                    },
                    "variables": {
                        "type": "object",
                        "description": "Optional name → number bindings only (plain JSON object, not a string). Values must be numbers, not expressions.",
                        "additionalProperties": { "type": "number" }
                    }
                },
                "required": ["expression"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let out = evaluate_expression(&args.expression, &args.variables);
        dump(&out)
    }
}

/// Deserialize `variables` with tolerance for model quirks:
/// - missing / null → empty map
/// - object with number or numeric-string values
/// - string containing a JSON object (double-encoded)
fn deserialize_math_variables<'de, D>(deserializer: D) -> Result<BTreeMap<String, f64>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<Value>::deserialize(deserializer)?;
    coerce_math_variables(value.unwrap_or(Value::Null)).map_err(D::Error::custom)
}

/// Public for tests / agent error shaping.
pub fn coerce_math_variables(value: Value) -> Result<BTreeMap<String, f64>, String> {
    match value {
        Value::Null => Ok(BTreeMap::new()),
        Value::Object(map) => object_to_f64_map(map),
        Value::String(s) => {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                return Ok(BTreeMap::new());
            }
            // Models often JSON-stringify the whole variables object.
            match serde_json::from_str::<Value>(trimmed) {
                Ok(Value::Object(map)) => object_to_f64_map(map),
                Ok(other) => Err(format!(
                    "`variables` string must decode to a JSON object of numbers, got {other}; \
                     put arithmetic in `expression`, not inside `variables`"
                )),
                Err(parse_err) => {
                    // Common failure: double-encoded object with bare expressions
                    // e.g. "{\"x\": 1, \"y\": 0.4 * 0.6}" — invalid JSON because of `*`.
                    if trimmed.starts_with('{')
                        && trimmed.contains(|c| matches!(c, '*' | '/'))
                    {
                        return Err(format!(
                            "`variables` was a string that is not valid JSON ({parse_err}). \
                             Likely a formula was embedded in a value (e.g. 0.4 * 0.6). \
                             Pass a real object of numbers and put math in `expression`, e.g. \
                             expression=\"a/(b*c)\", variables={{\"a\":1,\"b\":0.4,\"c\":0.6}}."
                        ));
                    }
                    Err(
                        "`variables` must be a JSON object of numbers, not a plain string. \
                         Do not stringify the map. Example: \"variables\": {\"x\": 1.5}. \
                         Put formulas in `expression`."
                            .into(),
                    )
                }
            }
        }
        other => Err(format!(
            "`variables` must be a JSON object of numbers (got {other}); \
             put arithmetic in `expression`"
        )),
    }
}

fn object_to_f64_map(
    map: serde_json::Map<String, Value>,
) -> Result<BTreeMap<String, f64>, String> {
    let mut out = BTreeMap::new();
    for (key, val) in map {
        out.insert(key.clone(), coerce_variable_number(&key, &val)?);
    }
    Ok(out)
}

fn coerce_variable_number(key: &str, val: &Value) -> Result<f64, String> {
    match val {
        Value::Number(n) => n.as_f64().filter(|v| v.is_finite()).ok_or_else(|| {
            format!("variable `{key}` is not a finite number")
        }),
        Value::String(s) => {
            let s = s.trim();
            if s.is_empty() {
                return Err(format!("variable `{key}` is an empty string"));
            }
            // Reject expressions stuffed into values (e.g. "0.42 * 0.63").
            if looks_like_math_expression(s) {
                return Err(format!(
                    "variable `{key}` value `{s}` looks like an expression; \
                     put math in `expression` and bind only plain numbers in `variables`"
                ));
            }
            s.parse::<f64>()
                .ok()
                .filter(|v| v.is_finite())
                .ok_or_else(|| {
                    format!(
                        "variable `{key}` value `{s}` is not a finite number; \
                         use a plain number (no units or formulas)"
                    )
                })
        }
        Value::Null => Err(format!(
            "variable `{key}` is null; omit it or pass a number"
        )),
        other => Err(format!(
            "variable `{key}` must be a number, got {other}"
        )),
    }
}

/// True when a string is not a plain numeric literal (optional leading sign / decimal / exponent)
/// but contains operators or parentheses that belong in `expression`.
fn looks_like_math_expression(s: &str) -> bool {
    // Plain numbers: 1, -2.5, .5, 1e-3, +3.0
    if s.parse::<f64>().is_ok() && !s.chars().any(|c| matches!(c, '*' | '/' | '(' | ')')) {
        // `parse` accepts "1e2" etc.; still reject if * / ( ) present.
        // Note: unary +/- is fine for parse; binary + or - mid-string is rare in valid f64.
        return false;
    }
    s.chars()
        .any(|c| matches!(c, '*' | '/' | '(' | ')' | '+' | '-'))
        && s.parse::<f64>().is_err()
}

// --- submit_analysis_report ---

#[derive(Clone)]
pub struct SubmitAnalysisReport {
    pub slot: ReportSlot,
}

impl Tool for SubmitAnalysisReport {
    const NAME: &'static str = "submit_analysis_report";
    type Error = ToolErr;
    type Args = AnalysisReport;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.into(),
            description: "Submit the final structured analysis report. Call exactly once when done. Summary must name places/road environments visited. Every claim across fields must include brief quantitative proof from tools (counts, ranges, durations) — no bare qualitative labels.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "summary": {
                        "type": "string",
                        "description": "Short executive summary: places/road types visited (from get_route_position_profile when available) PLUS key findings each with brief numbers (e.g. stop counts, max coolant °C, hard_brake_events). No unproven qualitative-only claims."
                    },
                    "mechanical_findings": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "title": { "type": "string", "description": "Short finding label" },
                                "evidence": {
                                    "type": "string",
                                    "description": "Required proof with tool metrics (min/avg/max, counts, durations, thresholds). Example: 'Coolant max 91°C (normal band); module voltage min 13.8 V while moving.'"
                                },
                                "severity": { "type": "string", "enum": ["low", "medium", "high"] },
                                "recommendation": { "type": "string" }
                            },
                            "required": ["title", "evidence"]
                        }
                    },
                    "driving_style": {
                        "type": "object",
                        "properties": {
                            "assessment": {
                                "type": "string",
                                "description": "Overall style with proof (e.g. 'Stop-heavy residential drive: 4 stops ≥60s, hard_accel=8, hard_brake=12, p95 speed 48 kph')."
                            },
                            "positives": {
                                "type": "array",
                                "items": {
                                    "type": "string",
                                    "description": "Positive with brief metric proof"
                                }
                            },
                            "improvements": {
                                "type": "array",
                                "items": {
                                    "type": "string",
                                    "description": "Improvement with brief metric proof (what happened + numbers)"
                                }
                            }
                        },
                        "required": ["assessment"]
                    },
                    "financial": {
                        "type": "object",
                        "properties": {
                            "fuel_used_note": {
                                "type": "string",
                                "description": "Fuel used with numbers from tools (volume, distance, rate)"
                            },
                            "efficiency_notes": {
                                "type": "string",
                                "description": "Efficiency assessment with computed L/100km or equivalent metrics"
                            },
                            "potential_savings": {
                                "type": "string",
                                "description": "Savings idea tied to measured behavior counts/rates; no invented prices"
                            },
                            "cost_estimate": { "type": ["number", "null"] }
                        }
                    },
                    "confidence": { "type": "string", "enum": ["low", "medium", "high"] },
                    "markdown": {
                        "type": "string",
                        "description": "Full narrative markdown; every factual claim must cite brief tool-backed numbers (counts, min/avg/max, durations, shares)."
                    }
                },
                "required": ["summary", "driving_style", "financial", "markdown"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        args.validate().map_err(ToolErr)?;
        let mut guard = self
            .slot
            .0
            .lock()
            .map_err(|_| ToolErr("report lock poisoned".into()))?;
        *guard = Some(args);
        Ok("report accepted".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::*;
    use chrono::TimeZone;

    fn sample_ctx() -> TripAnalysisContext {
        let t0 = Utc.with_ymd_and_hms(2026, 1, 1, 12, 0, 0).unwrap();
        TripAnalysisContext {
            overview: TripOverview {
                trip_id: "t1".into(),
                car_name: "Test".into(),
                make_model: Some("Car".into()),
                fuel_type: "E10".into(),
                started_at: Some(t0),
                finished_at: Some(t0 + chrono::Duration::minutes(30)),
                finished: true,
                point_count: 2,
                distance_m: Some(10_000.0),
                duration_secs: Some(1800.0),
                avg_speed_kph: Some(40.0),
                max_speed_kph: Some(80.0),
                fuel_used_l: Some(1.0),
                fuel_used_moving_l: Some(0.8),
                displacement_l: Some(1.0),
                stoich_afr: Some(14.0),
                density_gl: Some(745.0),
                ve: Some(0.85),
            },
            units: UnitLabels::metric(),
            speed: SpeedProfile {
                sample_count: 2,
                min_kph: Some(0.0),
                p50_kph: Some(40.0),
                p95_kph: Some(75.0),
                max_kph: Some(80.0),
                hard_accel_events: 0,
                hard_brake_events: 0,
                moving_share: Some(0.9),
            },
            engine: EngineStats::default(),
            fuel: FuelMixtureStats::default(),
            thermal: ThermalElectricalStats::default(),
            stops: StopSummary::default(),
            samples: vec![SamplePoint {
                recorded_at: t0,
                lat: Some(1.0),
                lon: Some(2.0),
                speed_kph: Some(40.0),
                rpm: Some(2000.0),
                engine_load_pct: None,
                fuel_rate_lph: None,
                coolant_c: None,
                voltage: None,
                stft_pct: None,
                ltft_pct: None,
                lambda: None,
                odometer_km: None,
                engine_on_time_s: None,
            }],
            prior_markdown: None,
            traffic: TrafficSummary::default(),
            route_positions: RoutePositionProfile::default(),
        }
    }

    #[tokio::test]
    async fn route_position_profile_tool_returns_payload() {
        let t0 = Utc.with_ymd_and_hms(2026, 1, 1, 12, 0, 0).unwrap();
        let mut ctx = sample_ctx();
        ctx.route_positions = RoutePositionProfile {
            available: true,
            step_pct: 5,
            samples: vec![crate::context::RoutePositionSample {
                pct: 0,
                recorded_at: t0,
                lat: Some(1.0),
                lon: Some(2.0),
                speed_kph: Some(15.0),
                osm_highway: Some("service".into()),
                position_type: "service_access".into(),
                maxspeed_kph: Some(20.0),
            }],
            type_counts: BTreeMap::from([("service_access".into(), 1)]),
            note: None,
        };
        let tool = GetRoutePositionProfile {
            ctx: CtxHandle(Arc::new(ctx)),
        };
        let out = tool.call(EmptyArgs {}).await.unwrap();
        assert!(out.contains("service_access"));
        assert!(out.contains("\"available\": true") || out.contains("\"available\":true"));
    }

    #[tokio::test]
    async fn traffic_summary_tool_returns_payload() {
        let mut ctx = sample_ctx();
        ctx.traffic = TrafficSummary {
            available: true,
            status: "ready".into(),
            overall_index: Some(0.42),
            time_share: Some(serde_json::json!({ "free": 0.5, "heavy": 0.2 })),
            distance_share: Some(serde_json::json!({ "free": 0.6, "heavy": 0.1 })),
            frame_count: 12,
        };
        let tool = GetTrafficSummary {
            ctx: CtxHandle(Arc::new(ctx)),
        };
        let out = tool.call(EmptyArgs {}).await.unwrap();
        assert!(out.contains("\"available\": true") || out.contains("\"available\":true"));
        assert!(out.contains("0.42"));
        assert!(out.contains("ready"));
    }

    #[tokio::test]
    async fn overview_tool_returns_json() {
        let ctx = CtxHandle(Arc::new(sample_ctx()));
        let tool = GetTripOverview { ctx };
        let out = tool.call(EmptyArgs {}).await.unwrap();
        assert!(out.contains("Test"));
        assert!(out.contains("distance_m"));
    }

    #[tokio::test]
    async fn evaluate_math_tool_works() {
        let tool = EvaluateMath;
        let out = tool
            .call(EvaluateMathArgs {
                expression: "l_per_100km(fuel_l, dist_km)".into(),
                variables: BTreeMap::from([
                    ("fuel_l".into(), 2.0),
                    ("dist_km".into(), 25.0),
                ]),
            })
            .await
            .unwrap();
        assert!(out.contains("8.0") || out.contains("\"result\": 8"));
        assert!(out.contains("\"error\": null") || out.contains("\"error\":null"));
    }

    #[test]
    fn coerce_variables_accepts_normal_object() {
        let v = json!({"hard_accel_events": 113, "moving_hours": 0.27});
        let map = coerce_math_variables(v).unwrap();
        assert_eq!(map.get("hard_accel_events"), Some(&113.0));
        assert_eq!(map.get("moving_hours"), Some(&0.27));
    }

    #[test]
    fn coerce_variables_accepts_json_object_string() {
        // Production failure mode: model double-stringifies the map.
        let inner = r#"{"hard_accel_events": 113, "hard_brake_events": 98, "moving_hours": 0.27}"#;
        let map = coerce_math_variables(Value::String(inner.into())).unwrap();
        assert_eq!(map.get("hard_accel_events"), Some(&113.0));
        assert_eq!(map.get("hard_brake_events"), Some(&98.0));
        assert_eq!(map.get("moving_hours"), Some(&0.27));
    }

    #[test]
    fn coerce_variables_accepts_numeric_strings_in_object() {
        let v = json!({"fuel_l": "2.0", "dist_km": "25"});
        let map = coerce_math_variables(v).unwrap();
        assert_eq!(map.get("fuel_l"), Some(&2.0));
        assert_eq!(map.get("dist_km"), Some(&25.0));
    }

    #[test]
    fn coerce_variables_rejects_expression_in_value() {
        let v = json!({"moving_hours": "0.426 * 0.637"});
        let err = coerce_math_variables(v).unwrap_err();
        assert!(err.contains("expression"), "err={err}");
        assert!(err.contains("moving_hours"), "err={err}");
    }

    #[test]
    fn coerce_variables_rejects_invalid_json_string_with_formula() {
        // Exact shape from production warn: stringified map + bare `*` (invalid JSON).
        let bad = r#"{"hard_accel_events": 113, "hard_brake_events": 98, "moving_hours": 0.4268847830555556 * 0.6371335504885993}"#;
        let err = coerce_math_variables(Value::String(bad.into())).unwrap_err();
        assert!(err.contains("expression") || err.contains("formula") || err.contains("not valid JSON"), "err={err}");
    }

    #[test]
    fn evaluate_math_args_deserializes_stringified_variables() {
        let raw = r#"{
            "expression": "hard_accel_events / moving_hours",
            "variables": "{\"hard_accel_events\": 113, \"moving_hours\": 0.27}"
        }"#;
        let args: EvaluateMathArgs = serde_json::from_str(raw).unwrap();
        assert_eq!(args.expression, "hard_accel_events / moving_hours");
        assert_eq!(args.variables.get("hard_accel_events"), Some(&113.0));
        assert_eq!(args.variables.get("moving_hours"), Some(&0.27));
    }

    #[test]
    fn evaluate_math_args_missing_variables_defaults_empty() {
        let raw = r#"{"expression": "1+1"}"#;
        let args: EvaluateMathArgs = serde_json::from_str(raw).unwrap();
        assert!(args.variables.is_empty());
    }

    fn sp(
        t0: DateTime<Utc>,
        offset_s: i64,
        speed: f64,
        rpm: f64,
        load: Option<f64>,
    ) -> SamplePoint {
        SamplePoint {
            recorded_at: t0 + chrono::Duration::seconds(offset_s),
            lat: Some(10.0 + offset_s as f64 * 0.001),
            lon: Some(20.0),
            speed_kph: Some(speed),
            rpm: Some(rpm),
            engine_load_pct: load,
            fuel_rate_lph: Some(speed / 20.0),
            coolant_c: Some(90.0),
            voltage: Some(14.0),
            stft_pct: Some(1.0),
            ltft_pct: Some(-2.0),
            lambda: Some(1.0),
            odometer_km: Some(100.0 + offset_s as f64),
            engine_on_time_s: Some(offset_s as f64),
        }
    }

    #[test]
    fn even_anchor_indices_includes_first_and_last() {
        let idxs = even_anchor_indices(20, 5);
        assert_eq!(idxs.first().copied(), Some(0));
        assert_eq!(idxs.last().copied(), Some(19));
        assert_eq!(idxs.len(), 5);
        // Monotonic unique
        for w in idxs.windows(2) {
            assert!(w[0] < w[1]);
        }
    }

    #[test]
    fn point_window_empty_range() {
        let t0 = Utc.with_ymd_and_hms(2026, 1, 1, 12, 0, 0).unwrap();
        let samples = vec![sp(t0, 0, 40.0, 2000.0, Some(30.0))];
        let out = build_point_window_payload(
            &samples,
            t0 + chrono::Duration::hours(1),
            t0 + chrono::Duration::hours(2),
            5,
        );
        assert_eq!(out["matched_count"], 0);
        assert_eq!(out["anchor_count"], 0);
        assert!(out["anchors"].as_array().unwrap().is_empty());
        assert!(out.get("points").is_none());
    }

    #[test]
    fn point_window_summary_and_slim_anchors() {
        let t0 = Utc.with_ymd_and_hms(2026, 1, 1, 12, 0, 0).unwrap();
        let samples: Vec<_> = (0..20)
            .map(|i| sp(t0, i, 10.0 + i as f64, 1500.0 + i as f64 * 50.0, Some(20.0 + i as f64)))
            .collect();
        // Request a huge limit — must clamp to max 8 anchors, not dump 20 fat points.
        let out = build_point_window_payload(
            &samples,
            t0,
            t0 + chrono::Duration::seconds(19),
            50,
        );
        assert_eq!(out["matched_count"], 20);
        assert_eq!(out["anchor_count"], MAX_POINT_WINDOW_ANCHORS);
        assert!(out.get("points").is_none(), "legacy raw points key must be gone");

        let speed = &out["summary"]["speed_kph"];
        assert_eq!(speed["min"], 10.0);
        assert_eq!(speed["max"], 29.0);
        let avg = speed["avg"].as_f64().unwrap();
        assert!((avg - 19.5).abs() < 1e-9, "avg={avg}");

        // trims are min/max only
        assert!(out["summary"]["stft_pct"].get("avg").is_none());
        assert_eq!(out["summary"]["stft_pct"]["min"], 1.0);

        let anchors = out["anchors"].as_array().unwrap();
        assert_eq!(anchors.len(), MAX_POINT_WINDOW_ANCHORS);
        // first/last preserved
        assert_eq!(
            anchors[0]["recorded_at"],
            json!(t0)
        );
        assert_eq!(
            anchors.last().unwrap()["recorded_at"],
            json!(t0 + chrono::Duration::seconds(19))
        );
        // slim: no odometer / voltage / trims / lambda on anchors
        let a0 = &anchors[0];
        assert!(a0.get("odometer_km").is_none());
        assert!(a0.get("voltage").is_none());
        assert!(a0.get("stft_pct").is_none());
        assert!(a0.get("lambda").is_none());
        assert!(a0.get("engine_on_time_s").is_none());
        assert!(a0.get("speed_kph").is_some());
        assert!(a0.get("rpm").is_some());
    }

    #[test]
    fn point_window_default_limit_is_five() {
        let raw = r#"{"start":"2026-01-01T12:00:00Z","end":"2026-01-01T13:00:00Z"}"#;
        let args: PointWindowArgs = serde_json::from_str(raw).unwrap();
        assert_eq!(args.limit, DEFAULT_POINT_WINDOW_ANCHORS);
    }

    #[tokio::test]
    async fn get_point_window_tool_returns_summary_shape() {
        let t0 = Utc.with_ymd_and_hms(2026, 1, 1, 12, 0, 0).unwrap();
        let mut ctx = sample_ctx();
        ctx.samples = (0..10).map(|i| sp(t0, i, 30.0, 2000.0, None)).collect();
        let tool = GetPointWindow {
            ctx: CtxHandle(Arc::new(ctx)),
        };
        let out = tool
            .call(PointWindowArgs {
                start: t0,
                end: t0 + chrono::Duration::seconds(9),
                limit: 5,
            })
            .await
            .unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["matched_count"], 10);
        assert_eq!(v["anchor_count"], 5);
        assert!(v["summary"]["speed_kph"]["avg"].is_number());
        assert_eq!(v["anchors"].as_array().unwrap().len(), 5);
        assert!(out.contains("summary"));
        assert!(!out.contains("\"points\""));
    }

    #[tokio::test]
    async fn submit_validates_and_stores() {
        let slot = ReportSlot::default();
        let tool = SubmitAnalysisReport { slot: slot.clone() };
        let report = AnalysisReport {
            summary: "ok".into(),
            mechanical_findings: vec![],
            driving_style: crate::report::DrivingStyle {
                assessment: "fine".into(),
                positives: vec![],
                improvements: vec![],
            },
            financial: crate::report::FinancialSection {
                fuel_used_note: "1L".into(),
                efficiency_notes: "".into(),
                potential_savings: "".into(),
                cost_estimate: None,
            },
            confidence: crate::report::Confidence::Medium,
            markdown: "# hi".into(),
        };
        tool.call(report).await.unwrap();
        assert!(slot.take().is_some());
    }
}
