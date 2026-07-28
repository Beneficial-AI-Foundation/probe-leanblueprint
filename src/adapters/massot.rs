//! Patrick Massot `leanblueprint` (LaTeX/plasTeX) adapter.
//!
//! Shells out to the bundled `scripts/blueprint_emit.py`, which reuses
//! leanblueprint's own plasTeX parser to emit normalized node/edge JSON, and
//! maps that into a [`BlueprintModel`]. The Python emitter needs plasTeX +
//! leanblueprint installed; no Lean build is required.

use std::collections::HashMap;
use std::path::Path;
use std::process::Command;

use serde::Deserialize;

use crate::error::BlueprintError;
use crate::model::{
    merge_node, BlueprintModel, BlueprintNode, NodeKind, ProofStatus, StatementStatus, StatusSource,
};

type Result<T> = std::result::Result<T, BlueprintError>;

#[derive(Debug, Deserialize)]
struct EmitterOutput {
    #[serde(default)]
    nodes: Vec<EmitNode>,
    #[serde(default)]
    edges: Vec<EmitEdge>,
}

#[derive(Debug, Deserialize)]
struct EmitNode {
    label: String,
    #[serde(default)]
    kind: String,
    #[serde(default)]
    lean_decls: Vec<String>,
    #[serde(default)]
    leanok: bool,
    #[serde(default)]
    mathlibok: bool,
    #[serde(default)]
    notready: bool,
    #[serde(default)]
    can_state: bool,
    #[serde(default)]
    can_prove: bool,
    #[serde(default)]
    proved: bool,
    #[serde(default)]
    fully_proved: bool,
    #[serde(default)]
    issue: Option<String>,
}

#[derive(Debug, Deserialize)]
struct EmitEdge {
    source: String,
    target: String,
    axis: String,
}

fn map_statement(n: &EmitNode) -> StatementStatus {
    if n.leanok || n.mathlibok {
        StatementStatus::Formalized
    } else if n.can_state {
        StatementStatus::Ready
    } else if n.notready {
        StatementStatus::Blocked
    } else {
        StatementStatus::NonePlanned
    }
}

fn map_proof(n: &EmitNode) -> ProofStatus {
    // `fully_proved` in leanblueprint counts definitions as vacuously done, so
    // gate the strongest state on `proved` to avoid over-claiming on defs.
    if n.proved && n.fully_proved {
        ProofStatus::FullyProved
    } else if n.proved {
        ProofStatus::Proved
    } else if n.can_prove {
        ProofStatus::Ready
    } else {
        ProofStatus::None
    }
}

/// Parse the emitter's JSON output into a [`BlueprintModel`].
pub fn parse_emitter_json(text: &str) -> Result<BlueprintModel> {
    let out: EmitterOutput = serde_json::from_str(text).map_err(BlueprintError::EmitterParse)?;

    // Reconstruct per-node uses from edges. Edge (source, target) means
    // `source` is used BY `target` (depgraph convention).
    let mut stmt_uses: HashMap<String, Vec<String>> = HashMap::new();
    let mut proof_uses: HashMap<String, Vec<String>> = HashMap::new();
    for e in &out.edges {
        let bucket = if e.axis == "proof" {
            &mut proof_uses
        } else {
            &mut stmt_uses
        };
        bucket
            .entry(e.target.clone())
            .or_default()
            .push(e.source.clone());
    }

    let mut model = BlueprintModel::default();
    // De-duplicate by label with the shared merge policy (like the Verso
    // adapter), so a repeated label in the emitter output is not double-counted.
    let mut index_by_label: HashMap<String, usize> = HashMap::new();
    for n in &out.nodes {
        let built = BlueprintNode {
            label: n.label.clone(),
            kind: Some(NodeKind::from_source(&n.kind)),
            lean_decls: n.lean_decls.clone(),
            statement_status: map_statement(n),
            proof_status: map_proof(n),
            // Massot's canonical mapping is lossless (no mathlib/incomplete
            // equivalent), so there is no raw status to preserve.
            source_statement_status: None,
            source_proof_status: None,
            statement_uses: stmt_uses.get(&n.label).cloned().unwrap_or_default(),
            proof_uses: proof_uses.get(&n.label).cloned().unwrap_or_default(),
            group: None,
            chapter: None,
            title: None,
            discussion: n.issue.clone(),
            status_source: StatusSource::Declared,
        };
        if let Some(&idx) = index_by_label.get(&built.label) {
            merge_node(&mut model.nodes[idx], built);
        } else {
            index_by_label.insert(built.label.clone(), model.nodes.len());
            model.nodes.push(built);
        }
    }
    Ok(model)
}

