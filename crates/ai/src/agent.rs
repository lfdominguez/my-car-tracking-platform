//! OpenRouter multi-turn analysis agent (tolerant HTTP client + local tools).

use std::sync::{Arc, Mutex};

use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde_json::{json, Value};
use tracing::{info, warn};

use crate::context::TripAnalysisContext;
use crate::error::AiError;
use crate::openrouter::{OpenRouterClient, ToolCall};
use crate::prompt::{SYSTEM_PREAMBLE, USER_TASK};
use crate::report::AnalysisReport;
use crate::tools::{
    CtxHandle, EmptyArgs, EvaluateMath, EvaluateMathArgs, GetEngineStats, GetFuelMixtureStats,
    GetPointWindow, GetSpeedProfile, GetStopSummary, GetThermalElectricalStats, GetTripOverview,
    PointWindowArgs, ReportSlot, SubmitAnalysisReport,
};

const MAX_TURNS: usize = 24;
const MAX_TOKENS: u32 = 8192;

/// Run a multi-turn tool-using analysis against OpenRouter.
pub async fn analyze_trip(
    api_key: &str,
    model: &str,
    ctx: TripAnalysisContext,
) -> Result<AnalysisReport, AiError> {
    let model = model.trim();
    if model.is_empty() {
        return Err(AiError::Agent("model id is empty".into()));
    }
    if api_key.trim().is_empty() {
        return Err(AiError::Agent("api key is empty".into()));
    }

    let handle = CtxHandle(Arc::new(ctx));
    let slot = ReportSlot(Arc::new(Mutex::new(None)));
    let tools = ToolBundle::new(handle, slot.clone());
    let tool_defs = tools.openai_tools().await;

    let client = OpenRouterClient::new(api_key)?;

    let mut messages: Vec<Value> = vec![
        json!({ "role": "system", "content": SYSTEM_PREAMBLE }),
        json!({ "role": "user", "content": USER_TASK }),
    ];

    info!(%model, tools = tool_defs.len(), "starting openrouter trip analysis");

    let mut last_text: Option<String> = None;

    for turn_idx in 0..MAX_TURNS {
        let turn = client
            .chat_completion(model, &messages, &tool_defs, MAX_TOKENS)
            .await?;

        if !turn.tool_calls.is_empty() {
            messages.push(assistant_tool_call_message(&turn.tool_calls, turn.content.as_deref()));

            for tc in &turn.tool_calls {
                info!(turn = turn_idx, tool = %tc.name, "agent tool call");
                let result = tools.dispatch(&tc.name, &tc.arguments).await;
                let content = match result {
                    Ok(s) => s,
                    Err(e) => {
                        warn!(tool = %tc.name, error = %e, "tool failed");
                        json!({ "error": e }).to_string()
                    }
                };
                messages.push(json!({
                    "role": "tool",
                    "tool_call_id": tc.id,
                    "name": tc.name,
                    "content": content,
                }));

                if tc.name == SubmitAnalysisReport::NAME {
                    if let Some(report) = slot.take() {
                        if report.validate().is_ok() {
                            info!(turns = turn_idx + 1, "analysis report submitted");
                            return Ok(report);
                        }
                    }
                }
            }
            continue;
        }

        // Final text turn (no tools)
        if let Some(text) = turn.content.clone() {
            last_text = Some(text.clone());
            messages.push(json!({ "role": "assistant", "content": text }));
        }
        break;
    }

    if let Some(report) = slot.take() {
        if report.validate().is_ok() {
            return Ok(report);
        }
    }

    if let Some(text) = last_text {
        match try_parse_report_from_text(&text) {
            Ok(report) => {
                warn!("report parsed from assistant text (submit tool skipped)");
                return Ok(report);
            }
            Err(e) => {
                warn!(error = %e, "failed to parse assistant text as report");
            }
        }
    }

    Err(AiError::MissingReport)
}

struct ToolBundle {
    overview: GetTripOverview,
    speed: GetSpeedProfile,
    engine: GetEngineStats,
    fuel: GetFuelMixtureStats,
    thermal: GetThermalElectricalStats,
    stops: GetStopSummary,
    points: GetPointWindow,
    math: EvaluateMath,
    submit: SubmitAnalysisReport,
}

impl ToolBundle {
    fn new(handle: CtxHandle, slot: ReportSlot) -> Self {
        Self {
            overview: GetTripOverview {
                ctx: handle.clone(),
            },
            speed: GetSpeedProfile {
                ctx: handle.clone(),
            },
            engine: GetEngineStats {
                ctx: handle.clone(),
            },
            fuel: GetFuelMixtureStats {
                ctx: handle.clone(),
            },
            thermal: GetThermalElectricalStats {
                ctx: handle.clone(),
            },
            stops: GetStopSummary {
                ctx: handle.clone(),
            },
            points: GetPointWindow {
                ctx: handle.clone(),
            },
            math: EvaluateMath,
            submit: SubmitAnalysisReport { slot },
        }
    }

