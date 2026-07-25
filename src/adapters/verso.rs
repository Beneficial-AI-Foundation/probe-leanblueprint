//! Verso Blueprint adapter.
//!
//! Reads a `blueprint-manifest.json` produced by the `versoBlueprint` renderer
//! and maps its graph nodes + Lean-decl bindings into a [`BlueprintModel`].
//! No parsing of our own — the manifest is already machine-readable.

use std::collections::HashMap;
use std::path::Path;

use serde::Deserialize;

use crate::error::BlueprintError;
use crate::model::{
    merge_node, BlueprintModel, BlueprintNode, NodeKind, ProofStatus, StatementStatus, StatusSource,
};

type Result<T> = std::result::Result<T, BlueprintError>;

#[derive(Debug, Deserialize)]
struct Manifest {
    #[serde(default)]
    graphs: Vec<Graph>,
    #[serde(default)]
    previews: Vec<Preview>,
}

#[derive(Debug, Deserialize)]
struct Graph {
    #[serde(default)]
    nodes: Vec<Node>,
}

#[derive(Debug, Deserialize)]
struct Node {
    label: String,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    parent: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(rename = "previewKey", default)]
    preview_key: Option<String>,
    #[serde(rename = "statementStatus", default)]
    statement_status: Option<String>,
    #[serde(rename = "proofStatus", default)]
    proof_status: Option<String>,
    #[serde(rename = "statementUses", default)]
    statement_uses: Vec<UseEdge>,
    #[serde(rename = "proofUses", default)]
    proof_uses: Vec<UseEdge>,
}

#[derive(Debug, Deserialize)]
struct UseEdge {
    label: String,
}

#[derive(Debug, Deserialize)]
struct Preview {
    key: String,
    #[serde(rename = "codeData", default)]
    code_data: Option<CodeData>,
}

#[derive(Debug, Deserialize)]
struct CodeData {
    #[serde(default)]
    external: Option<External>,
}

#[derive(Debug, Deserialize)]
struct External {
    #[serde(default)]
    decls: Vec<Decl>,
}

#[derive(Debug, Deserialize)]
struct Decl {
    canonical: String,
}

/// Known Verso statement-status vocabulary (for error messages).
const STATEMENT_STATUSES: &str = "none, blocked, ready, formalized, mathlib";
/// Known Verso proof-status vocabulary (for error messages).
const PROOF_STATUSES: &str = "none, ready, incomplete, formalized, formalizedWithAncestors";

fn map_statement(s: Option<&str>) -> Result<StatementStatus> {
    Ok(match s {
        None | Some("none") => StatementStatus::NonePlanned,
        Some("formalized") => StatementStatus::Formalized,
        // `mathlib` (Verso ≥ v4.31) marks a statement already available upstream
        // in Mathlib: it is formalized, only not in this project's own sources.
        // Treat it as `formalized` on the statement axis.
        Some("mathlib") => StatementStatus::Formalized,
        Some("ready") => StatementStatus::Ready,
        Some("blocked") => StatementStatus::Blocked,
        // Unknown status: fail loudly rather than silently bucketing to the
        // worst state, which would under-count progress on schema drift.
        Some(other) => {
            return Err(BlueprintError::UnknownStatus {
                axis: "statement",
                value: other.to_string(),
                expected: STATEMENT_STATUSES,
            })
        }
    })
}

fn map_proof(s: Option<&str>) -> Result<ProofStatus> {
    Ok(match s {
        None | Some("none") => ProofStatus::None,
        Some("formalizedWithAncestors") => ProofStatus::FullyProved,
        Some("formalized") => ProofStatus::Proved,
        Some("ready") => ProofStatus::Ready,
        // `incomplete` (Verso ≥ v4.31) marks a proof whose Lean code exists but
        // is not complete (contains `sorry`/gaps). It is explicitly *not* a
        // finished proof, so it must not count as proved; bucket it as `none`
        // (not-proved) so the proof axis and the `blueprint-status-mismatch`
        // check (P26) stay honest against probe-lean's machine status.
        Some("incomplete") => ProofStatus::None,
        Some(other) => {
            return Err(BlueprintError::UnknownStatus {
                axis: "proof",
                value: other.to_string(),
                expected: PROOF_STATUSES,
            })
        }
    })
}