/// Run the bundled emitter over a blueprint `web.tex` and parse the result.
///
/// `python` is the interpreter to use (e.g. a venv's python); `script` is the
/// path to `blueprint_emit.py`; `web_tex` is the blueprint LaTeX entry point.
pub fn run(python: &str, script: &Path, web_tex: &Path) -> Result<BlueprintModel> {
    // plasTeX resolves `\input` paths relative to the working directory, so run
    // in the tex file's directory. That means the script and tex paths must be
    // absolute / cwd-independent.
    let script_abs = std::fs::canonicalize(script).map_err(|source| BlueprintError::Io {
        path: script.to_path_buf(),
        source,
    })?;
    let web_tex_abs = std::fs::canonicalize(web_tex).map_err(|source| BlueprintError::Io {
        path: web_tex.to_path_buf(),
        source,
    })?;
    let dir = web_tex_abs.parent().unwrap_or(Path::new("."));

    let output = Command::new(python)
        .arg(&script_abs)
        .arg(&web_tex_abs)
        .current_dir(dir)
        .output()
        .map_err(|source| BlueprintError::EmitterSpawn {
            python: python.to_string(),
            script: script_abs.clone(),
            source,
        })?;

    if !output.status.success() {
        return Err(BlueprintError::EmitterFailed {
            status: output.status.to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }
    let stdout = String::from_utf8(output.stdout).map_err(BlueprintError::EmitterNonUtf8)?;
    parse_emitter_json(&stdout)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_two_axes_and_uses() {
        let text = r#"{
          "nodes": [
            {"label":"def:foo","kind":"definition","lean_decls":["Foo.foo"],
             "leanok":true,"mathlibok":false,"notready":false,"can_state":true,
             "can_prove":false,"proved":false,"fully_proved":true,"issue":null},
            {"label":"thm:bar","kind":"theorem","lean_decls":["Foo.bar"],
             "leanok":false,"mathlibok":false,"notready":false,"can_state":true,
             "can_prove":true,"proved":true,"fully_proved":true,"issue":"42"},
            {"label":"thm:qux","kind":"theorem","lean_decls":[],
             "leanok":false,"mathlibok":false,"notready":false,"can_state":false,
             "can_prove":false,"proved":false,"fully_proved":false,"issue":null}
          ],
          "edges": [
            {"source":"def:foo","target":"thm:bar","axis":"statement"},
            {"source":"def:foo","target":"thm:bar","axis":"proof"}
          ]
        }"#;
        let model = parse_emitter_json(text).unwrap();
        assert_eq!(model.nodes.len(), 3);

        let foo = model.nodes.iter().find(|n| n.label == "def:foo").unwrap();
        assert_eq!(foo.statement_status, StatementStatus::Formalized);
        assert_eq!(foo.lean_decls, vec!["Foo.foo"]);

        let bar = model.nodes.iter().find(|n| n.label == "thm:bar").unwrap();
        assert_eq!(bar.proof_status, ProofStatus::FullyProved);
        assert_eq!(bar.statement_uses, vec!["def:foo"]);
        assert_eq!(bar.proof_uses, vec!["def:foo"]);
        assert_eq!(bar.discussion.as_deref(), Some("42"));

        let qux = model.nodes.iter().find(|n| n.label == "thm:qux").unwrap();
        assert!(qux.lean_decls.is_empty());
        assert_eq!(qux.statement_status, StatementStatus::NonePlanned);
        assert_eq!(qux.proof_status, ProofStatus::None);
    }

    #[test]
    fn duplicate_labels_are_deduped() {
        // A malformed/hand-crafted emitter output with a repeated label must not
        // double-count; statuses merge with the model's max policy.
        let text = r#"{
          "nodes": [
            {"label":"thm:x","kind":"theorem","lean_decls":["Foo.x"],
             "leanok":false,"notready":true,"can_state":false,
             "can_prove":false,"proved":false,"fully_proved":false},
            {"label":"thm:x","kind":"theorem","lean_decls":["Foo.x2"],
             "leanok":true,"notready":false,"can_state":true,
             "can_prove":true,"proved":true,"fully_proved":true}
          ],
          "edges": []
        }"#;
        let model = parse_emitter_json(text).unwrap();
        assert_eq!(
            model.nodes.len(),
            1,
            "duplicate label collapses to one node"
        );
        let n = &model.nodes[0];
        assert_eq!(n.lean_decls, vec!["Foo.x", "Foo.x2"], "decls set-unioned");
        assert_eq!(
            n.statement_status,
            StatementStatus::Formalized,
            "status max"
        );
        assert_eq!(n.proof_status, ProofStatus::FullyProved, "status max");
    }
}