    async fn openai_tools(&self) -> Vec<Value> {
        let defs = [
            self.overview.definition(String::new()).await,
            self.speed.definition(String::new()).await,
            self.engine.definition(String::new()).await,
            self.fuel.definition(String::new()).await,
            self.thermal.definition(String::new()).await,
            self.stops.definition(String::new()).await,
            self.points.definition(String::new()).await,
            self.math.definition(String::new()).await,
            self.submit.definition(String::new()).await,
        ];
        defs.into_iter().map(tool_def_to_openai).collect()
    }

    async fn dispatch(&self, name: &str, arguments: &str) -> Result<String, String> {
        let args_raw = if arguments.trim().is_empty() {
            "{}"
        } else {
            arguments
        };

        match name {
            GetTripOverview::NAME => self
                .overview
                .call(parse_empty(args_raw)?)
                .await
                .map_err(|e| e.0),
            GetSpeedProfile::NAME => self
                .speed
                .call(parse_empty(args_raw)?)
                .await
                .map_err(|e| e.0),
            GetEngineStats::NAME => self
                .engine
                .call(parse_empty(args_raw)?)
                .await
                .map_err(|e| e.0),
            GetFuelMixtureStats::NAME => self
                .fuel
                .call(parse_empty(args_raw)?)
                .await
                .map_err(|e| e.0),
            GetThermalElectricalStats::NAME => self
                .thermal
                .call(parse_empty(args_raw)?)
                .await
                .map_err(|e| e.0),
            GetStopSummary::NAME => self
                .stops
                .call(parse_empty(args_raw)?)
                .await
                .map_err(|e| e.0),
            GetPointWindow::NAME => {
                let args: PointWindowArgs =
                    serde_json::from_str(args_raw).map_err(|e| format!("args: {e}"))?;
                self.points.call(args).await.map_err(|e| e.0)
            }
            EvaluateMath::NAME => {
                let args: EvaluateMathArgs =
                    serde_json::from_str(args_raw).map_err(|e| format!("args: {e}"))?;
                self.math.call(args).await.map_err(|e| e.0)
            }
            SubmitAnalysisReport::NAME => {
                let args: AnalysisReport =
                    serde_json::from_str(args_raw).map_err(|e| format!("args: {e}"))?;
                self.submit.call(args).await.map_err(|e| e.0)
            }
            other => Err(format!("unknown tool: {other}")),
        }
    }
}

fn tool_def_to_openai(def: ToolDefinition) -> Value {
    json!({
        "type": "function",
        "function": {
            "name": def.name,
            "description": def.description,
            "parameters": def.parameters,
        }
    })
}

fn parse_empty(args_raw: &str) -> Result<EmptyArgs, String> {
    if args_raw.trim().is_empty() || args_raw.trim() == "null" {
        return Ok(EmptyArgs {});
    }
    serde_json::from_str(args_raw).map_err(|e| format!("args: {e}"))
}

fn assistant_tool_call_message(tool_calls: &[ToolCall], content: Option<&str>) -> Value {
    let calls: Vec<Value> = tool_calls
        .iter()
        .map(|tc| {
            json!({
                "id": tc.id,
                "type": "function",
                "function": {
                    "name": tc.name,
                    "arguments": tc.arguments,
                }
            })
        })
        .collect();

    let mut msg = json!({
        "role": "assistant",
        "tool_calls": calls,
    });
    // OpenRouter/OpenAI accept null or omit; include content when present.
    if let Some(c) = content {
        msg["content"] = Value::String(c.to_string());
    } else {
        msg["content"] = Value::Null;
    }
    msg
}

fn try_parse_report_from_text(text: &str) -> Result<AnalysisReport, AiError> {
    let trimmed = text.trim();
    // strip markdown fence if present
    let body = if let Some(rest) = trimmed.strip_prefix("```json") {
        rest.trim_end_matches("```").trim()
    } else if let Some(rest) = trimmed.strip_prefix("```") {
        rest.trim_end_matches("```").trim()
    } else {
        trimmed
    };
    // Some models wrap JSON in prose — try to find the outermost object.
    let candidate = extract_json_object(body).unwrap_or(body);
    let report: AnalysisReport =
        serde_json::from_str(candidate).map_err(|e| AiError::InvalidReport(e.to_string()))?;
    report.validate().map_err(AiError::InvalidReport)?;
    Ok(report)
}

fn extract_json_object(s: &str) -> Option<&str> {
    let start = s.find('{')?;
    let end = s.rfind('}')?;
    if end > start {
        Some(&s[start..=end])
    } else {
        None
    }
}
