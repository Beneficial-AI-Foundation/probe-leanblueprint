//! Build the output envelopes: `probe-leanblueprint/extract` (enriched atoms)
//! and the `probe-leanblueprint/summary` sidecar (two-axis progress counts).

use std::collections::BTreeMap;

use probe::types::{Atom, AtomEnvelope, Source, Tool};
use serde::Serialize;

use crate::enrich::Summary;

pub const TOOL_NAME: &str = "probe-leanblueprint";
// 2.1 adds the optional `blueprint-source-*-status` fields (raw Verso status
// preserved when the canonical enum is lossy) and the machine-confirmed headline
// counts. Both are additive, so 2.0 consumers keep working.
pub const SCHEMA_VERSION: &str = "2.1";
pub const EXTRACT_SCHEMA: &str = "probe-leanblueprint/extract";
pub const SUMMARY_SCHEMA: &str = "probe-leanblueprint/summary";

fn tool() -> Tool {
    Tool {
        name: TOOL_NAME.to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        command: "extract".to_string(),
    }
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// Build the enriched atom envelope.
pub fn build_extract_envelope(atoms: BTreeMap<String, Atom>, source: Source) -> AtomEnvelope {
    AtomEnvelope {
        schema: EXTRACT_SCHEMA.to_string(),
        schema_version: SCHEMA_VERSION.to_string(),
        tool: tool(),
        source,
        timestamp: now(),
        data: atoms,
    }
}

/// The summary sidecar envelope (kept separate from the atoms category so it is
/// never merged; it carries the meaningful two-axis progress stats).
#[derive(Debug, Serialize)]
pub struct SummaryEnvelope {
    pub schema: String,
    #[serde(rename = "schema-version")]
    pub schema_version: String,
    pub tool: Tool,
    pub source: Source,
    pub timestamp: String,
    pub data: Summary,
}

pub fn build_summary_envelope(summary: Summary, source: Source) -> SummaryEnvelope {
    SummaryEnvelope {
        schema: SUMMARY_SCHEMA.to_string(),
        schema_version: SCHEMA_VERSION.to_string(),
        tool: tool(),
        source,
        timestamp: now(),
        data: summary,
    }
}
