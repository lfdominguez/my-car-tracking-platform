//! Safe mathematical expression evaluation for the analysis agent.

use std::collections::BTreeMap;

use meval::{Context, Expr};
use serde::{Deserialize, Serialize};

/// Maximum accepted expression length (characters).
pub const MAX_EXPRESSION_LEN: usize = 500;

const M_PER_MI: f64 = 1609.344;
const KM_PER_MI: f64 = 1.609_344;
const L_PER_US_GAL: f64 = 3.785_411_784;
/// US MPG from L/100km: `235.214583 / (L/100km)`.
const MPG_US_FROM_L100: f64 = 235.214_583;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MathEvalResult {
    pub expression: String,
    pub result: Option<f64>,
    pub error: Option<String>,
}

/// Evaluate a free-form expression with optional variable bindings and trip helpers.
pub fn evaluate_expression(
    expression: &str,
    variables: &BTreeMap<String, f64>,
) -> MathEvalResult {
    let expression = expression.trim();
    if expression.is_empty() {
        return MathEvalResult {
            expression: expression.to_string(),
            result: None,
            error: Some("expression is empty".into()),
        };
    }
    if expression.len() > MAX_EXPRESSION_LEN {
        return MathEvalResult {
            expression: expression.to_string(),
            result: None,
            error: Some(format!(
                "expression too long (max {MAX_EXPRESSION_LEN} characters)"
            )),
        };
    }

    for name in variables.keys() {
        if !is_safe_ident(name) {
            return MathEvalResult {
                expression: expression.to_string(),
                result: None,
                error: Some(format!("invalid variable name: {name}")),
            };
        }
    }

    let mut ctx = build_context();
    for (name, value) in variables {
        if !value.is_finite() {
            return MathEvalResult {
                expression: expression.to_string(),
                result: None,
                error: Some(format!("variable `{name}` is not a finite number")),
            };
        }
        ctx.var(name.as_str(), *value);
    }

    let expr: Expr = match expression.parse() {
        Ok(e) => e,
        Err(e) => {
            return MathEvalResult {
                expression: expression.to_string(),
                result: None,
                error: Some(format!("parse error: {e}")),
            };
        }
    };

    match expr.eval_with_context(ctx) {
        Ok(v) if v.is_finite() => MathEvalResult {
            expression: expression.to_string(),
            result: Some(v),
            error: None,
        },
        Ok(v) => MathEvalResult {
            expression: expression.to_string(),
            result: None,
            error: Some(format!("non-finite result: {v}")),
        },
        Err(e) => MathEvalResult {
            expression: expression.to_string(),
            result: None,
            error: Some(e.to_string()),
        },
    }
}

fn is_safe_ident(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn build_context<'a>() -> Context<'a> {
    let mut ctx = Context::new(); // pi, e, sqrt, exp, ln, abs, min, max, floor, ceil, round, …

    // log10 alias (meval has ln but not log)
    ctx.func("log", f64::log10);
    ctx.func2("pow", f64::powf);

    // Trip / unit helpers
    ctx.func2("l_per_100km", |liters, km| {
        if km > 0.0 && liters.is_finite() {
            liters / km * 100.0
        } else {
            f64::NAN
        }
    });
    ctx.func2("mpg_us", |liters, km| {
        if km > 0.0 && liters > 0.0 {
            let l100 = liters / km * 100.0;
            MPG_US_FROM_L100 / l100
        } else {
            f64::NAN
        }
    });
    ctx.func("kph_to_mph", |v| v * KM_PER_MI.recip()); // * 0.621371192
    ctx.func("mph_to_kph", |v| v * KM_PER_MI);
    ctx.func("km_to_mi", |d| d / KM_PER_MI);
    ctx.func("mi_to_km", |d| d * KM_PER_MI);
    ctx.func("m_to_mi", |m| m / M_PER_MI);
    ctx.func("mi_to_m", |mi| mi * M_PER_MI);
    ctx.func("l_to_gal_us", |l| l / L_PER_US_GAL);
    ctx.func("gal_us_to_l", |g| g * L_PER_US_GAL);
    ctx.func("seconds_to_hours", |s| s / 3600.0);

    ctx
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arithmetic() {
        let r = evaluate_expression("2 + 3 * 4", &BTreeMap::new());
        assert_eq!(r.result, Some(14.0));
        assert!(r.error.is_none());
    }

    #[test]
    fn variables() {
        let mut vars = BTreeMap::new();
        vars.insert("fuel_l".into(), 1.2);
        vars.insert("dist_km".into(), 15.0);
        let r = evaluate_expression("fuel_l / dist_km * 100", &vars);
        assert!((r.result.unwrap() - 8.0).abs() < 1e-9);
    }

    #[test]
    fn l_per_100km_helper() {
        let r = evaluate_expression("l_per_100km(1.2, 15)", &BTreeMap::new());
        assert!((r.result.unwrap() - 8.0).abs() < 1e-9);
    }

    #[test]
    fn mpg_us_helper() {
        // 1 L / 100 km ≈ 235.214583 MPG
        let r = evaluate_expression("mpg_us(1, 100)", &BTreeMap::new());
        assert!((r.result.unwrap() - MPG_US_FROM_L100).abs() < 1e-6);
    }

    #[test]
    fn unit_helpers() {
        let r = evaluate_expression("kph_to_mph(160.9344)", &BTreeMap::new());
        assert!((r.result.unwrap() - 100.0).abs() < 1e-6);
        let r = evaluate_expression("l_to_gal_us(3.785411784)", &BTreeMap::new());
        assert!((r.result.unwrap() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn div_by_zero_non_finite() {
        let r = evaluate_expression("1 / 0", &BTreeMap::new());
        // meval may yield inf → we reject non-finite
        assert!(r.result.is_none());
        assert!(r.error.is_some());
    }

    #[test]
    fn l_per_100km_zero_distance() {
        let r = evaluate_expression("l_per_100km(1, 0)", &BTreeMap::new());
        assert!(r.result.is_none());
        assert!(r.error.is_some());
    }

    #[test]
    fn unknown_var() {
        let r = evaluate_expression("missing + 1", &BTreeMap::new());
        assert!(r.result.is_none());
        assert!(r.error.is_some());
    }

    #[test]
    fn oversized() {
        let expr = "1+".repeat(300) + "1";
        let r = evaluate_expression(&expr, &BTreeMap::new());
        assert!(r.result.is_none());
        assert!(r
            .error
            .as_deref()
            .unwrap_or("")
            .contains("too long"));
    }

    #[test]
    fn invalid_variable_name() {
        let mut vars = BTreeMap::new();
        vars.insert("1bad".into(), 1.0);
        let r = evaluate_expression("1", &vars);
        assert!(r.error.unwrap().contains("invalid variable"));
    }

    #[test]
    fn pow_and_log() {
        let r = evaluate_expression("pow(2, 10)", &BTreeMap::new());
        assert_eq!(r.result, Some(1024.0));
        let r = evaluate_expression("log(1000)", &BTreeMap::new());
        assert!((r.result.unwrap() - 3.0).abs() < 1e-9);
    }
}
