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
    GetPointWindow, GetRoutePositionProfile, GetSpeedProfile, GetStopSummary,
    GetThermalElectricalStats, GetTrafficSummary, GetTripOverview, PointWindowArgs, ReportSlot,
    SubmitAnalysisReport,
};

const MAX_TURNS: usize = 24;
const MAX_TOKENS: u32 = 8192;
/// How many times we nudge the model after a text-only / empty turn without a valid report.
const MAX_SUBMIT_NUDGES: usize = 2;

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
    let mut submit_nudges: usize = 0;

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
                        // Feed structured errors so the model can correct messy args.
                        json!({
                            "error": e,
                            "hint": "Fix arguments and retry. submit_analysis_report requires a single JSON object matching the tool schema (no markdown fences)."
                        })
                        .to_string()
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

        // Text-only turn (no tools): try to recover a report, else nudge to submit.
        if let Some(text) = turn.content.clone() {
            last_text = Some(text.clone());
            messages.push(json!({ "role": "assistant", "content": text.clone() }));

            match try_parse_report_from_text(&text) {
                Ok(report) => {
                    warn!("report parsed from assistant text (submit tool skipped)");
                    return Ok(report);
                }
                Err(e) => {
                    warn!(error = %e, "assistant text is not a valid report");
                }
            }
        } else {
            warn!(
                turn = turn_idx,
                finish = turn.finish_reason.as_deref().unwrap_or("?"),
                "assistant returned empty content without tool calls"
            );
        }

        if submit_nudges < MAX_SUBMIT_NUDGES && turn_idx + 1 < MAX_TURNS {
            submit_nudges += 1;
            messages.push(json!({
                "role": "user",
                "content": "You must finish by calling the submit_analysis_report tool exactly once. Pass a single JSON object with fields: summary, mechanical_findings, driving_style, financial, confidence, markdown. Do not wrap the arguments in markdown fences or prose."
            }));
            continue;
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
    traffic: GetTrafficSummary,
    route_positions: GetRoutePositionProfile,
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
            traffic: GetTrafficSummary {
                ctx: handle.clone(),
            },
            route_positions: GetRoutePositionProfile {
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
            self.traffic.definition(String::new()).await,
            self.route_positions.definition(String::new()).await,
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
            GetTrafficSummary::NAME => self
                .traffic
                .call(parse_empty(args_raw)?)
                .await
                .map_err(|e| e.0),
            GetRoutePositionProfile::NAME => self
                .route_positions
                .call(parse_empty(args_raw)?)
                .await
                .map_err(|e| e.0),
            GetPointWindow::NAME => {
                let args: PointWindowArgs = parse_json_args(args_raw)?;
                self.points.call(args).await.map_err(|e| e.0)
            }
            EvaluateMath::NAME => {
                let args: EvaluateMathArgs = parse_json_args(args_raw)?;
                self.math.call(args).await.map_err(|e| e.0)
            }
            SubmitAnalysisReport::NAME => {
                let args: AnalysisReport = parse_json_args(args_raw)?;
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
    parse_json_args(args_raw)
}

/// Tolerate markdown fences / prose wrappers around tool argument JSON.
fn parse_json_args<T: serde::de::DeserializeOwned>(args_raw: &str) -> Result<T, String> {
    let cleaned = strip_code_fences(args_raw.trim());
    let candidate = extract_json_object(cleaned).unwrap_or(cleaned);
    serde_json::from_str(candidate).map_err(|e| format!("args: {e}"))
}

fn strip_code_fences(s: &str) -> &str {
    let trimmed = s.trim();
    let body = if let Some(rest) = trimmed.strip_prefix("```json") {
        rest
    } else if let Some(rest) = trimmed.strip_prefix("```JSON") {
        rest
    } else if let Some(rest) = trimmed.strip_prefix("```") {
        rest
    } else {
        return trimmed;
    };
    body.trim().trim_end_matches("```").trim()
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
    let body = strip_code_fences(text.trim());
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::Confidence;

    #[test]
    fn parse_json_args_strips_markdown_fence() {
        let raw = "```json\n{\"expression\":\"1+1\",\"variables\":{}}\n```";
        let args: EvaluateMathArgs = parse_json_args(raw).unwrap();
        assert_eq!(args.expression, "1+1");
    }

    #[test]
    fn try_parse_report_from_fenced_prose() {
        let text = concat!(
            "Here is the report:\n",
            "```json\n",
            "{\n",
            "  \"summary\": \"ok\",\n",
            "  \"mechanical_findings\": [],\n",
            "  \"driving_style\": {\"assessment\": \"calm\", \"positives\": [], \"improvements\": []},\n",
            "  \"financial\": {\"fuel_used_note\": \"\", \"efficiency_notes\": \"\", \"potential_savings\": \"\"},\n",
            "  \"confidence\": \"medium\",\n",
            "  \"markdown\": \"## Done\"\n",
            "}\n",
            "```\n",
        );
        let report = try_parse_report_from_text(text).unwrap();
        assert_eq!(report.summary, "ok");
        assert_eq!(report.confidence, Confidence::Medium);
        assert_eq!(report.driving_style.assessment, "calm");
    }
}
