use crate::differ::DiffEntry;
use crate::engine::ParityResult;
use serde::{Deserialize, Serialize};
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
    pub expected_diffs: usize,
    pub missing: usize,
    pub errors: usize,
}

/// The serialized form of a single method's parity result.
///
/// The `type` tag distinguishes the variant in JSON:
/// `"match"`, `"diff"`, `"expected_diff"`, `"missing"`, `"error"`
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ParityResultReport {
    Match,
    /// Unexpected diff — represents a real compatibility gap.
    Diff {
        /// Number of leaf-level differences found.
        diff_count: usize,
        /// JSON Pointer paths where differences were found.
        diff_paths: Vec<String>,
    },
    /// Known/intentional diff — visible in the report but not a blocker.
    ExpectedDiff {
        diff_count: usize,
        diff_paths: Vec<String>,
        /// Human-readable explanation from the expected-diffs file.
        reason: String,
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
        let mut matches = 0usize;
        let mut diffs = 0usize;
        let mut expected_diffs = 0usize;
        let mut missing = 0usize;
        let mut errors = 0usize;
        let mut details = HashMap::new();

        for (method, res) in results {
            let report_res = match res {
                ParityResult::Match => {
                    matches += 1;
                    ParityResultReport::Match
                }
                ParityResult::Diff { diff_entries } => {
                    diffs += 1;
                    let diff_paths: Vec<String> =
                        diff_entries.iter().map(|e| e.path.clone()).collect();
                    ParityResultReport::Diff {
                        diff_count: diff_paths.len(),
                        diff_paths,
                    }
                }
                ParityResult::ExpectedDiff {
                    diff_entries,
                    reason,
                } => {
                    expected_diffs += 1;
                    let diff_paths: Vec<String> =
                        diff_entries.iter().map(|e| e.path.clone()).collect();
                    ParityResultReport::ExpectedDiff {
                        diff_count: diff_paths.len(),
                        diff_paths,
                        reason,
                    }
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
                expected_diffs,
                missing,
                errors,
            },
            details,
        }
    }

    /// Returns the raw `DiffEntry` objects for a given method (for verbose/debug output).
    pub fn with_diff_detail(
        results: Vec<(String, ParityResult)>,
    ) -> (Self, HashMap<String, Vec<DiffEntry>>) {
        let mut raw_diffs: HashMap<String, Vec<DiffEntry>> = HashMap::new();
        let mapped: Vec<(String, ParityResult)> = results
            .into_iter()
            .map(|(method, res)| {
                match &res {
                    ParityResult::Diff { diff_entries }
                    | ParityResult::ExpectedDiff { diff_entries, .. } => {
                        raw_diffs.insert(method.clone(), diff_entries.clone());
                    }
                    _ => {}
                }
                (method, res)
            })
            .collect();
        (Self::new(mapped), raw_diffs)
    }

    pub fn to_markdown(&self) -> String {
        let mut md = String::from("# Zallet Parity Report\n\n");
        md.push_str(&format!("- **Total Tests**: {}\n", self.summary.total));
        md.push_str(&format!("- **✅ Matches**: {}\n", self.summary.matches));
        md.push_str(&format!("- **❌ Diffs**: {}\n", self.summary.diffs));
        md.push_str(&format!(
            "- **📋 Expected Diffs**: {}\n",
            self.summary.expected_diffs
        ));
        md.push_str(&format!("- **🔍 Missing**: {}\n", self.summary.missing));
        md.push_str(&format!("- **⚠️ Errors**: {}\n\n", self.summary.errors));

        md.push_str("## Detailed Results\n\n");
        md.push_str("| Method | Status | Details |\n");
        md.push_str("| :--- | :--- | :--- |\n");

        let mut sorted_details: Vec<_> = self.details.iter().collect();
        sorted_details.sort_by_key(|(k, _)| *k);

        for (method, res) in sorted_details {
            let (status, notes) = match res {
                ParityResultReport::Match => ("✅ Match", String::new()),
                ParityResultReport::Diff {
                    diff_count,
                    diff_paths,
                } => {
                    let paths = diff_paths.join(", ");
                    (
                        "❌ Diff",
                        format!("{} field(s) differ: `{}`", diff_count, paths),
                    )
                }
                ParityResultReport::ExpectedDiff {
                    diff_count,
                    diff_paths,
                    reason,
                } => {
                    let paths = diff_paths.join(", ");
                    (
                        "📋 Expected Diff",
                        format!("{} field(s): `{}` — _{}_", diff_count, paths, reason),
                    )
                }
                ParityResultReport::Missing { method: m } => (
                    "🔍 Missing",
                    format!("Method `{}` not found on one endpoint", m),
                ),
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
