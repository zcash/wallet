use serde::{Deserialize, Serialize};
use crate::engine::ParityResult;
use std::collections::HashMap;

/// The final report structure for a parity run.
#[derive(Debug, Serialize, Deserialize)]
pub struct FinalReport {
    pub summary: RunSummary,
    pub details: HashMap<String, ParityResultReport>,
}

/// Counts for the run summary.
#[derive(Debug, Serialize, Deserialize)]
pub struct RunSummary {
    pub total: usize,
    pub matches: usize,
    pub diffs: usize,
    pub missing: usize,
    pub errors: usize,
}

/// The serialized form of a single method's parity result.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ParityResultReport {
    Match,
    Diff {
        diff_message: String,
    },
    Missing {
        method: String,
    },
    Error {
        message: String,
    },
}

impl FinalReport {
    pub fn new(results: Vec<(String, ParityResult)>) -> Self {
        let mut matches = 0;
        let mut diffs = 0;
        let mut missing = 0;
        let mut errors = 0;
        let mut details = HashMap::new();

        for (method, res) in results {
            let report_res = match res {
                ParityResult::Match => {
                    matches += 1;
                    ParityResultReport::Match
                }
                ParityResult::Diff { diff_message, .. } => {
                    diffs += 1;
                    ParityResultReport::Diff { diff_message }
                }
                ParityResult::Missing { method: ref m } => {
                    missing += 1;
                    ParityResultReport::Missing { method: m.clone() }
                }
                ParityResult::Error(message) => {
                    errors += 1;
                    ParityResultReport::Error { message }
                }
            };
            details.insert(method, report_res);
        }

        Self {
            summary: RunSummary {
                total: details.len(),
                matches,
                diffs,
                missing,
                errors,
            },
            details,
        }
    }

    pub fn to_markdown(&self) -> String {
        let mut md = String::from("# Zallet Parity Report\n\n");
        md.push_str(&format!("- **Total Tests**: {}\n", self.summary.total));
        md.push_str(&format!("- **✅ Matches**: {}\n", self.summary.matches));
        md.push_str(&format!("- **❌ Diffs**: {}\n", self.summary.diffs));
        md.push_str(&format!("- **🔍 Missing**: {}\n", self.summary.missing));
        md.push_str(&format!("- **⚠️ Errors**: {}\n\n", self.summary.errors));

        md.push_str("## Detailed Results\n\n");
        md.push_str("| Method | Status | Notes |\n");
        md.push_str("| :--- | :--- | :--- |\n");

        let mut sorted_details: Vec<_> = self.details.iter().collect();
        sorted_details.sort_by_key(|(k, _)| *k);

        for (method, res) in sorted_details {
            let (status, notes) = match res {
                ParityResultReport::Match => ("✅ Match", String::new()),
                ParityResultReport::Diff { diff_message } => ("❌ Diff", diff_message.clone()),
                ParityResultReport::Missing { method: m } => {
                    ("🔍 Missing", format!("Method `{}` not found on one endpoint", m))
                }
                ParityResultReport::Error { message } => ("⚠️ Error", message.clone()),
            };
            md.push_str(&format!(
                "| `{}` | {} | {} |\n",
                method,
                status,
                notes.replace('\n', "<br>")
            ));
        }

        md
    }
}
