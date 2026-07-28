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

/// A single blueprint input used for this run, recorded so a summary can
/// substantiate which render it was based on.
#[derive(Debug, Clone, Serialize)]
pub struct ManifestRef {
    pub path: String,
    pub sha256: String,
    #[serde(
        rename = "vbp-internal-schema-version",
        skip_serializing_if = "Option::is_none"
    )]
    pub vbp_internal_schema_version: Option<u64>,
}

/// Which blueprint inputs produced this output. Pairs the code-atom identity
/// (`source`) with the blueprint side, so an output isn't left unable to say
/// which manifest/render it came from.
#[derive(Debug, Clone, Serialize, Default)]
pub struct BlueprintProvenance {
    pub adapter: String,
    #[serde(rename = "manifests", skip_serializing_if = "Vec::is_empty")]
    pub manifests: Vec<ManifestRef>,
    #[serde(rename = "web-tex", skip_serializing_if = "Option::is_none")]
    pub web_tex: Option<String>,
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
    #[serde(rename = "blueprint-provenance")]
    pub blueprint_provenance: BlueprintProvenance,
    pub timestamp: String,
    pub data: Summary,
}

pub fn build_summary_envelope(
    summary: Summary,
    source: Source,
    provenance: BlueprintProvenance,
) -> SummaryEnvelope {
    SummaryEnvelope {
        schema: SUMMARY_SCHEMA.to_string(),
        schema_version: SCHEMA_VERSION.to_string(),
        tool: tool(),
        source,
        blueprint_provenance: provenance,
        timestamp: now(),
        data: summary,
    }
}