fn parse_manifest(text: &str, chapter: Option<&str>) -> Result<BlueprintModel> {
    let manifest: Manifest = serde_json::from_str(text).map_err(BlueprintError::ManifestParse)?;

    // Index preview key -> Lean decl canonical names.
    let mut decls_by_preview: HashMap<String, Vec<String>> = HashMap::new();
    for preview in &manifest.previews {
        if let Some(cd) = &preview.code_data {
            if let Some(ext) = &cd.external {
                let names: Vec<String> = ext.decls.iter().map(|d| d.canonical.clone()).collect();
                if !names.is_empty() {
                    decls_by_preview.insert(preview.key.clone(), names);
                }
            }
        }
    }

    let mut model = BlueprintModel::default();
    // A label can appear in multiple graphs within one manifest (e.g. statement
    // and proof sub-graphs). De-duplicate within the manifest so a node is not
    // counted twice; recurring labels are merged with the model's merge policy.
    let mut index_by_label: HashMap<String, usize> = HashMap::new();
    for graph in &manifest.graphs {
        for node in &graph.nodes {
            let lean_decls = node
                .preview_key
                .as_ref()
                .and_then(|k| decls_by_preview.get(k))
                .cloned()
                .unwrap_or_default();

            let built = BlueprintNode {
                label: node.label.clone(),
                kind: NodeKind::from_source(node.kind.as_deref().unwrap_or("theorem")),
                lean_decls,
                statement_status: map_statement(node.statement_status.as_deref())?,
                proof_status: map_proof(node.proof_status.as_deref())?,
                statement_uses: node
                    .statement_uses
                    .iter()
                    .map(|u| u.label.clone())
                    .collect(),
                proof_uses: node.proof_uses.iter().map(|u| u.label.clone()).collect(),
                group: node.parent.clone(),
                chapter: chapter.map(str::to_string),
                title: node.title.clone(),
                discussion: None,
                status_source: StatusSource::CodeDerived,
            };

            if let Some(&idx) = index_by_label.get(&built.label) {
                merge_node(&mut model.nodes[idx], built);
            } else {
                index_by_label.insert(built.label.clone(), model.nodes.len());
                model.nodes.push(built);
            }
        }
    }
    Ok(model)
}

