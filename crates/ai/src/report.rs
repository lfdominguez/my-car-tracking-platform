use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Low,
    Medium,
    High,
}

impl Default for Severity {
    fn default() -> Self {
        Self::Low
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Confidence {
    Low,
    Medium,
    High,
}

impl Default for Confidence {
    fn default() -> Self {
        Self::Medium
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MechanicalFinding {
    pub title: String,
    pub evidence: String,
    #[serde(default)]
    pub severity: Severity,
    #[serde(default)]
    pub recommendation: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DrivingStyle {
    pub assessment: String,
    #[serde(default)]
    pub positives: Vec<String>,
    #[serde(default)]
    pub improvements: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FinancialSection {
    #[serde(default)]
    pub fuel_used_note: String,
    #[serde(default)]
    pub efficiency_notes: String,
    #[serde(default)]
    pub potential_savings: String,
    /// Reserved; v1 leaves cost null unless a price was provided in context.
    #[serde(default)]
    pub cost_estimate: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalysisReport {
    pub summary: String,
    #[serde(default)]
    pub mechanical_findings: Vec<MechanicalFinding>,
    pub driving_style: DrivingStyle,
    pub financial: FinancialSection,
    #[serde(default)]
    pub confidence: Confidence,
    /// Full narrative markdown body for the trip UI.
    pub markdown: String,
}

impl AnalysisReport {
    pub fn validate(&self) -> Result<(), String> {
        if self.summary.trim().is_empty() {
            return Err("summary is empty".into());
        }
        if self.markdown.trim().is_empty() {
            return Err("markdown is empty".into());
        }
        if self.driving_style.assessment.trim().is_empty() {
            return Err("driving_style.assessment is empty".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_roundtrip_json() {
        let r = AnalysisReport {
            summary: "Smooth highway run".into(),
            mechanical_findings: vec![MechanicalFinding {
                title: "Coolant normal".into(),
                evidence: "Max 92C".into(),
                severity: Severity::Low,
                recommendation: "None".into(),
            }],
            driving_style: DrivingStyle {
                assessment: "Steady".into(),
                positives: vec!["Gentle accel".into()],
                improvements: vec![],
            },
            financial: FinancialSection {
                fuel_used_note: "1.2 L".into(),
                efficiency_notes: "Good".into(),
                potential_savings: "Minor".into(),
                cost_estimate: None,
            },
            confidence: Confidence::High,
            markdown: "## Summary\nOK".into(),
        };
        let v = serde_json::to_value(&r).unwrap();
        let back: AnalysisReport = serde_json::from_value(v).unwrap();
        assert_eq!(back.summary, r.summary);
        back.validate().unwrap();
    }
}
