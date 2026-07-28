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
    /// Declarations bound *inline* in the blueprint text (a `lean` code block
    /// rather than a reference to an existing decl). Modern manifests carry the
    /// bound decl names here, so a node authored this way must be joined too.
    #[serde(default)]
    inline: Option<Inline>,
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

#[derive(Debug, Deserialize)]
struct Inline {
    #[serde(default)]
    code: Option<InlineCode>,
}

#[derive(Debug, Deserialize)]
struct InlineCode {
    #[serde(rename = "definedDefs", default)]
    defined_defs: Vec<DefinedDecl>,
    #[serde(rename = "definedTheorems", default)]
    defined_theorems: Vec<DefinedDecl>,
}

#[derive(Debug, Deserialize)]
struct DefinedDecl {
    name: String,
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

    // Index preview key -> Lean decl names. A preview binds decls either by
    // reference to an existing decl (`external.decls[].canonical`) or inline in
    // the blueprint text (`inline.code.definedDefs/definedTheorems[].name`);
    // collect both so inline-authored nodes join too.
    let mut decls_by_preview: HashMap<String, Vec<String>> = HashMap::new();
    for preview in &manifest.previews {
        let Some(cd) = &preview.code_data else {
            continue;
        };
        let mut names: Vec<String> = Vec::new();
        if let Some(ext) = &cd.external {
            names.extend(ext.decls.iter().map(|d| d.canonical.clone()));
        }
        if let Some(code) = cd.inline.as_ref().and_then(|i| i.code.as_ref()) {
            names.extend(code.defined_defs.iter().map(|d| d.name.clone()));
            names.extend(code.defined_theorems.iter().map(|d| d.name.clone()));
        }
        // De-duplicate while preserving first-seen order (external before inline).
        let mut seen = std::collections::HashSet::new();
        names.retain(|n| seen.insert(n.clone()));
        if !names.is_empty() {
            decls_by_preview.insert(preview.key.clone(), names);
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
                // A `null` kind means this is a mention of a node defined in
                // another chapter; leave it unknown so the defining copy wins
                // the kind during merge rather than freezing it as a theorem.
                kind: node.kind.as_deref().map(NodeKind::from_source),
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
    // Only a real Verso output artifact carries chapter structure in its path.
    // For any other file (e.g. a manifest passed via `--verso-manifest` under an
    // arbitrary name), the parent directory is just wherever the file happens to
    // live, not a chapter — deriving one from it would be misleading.
    if path.file_name().and_then(|n| n.to_str()) != Some("blueprint-manifest.json") {
        return None;
    }
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

/// Directories skipped when walking a Lean project for manifests. Build,
/// dependency, and VCS trees can be huge and never contain a Verso render (which
/// lands under `_out/site/`), so descending into them only wastes time.
const SKIP_DIRS: &[&str] = &[".lake", ".git", "target", "node_modules", "lake-packages"];

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
        // `file_type()` reports the entry itself without following symlinks, so
        // symlinked directories are treated as non-directories and skipped —
        // avoiding surprising recursion (and cycles) through linked trees.
        let file_type = entry.file_type().map_err(|source| BlueprintError::Io {
            path: entry.path(),
            source,
        })?;
        let path = entry.path();
        if file_type.is_dir() {
            let name = path.file_name().and_then(|n| n.to_str());
            if name.is_some_and(|n| SKIP_DIRS.contains(&n)) {
                continue;
            }
            collect_manifests(&path, out)?;
        } else if file_type.is_file()
            && path.file_name().and_then(|n| n.to_str()) == Some("blueprint-manifest.json")
        {
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
    fn binds_inline_decls_not_just_external() {
        // `a` binds an existing decl by reference (external); `b` binds a decl
        // defined inline in the blueprint text. Both must join.
        let text = r#"{
          "graphs": [{"nodes": [
            {"label":"a","kind":"theorem","previewKey":"a--statement",
             "statementStatus":"formalized","proofStatus":"formalized"},
            {"label":"b","kind":"theorem","previewKey":"b--statement",
             "statementStatus":"formalized","proofStatus":"formalized"}
          ]}],
          "previews": [
            {"key":"a--statement","codeData":{"external":{"decls":[{"canonical":"Foo.a"}]}}},
            {"key":"b--statement","codeData":{"inline":{"code":{
               "definedDefs":[{"name":"b_def"}],
               "definedTheorems":[{"name":"b_thm"}]}}}}
          ]
        }"#;
        let model = parse_manifest(text, None).unwrap();
        let a = model.nodes.iter().find(|n| n.label == "a").unwrap();
        assert_eq!(a.lean_decls, vec!["Foo.a"], "external decl still bound");
        let b = model.nodes.iter().find(|n| n.label == "b").unwrap();
        assert_eq!(
            b.lean_decls,
            vec!["b_def", "b_thm"],
            "inline defs and theorems both bound"
        );
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
    fn chapter_only_derived_for_canonical_manifest_name() {
        // A real Verso artifact carries chapter structure in its path.
        let real =
            Path::new("_out/site/html-multi/Erasure-Codes/-verso-data/blueprint-manifest.json");
        assert_eq!(chapter_from_path(real).as_deref(), Some("Erasure-Codes"));
        // An arbitrarily-named manifest (e.g. via `--verso-manifest`) has no
        // chapter to infer; the parent dir is not part of a Verso output tree.
        let arbitrary = Path::new("tests/fixtures/verso/erasure-codes-manifest.json");
        assert_eq!(chapter_from_path(arbitrary), None);
    }

    #[test]
    fn collect_manifests_skips_build_and_vcs_dirs() {
        let root = tempfile::tempdir().unwrap();
        let base = root.path();
        // A manifest inside a real render tree must be found.
        let good = base.join("_out/site/html-multi/Chap/-verso-data");
        std::fs::create_dir_all(&good).unwrap();
        std::fs::write(good.join("blueprint-manifest.json"), "{}").unwrap();
        // Manifests buried in build/VCS trees must be ignored.
        for skipped in SKIP_DIRS {
            let d = base.join(skipped).join("nested");
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join("blueprint-manifest.json"), "{}").unwrap();
        }
        let mut found = Vec::new();
        collect_manifests(base, &mut found).unwrap();
        assert_eq!(found.len(), 1, "only the _out manifest should be collected");
        assert!(found[0].starts_with(base.join("_out")));
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