/// Load a single Verso blueprint manifest file into a [`BlueprintModel`].
pub fn load_manifest(path: &Path) -> Result<BlueprintModel> {
    let text = std::fs::read_to_string(path).map_err(|source| BlueprintError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    parse_manifest(&text, chapter_from_path(path).as_deref())
}

/// Infrastructure directory names in a Verso output tree that are not the
/// chapter. The chapter is the nearest ancestor of the manifest whose name is
/// not one of these — robust across the `chapter-renders/<Chapter>/html-multi`
/// and `html-multi/<Chapter>` layouts.
const INFRA_DIRS: &[&str] = &[
    "-verso-data",
    "html-multi",
    "html-single",
    "chapter-renders",
    "preview",
    "_out",
    "site",
];

fn chapter_from_path(path: &Path) -> Option<String> {
    for ancestor in path.ancestors().skip(1) {
        if let Some(name) = ancestor.file_name().and_then(|n| n.to_str()) {
            if !INFRA_DIRS.contains(&name) {
                return Some(name.to_string());
            }
        }
    }
    None
}

/// Discover and merge every `blueprint-manifest.json` under `root` (recursively),
/// de-duplicating nodes by label. Useful for Verso projects that render one
/// manifest per chapter.
pub fn load_from_dir(root: &Path) -> Result<BlueprintModel> {
    let mut found = Vec::new();
    collect_manifests(root, &mut found)?;
    if found.is_empty() {
        return Err(BlueprintError::NoManifest(root.to_path_buf()));
    }
    found.sort();
    let mut model = BlueprintModel::default();
    for path in found {
        let part = load_manifest(&path)?;
        model.merge_from(part);
    }
    Ok(model)
}

fn collect_manifests(dir: &Path, out: &mut Vec<std::path::PathBuf>) -> Result<()> {
    if !dir.is_dir() {
        return Ok(());
    }
    let read = std::fs::read_dir(dir).map_err(|source| BlueprintError::Io {
        path: dir.to_path_buf(),
        source,
    })?;
    for entry in read {
        let entry = entry.map_err(|source| BlueprintError::Io {
            path: dir.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        if path.is_dir() {
            collect_manifests(&path, out)?;
        } else if path.file_name().and_then(|n| n.to_str()) == Some("blueprint-manifest.json") {
            out.push(path);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_nodes_and_bindings() {
        let text = r#"{
          "graphs": [{
            "nodes": [
              {"label":"a","kind":"definition","parent":"chap","title":"Definition 1.1",
               "previewKey":"a--statement","statementStatus":"formalized",
               "proofStatus":"formalizedWithAncestors","statementUses":[],"proofUses":[]},
              {"label":"b","kind":"theorem","parent":"chap","title":"Theorem 1.2",
               "previewKey":"b--statement","statementStatus":"ready","proofStatus":"none",
               "statementUses":[{"label":"a"}],"proofUses":[]}
            ]
          }],
          "previews": [
            {"key":"a--statement","codeData":{"external":{"decls":[{"canonical":"Foo.a"}]}}},
            {"key":"b--statement","codeData":{"external":{"decls":[{"canonical":"Foo.b"}]}}}
          ]
        }"#;
        let model = parse_manifest(text, Some("Chap-One")).unwrap();
        assert_eq!(model.nodes.len(), 2);
        assert_eq!(model.nodes[0].chapter.as_deref(), Some("Chap-One"));
        let a = model.nodes.iter().find(|n| n.label == "a").unwrap();
        assert_eq!(a.lean_decls, vec!["Foo.a"]);
        assert_eq!(a.statement_status, StatementStatus::Formalized);
        assert_eq!(a.proof_status, ProofStatus::FullyProved);
        let b = model.nodes.iter().find(|n| n.label == "b").unwrap();
        assert_eq!(b.statement_uses, vec!["a"]);
        assert_eq!(b.statement_status, StatementStatus::Ready);
    }

    #[test]
    fn unknown_statement_status_is_an_error() {
        let text = r#"{
          "graphs": [{"nodes": [
            {"label":"a","statementStatus":"bogus","proofStatus":"none"}
          ]}],
          "previews": []
        }"#;
        let err = parse_manifest(text, None).unwrap_err();
        assert!(matches!(
            err,
            BlueprintError::UnknownStatus {
                axis: "statement",
                ..
            }
        ));
    }

    #[test]
    fn unknown_proof_status_is_an_error() {
        let text = r#"{
          "graphs": [{"nodes": [
            {"label":"a","statementStatus":"ready","proofStatus":"bogus"}
          ]}],
          "previews": []
        }"#;
        let err = parse_manifest(text, None).unwrap_err();
        assert!(matches!(
            err,
            BlueprintError::UnknownStatus { axis: "proof", .. }
        ));
    }

    #[test]
    fn verso_v431_statuses_map_to_canonical() {
        // Verso >= v4.31 introduces `mathlib` (statement axis) and `incomplete`
        // (proof axis). They must map into the canonical vocabulary rather than
        // erroring as schema drift.
        let text = r#"{
          "graphs": [{"nodes": [
            {"label":"m","statementStatus":"mathlib","proofStatus":"formalized"},
            {"label":"i","statementStatus":"formalized","proofStatus":"incomplete"}
          ]}],
          "previews": []
        }"#;
        let model = parse_manifest(text, None).unwrap();
        let m = model.nodes.iter().find(|n| n.label == "m").unwrap();
        assert_eq!(m.statement_status, StatementStatus::Formalized);
        let i = model.nodes.iter().find(|n| n.label == "i").unwrap();
        // An incomplete (sorried) proof is not a complete proof.
        assert_eq!(i.proof_status, ProofStatus::None);
        assert!(!i.proof_status.claims_proved());
    }

    #[test]
    fn within_manifest_duplicate_labels_are_deduped() {
        // The same label appears in two graphs; it must collapse to one node
        // with unioned statuses (max).
        let text = r#"{
          "graphs": [
            {"nodes": [
              {"label":"a","statementStatus":"blocked","proofStatus":"none"}
            ]},
            {"nodes": [
              {"label":"a","statementStatus":"formalized","proofStatus":"ready"}
            ]}
          ],
          "previews": []
        }"#;
        let model = parse_manifest(text, None).unwrap();
        assert_eq!(
            model.nodes.len(),
            1,
            "duplicate label collapses to one node"
        );
        assert_eq!(model.nodes[0].statement_status, StatementStatus::Formalized);
        assert_eq!(model.nodes[0].proof_status, ProofStatus::Ready);
    }
}
