//! Typed Rig tools over a prebuilt [`TripAnalysisContext`].

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::context::TripAnalysisContext;
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

// --- get_point_window ---

#[derive(Debug, Deserialize, Serialize)]
pub struct PointWindowArgs {
    /// ISO-8601 start (inclusive).
    pub start: DateTime<Utc>,
    /// ISO-8601 end (inclusive).
    pub end: DateTime<Utc>,
    /// Max points to return (capped server-side at 50).
    #[serde(default = "default_limit")]
    pub limit: usize,
}

fn default_limit() -> usize {
    30
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
            description: "Return up to `limit` downsampled samples between start and end (ISO-8601). Cap 50.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "start": { "type": "string", "description": "ISO-8601 start time" },
                    "end": { "type": "string", "description": "ISO-8601 end time" },
                    "limit": { "type": "integer", "description": "Max points (default 30, max 50)" }
                },
                "required": ["start", "end"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let limit = args.limit.clamp(1, 50);
        let mut pts: Vec<_> = self
            .ctx
            .0
            .samples
            .iter()
            .filter(|p| p.recorded_at >= args.start && p.recorded_at <= args.end)
            .cloned()
            .collect();
        if pts.len() > limit {
            // even sample
            let step = (pts.len() as f64 / limit as f64).ceil() as usize;
            pts = pts.into_iter().step_by(step.max(1)).take(limit).collect();
        }
        dump(&json!({ "count": pts.len(), "points": pts }))
    }
}

// --- evaluate_math ---

#[derive(Debug, Deserialize, Serialize)]
pub struct EvaluateMathArgs {
    /// Arithmetic / helper expression, e.g. `l_per_100km(1.2, 15)` or `fuel_l / dist_km * 100`.
    pub expression: String,
    /// Optional named numeric bindings usable inside the expression.
    #[serde(default)]
    pub variables: BTreeMap<String, f64>,
}

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
            description: "Evaluate a safe math expression. Use for L/100km, MPG, unit conversions, and general arithmetic. Helpers: l_per_100km(liters,km), mpg_us(liters,km), kph_to_mph, mph_to_kph, km_to_mi, mi_to_km, m_to_mi, mi_to_m, l_to_gal_us, gal_us_to_l, seconds_to_hours, pow, log, sqrt, min, max, abs, ln, exp, floor, ceil, round. Optional `variables` map binds names. Returns JSON {expression, result, error}.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "expression": {
                        "type": "string",
                        "description": "Math expression (max 500 chars)"
                    },
                    "variables": {
                        "type": "object",
                        "description": "Optional name → number bindings",
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
            description: "Submit the final structured analysis report. Call exactly once when done.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "summary": { "type": "string" },
                    "mechanical_findings": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "title": { "type": "string" },
                                "evidence": { "type": "string" },
                                "severity": { "type": "string", "enum": ["low", "medium", "high"] },
                                "recommendation": { "type": "string" }
                            },
                            "required": ["title", "evidence"]
                        }
                    },
                    "driving_style": {
                        "type": "object",
                        "properties": {
                            "assessment": { "type": "string" },
                            "positives": { "type": "array", "items": { "type": "string" } },
                            "improvements": { "type": "array", "items": { "type": "string" } }
                        },
                        "required": ["assessment"]
                    },
                    "financial": {
                        "type": "object",
                        "properties": {
                            "fuel_used_note": { "type": "string" },
                            "efficiency_notes": { "type": "string" },
                            "potential_savings": { "type": "string" },
                            "cost_estimate": { "type": ["number", "null"] }
                        }
                    },
                    "confidence": { "type": "string", "enum": ["low", "medium", "high"] },
                    "markdown": { "type": "string", "description": "Full narrative markdown" }
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
        }
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
