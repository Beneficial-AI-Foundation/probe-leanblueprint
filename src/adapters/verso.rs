//! Verso Blueprint adapter.
//!
//! Reads a `blueprint-manifest.json` produced by the `versoBlueprint` renderer
//! and maps its graph nodes + Lean-decl bindings into a [`BlueprintModel`].
//! No parsing of our own — the manifest is already machine-readable.

use std::collections::{HashMap, HashSet};
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
    /// Verso's internal manifest generation: 2 on v4.30, 3 on v4.31. Absent or
    /// unrecognized values suggest a drifted or pre-graph (v4.28) file.
    #[serde(rename = "vbpInternalSchemaVersion", default)]
    vbp_internal_schema_version: Option<u64>,
}

/// Manifest generations whose `graphs[].nodes[]` schema this adapter understands.
const KNOWN_SCHEMA_VERSIONS: &[u64] = &[2, 3];

/// Human-readable warnings about a parsed manifest — an unknown schema
/// generation, or an empty graph distinguished as previews-only vs wrong/drifted.
/// Empty when the manifest looks healthy.
fn manifest_warnings(
    schema_version: Option<u64>,
    graph_nodes: usize,
    previews: usize,
) -> Vec<String> {
    let mut out = Vec::new();
    match schema_version {
        Some(v) if KNOWN_SCHEMA_VERSIONS.contains(&v) => {}
        Some(v) => out.push(format!(
            "unrecognized vbpInternalSchemaVersion {v} (known: {KNOWN_SCHEMA_VERSIONS:?}); \
             the blueprint graph schema may have drifted"
        )),
        None => out.push(
            "manifest has no vbpInternalSchemaVersion; it may be a pre-graph (v4.28) \
             preview manifest or a non-Verso file"
                .to_string(),
        ),
    }
    if graph_nodes == 0 {
        if previews == 0 {
            out.push(
                "0 graph nodes and 0 previews — this looks like a wrong or drifted file, \
                 not a blueprint manifest"
                    .to_string(),
            );
        } else {
            out.push(format!(
                "0 graph nodes but {previews} previews — previews-only blueprint (no formal \
                 graph); nothing to score"
            ));
        }
    }
    out
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
    /// In-site link, e.g. `Multiplication/#--…`. Its first path segment is the
    /// chapter, authoritative over the manifest's directory (see
    /// [`chapter_from_href`]).
    #[serde(default)]
    href: Option<String>,
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
    /// Declarations defined *inline* in the blueprint text (a `lean` code block,
    /// as opposed to `external`, which references an existing decl).
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
    /// These three are decl *metadata* used only to detect upstream-proved decls.
    /// They are deliberately typed as free `Value` (not `bool`/`String`): the
    /// renderer emits them polymorphically (e.g. `provedStatus` is the string
    /// `"proved"` when sorry-free but an object like `{"containsSorry": …}`
    /// otherwise). A stricter type would abort the *whole* manifest parse on one
    /// variant decl; here an unexpected shape just fails the `is_upstream_proved`
    /// predicate (degrades to "not upstream-proved"), never crashes.
    #[serde(default)]
    present: Option<serde_json::Value>,
    #[serde(rename = "provedStatus", default)]
    proved_status: Option<serde_json::Value>,
    #[serde(default)]
    provenance: Option<serde_json::Value>,
}

impl Decl {
    /// Out-of-workspace (`provenance.outWorkspace`), present, and proved per the
    /// renderer — the upstream-proved criterion. "Out-of-workspace" is all this
    /// checks (a dependency, commonly but not necessarily Mathlib/stdlib); see
    /// `docs/SCHEMA.md` §Node classification.
    fn is_upstream_proved(&self) -> bool {
        let out_of_workspace = self
            .provenance
            .as_ref()
            .and_then(|p| p.get("outWorkspace"))
            .is_some();
        let present = self.present.as_ref().and_then(|v| v.as_bool()) == Some(true);
        let proved = self.proved_status.as_ref().and_then(|v| v.as_str()) == Some("proved");
        out_of_workspace && present && proved
    }
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

/// Raw source statuses whose canonical mapping loses information, so they are
/// preserved verbatim in `blueprint-source-*-status`: `mathlib` (statement,
/// formalized upstream in Mathlib) and `incomplete` (proof, sorried/in-progress).
const LOSSY_SOURCE_STATUSES: &[&str] = &["mathlib", "incomplete"];

fn lossy_source_status(raw: Option<&str>) -> Option<String> {
    raw.filter(|s| LOSSY_SOURCE_STATUSES.contains(s))
        .map(str::to_string)
}

fn map_statement(s: Option<&str>) -> Result<StatementStatus> {
    Ok(match s {
        None | Some("none") => StatementStatus::NonePlanned,
        Some("formalized") => StatementStatus::Formalized,
        // `mathlib` (Verso ≥ v4.31): formalized upstream in Mathlib, not in this
        // project's own sources; formalized on the statement axis.
        Some("mathlib") => StatementStatus::Formalized,
        Some("ready") => StatementStatus::Ready,
        Some("blocked") => StatementStatus::Blocked,
        // Unknown status: error on schema drift rather than bucketing it.
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
        // `incomplete` (Verso ≥ v4.31): Lean code exists but is not a finished
        // proof (contains `sorry`/gaps), so it maps to `none`, not proved.
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
    // Canonical names of external decls the renderer proved upstream (out of this
    // workspace). A node bound only to these is decl-missing here yet proved.
    let mut upstream_proved: HashSet<String> = HashSet::new();
    for preview in &manifest.previews {
        let Some(cd) = &preview.code_data else {
            continue;
        };
        let mut names: Vec<String> = Vec::new();
        if let Some(ext) = &cd.external {
            for d in &ext.decls {
                names.push(d.canonical.clone());
                if d.is_upstream_proved() {
                    upstream_proved.insert(d.canonical.clone());
                }
            }
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
                // A `null` kind (a mention of a node defined in another chapter)
                // stays unknown so the defining copy's kind wins during merge.
                kind: node.kind.as_deref().map(NodeKind::from_source),
                external_upstream_proved: lean_decls
                    .iter()
                    .filter(|d| upstream_proved.contains(*d))
                    .cloned()
                    .collect(),
                lean_decls,
                statement_status: map_statement(node.statement_status.as_deref())?,
                proof_status: map_proof(node.proof_status.as_deref())?,
                source_statement_status: lossy_source_status(node.statement_status.as_deref()),
                source_proof_status: lossy_source_status(node.proof_status.as_deref()),
                statement_uses: node
                    .statement_uses
                    .iter()
                    .map(|u| u.label.clone())
                    .collect(),
                proof_uses: node.proof_uses.iter().map(|u| u.label.clone()).collect(),
                group: node.parent.clone(),
                // Prefer the chapter encoded in the node's own href (correct even
                // for a single shared manifest); fall back to the manifest's
                // directory only when the node carries no href (older schemas).
                chapter: node
                    .href
                    .as_deref()
                    .and_then(chapter_from_href)
                    .or_else(|| chapter.map(str::to_string)),
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
    for w in manifest_warnings(
        manifest.vbp_internal_schema_version,
        model.nodes.len(),
        manifest.previews.len(),
    ) {
        eprintln!("warning: {w}");
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

/// The chapter a node belongs to, taken from the first path segment of its
/// in-site `href` (e.g. `Multiplication/#--…` -> `Multiplication`). This is
/// authoritative over the manifest's directory: a single shared manifest renders
/// every chapter, so the directory name would collapse them into one bucket.
fn chapter_from_href(href: &str) -> Option<String> {
    // The chapter is the first real path segment. Skip empty segments and any
    // anchor-only segment (`#…`): an href like `#--x--statement` carries no
    // chapter, so it must fall through to the manifest-directory fallback rather
    // than become a junk `#…` chapter bucket.
    let seg = href
        .split('/')
        .find(|s| !s.is_empty() && !s.starts_with('#'))?
        .trim();
    if seg.is_empty() {
        None
    } else {
        Some(seg.to_string())
    }
}

fn chapter_from_path(path: &Path) -> Option<String> {
    // Only a canonically-named manifest sits in a Verso output tree whose path
    // encodes the chapter; for any other file the parent directory is not one.
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

/// Discover every `blueprint-manifest.json` under `root` (recursively), sorted
/// for deterministic merge order. Errors with `NoManifest` when none is found.
pub fn discover_manifests(root: &Path) -> Result<Vec<std::path::PathBuf>> {
    let mut found = Vec::new();
    collect_manifests(root, &mut found)?;
    if found.is_empty() {
        return Err(BlueprintError::NoManifest(root.to_path_buf()));
    }
    found.sort();
    Ok(found)
}

/// Discover and merge every `blueprint-manifest.json` under `root` (recursively),
/// de-duplicating nodes by label. Useful for Verso projects that render one
/// manifest per chapter.
pub fn load_from_dir(root: &Path) -> Result<BlueprintModel> {
    let mut model = BlueprintModel::default();
    for path in discover_manifests(root)? {
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
        // `file_type()` does not follow symlinks, so symlinked directories are
        // not descended into (no cycles).
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
    fn external_upstream_proved_read_from_provenance() {
        // `up` binds an out-of-workspace, present, proved decl (upstream); `loc`
        // binds an in-workspace one. Only `up` is recorded as upstream-proved.
        let text = r#"{
          "graphs": [{"nodes": [
            {"label":"up","kind":"theorem","previewKey":"up--statement",
             "statementStatus":"formalized","proofStatus":"formalizedWithAncestors"},
            {"label":"loc","kind":"theorem","previewKey":"loc--statement",
             "statementStatus":"formalized","proofStatus":"formalizedWithAncestors"}
          ]}],
          "previews": [
            {"key":"up--statement","codeData":{"external":{"decls":[
              {"canonical":"Nat.mul_assoc","present":true,"provedStatus":"proved",
               "provenance":{"outWorkspace":{"moduleName":"Init.Data.Nat.Basic"}}}
            ]}}},
            {"key":"loc--statement","codeData":{"external":{"decls":[
              {"canonical":"MyProj.thm","present":true,"provedStatus":"proved",
               "provenance":{"inWorkspace":{"moduleName":"MyProj"}}}
            ]}}}
          ]
        }"#;
        let model = parse_manifest(text, None).unwrap();
        let up = model.nodes.iter().find(|n| n.label == "up").unwrap();
        assert_eq!(up.external_upstream_proved, vec!["Nat.mul_assoc"]);
        let loc = model.nodes.iter().find(|n| n.label == "loc").unwrap();
        assert!(
            loc.external_upstream_proved.is_empty(),
            "in-workspace decls are not upstream-proved"
        );
    }

    /// An upstream decl that is present but *not* proved (e.g. sorried upstream)
    /// is not upstream-proved — it stays a genuine gap.
    #[test]
    fn external_upstream_not_proved_is_not_credited() {
        let text = r#"{
          "graphs": [{"nodes": [
            {"label":"up","kind":"theorem","previewKey":"up--statement",
             "statementStatus":"formalized","proofStatus":"formalized"}
          ]}],
          "previews": [
            {"key":"up--statement","codeData":{"external":{"decls":[
              {"canonical":"Up.sorried","present":true,"provedStatus":"unverified",
               "provenance":{"outWorkspace":{"moduleName":"Up"}}}
            ]}}}
          ]
        }"#;
        let model = parse_manifest(text, None).unwrap();
        assert!(model.nodes[0].external_upstream_proved.is_empty());
    }

    /// The renderer emits `provedStatus` polymorphically: the string `"proved"`
    /// when sorry-free, but an OBJECT (`{"containsSorry": …}`) otherwise (real in
    /// the flt / sphere-packing manifests). A strict `String` type here aborts the
    /// whole parse; the decl-metadata fields must tolerate either shape. The
    /// object form must parse *and* not be credited as proved.
    #[test]
    fn object_valued_proved_status_parses_and_is_not_credited() {
        let text = r#"{
          "graphs": [{"nodes": [
            {"label":"up","kind":"theorem","previewKey":"up--statement",
             "statementStatus":"formalized","proofStatus":"formalized"},
            {"label":"ok","kind":"theorem","previewKey":"ok--statement",
             "statementStatus":"formalized","proofStatus":"formalizedWithAncestors"}
          ]}],
          "previews": [
            {"key":"up--statement","codeData":{"external":{"decls":[
              {"canonical":"Up.sorried","present":true,
               "provedStatus":{"containsSorry":{"info":[{"location":"proof"}]}},
               "provenance":{"outWorkspace":{"moduleName":"Up"}}}
            ]}}},
            {"key":"ok--statement","codeData":{"external":{"decls":[
              {"canonical":"Up.done","present":true,"provedStatus":"proved",
               "provenance":{"outWorkspace":{"moduleName":"Up"}}}
            ]}}}
          ]
        }"#;
        // Must not fail to parse despite the object-valued provedStatus.
        let model = parse_manifest(text, None).expect("object-valued provedStatus must parse");
        let up = model.nodes.iter().find(|n| n.label == "up").unwrap();
        assert!(
            up.external_upstream_proved.is_empty(),
            "containsSorry is not proved, so not upstream-proved"
        );
        // A sibling with string `"proved"` in the same manifest is still credited.
        let ok = model.nodes.iter().find(|n| n.label == "ok").unwrap();
        assert_eq!(ok.external_upstream_proved, vec!["Up.done"]);
    }

    #[test]
    fn chapter_comes_from_node_href_over_manifest_dir() {
        // A single shared manifest: nodes carry their real chapter in `href`,
        // which must win over the directory name passed for the whole manifest.
        assert_eq!(
            chapter_from_href("Multiplication/#--x--statement").as_deref(),
            Some("Multiplication")
        );
        assert_eq!(
            chapter_from_href("Collatz/Source-Entries/#--y").as_deref(),
            Some("Collatz")
        );
        assert_eq!(chapter_from_href("").as_deref(), None);
        // Anchor-only href carries no chapter path segment: must NOT become a
        // `#…` chapter (it should fall through to the directory fallback).
        assert_eq!(chapter_from_href("#--x--statement").as_deref(), None);

        let text = r#"{
          "graphs": [{"nodes": [
            {"label":"a","kind":"theorem","href":"Real-Chapter/#--a",
             "statementStatus":"formalized","proofStatus":"none"}
          ]}],
          "previews": []
        }"#;
        // Manifest-level chapter is "Wrong-Dir"; the node's href wins.
        let model = parse_manifest(text, Some("Wrong-Dir")).unwrap();
        assert_eq!(model.nodes[0].chapter.as_deref(), Some("Real-Chapter"));

        // A node whose only href is an anchor falls back to the manifest dir,
        // not a "#…" chapter bucket.
        let anchor = r##"{
          "graphs": [{"nodes": [
            {"label":"b","kind":"theorem","href":"#--b--statement",
             "statementStatus":"formalized","proofStatus":"none"}
          ]}],
          "previews": []
        }"##;
        let model = parse_manifest(anchor, Some("Fallback-Dir")).unwrap();
        assert_eq!(model.nodes[0].chapter.as_deref(), Some("Fallback-Dir"));
    }

    #[test]
    fn manifest_warnings_distinguish_empty_from_drifted() {
        // Healthy manifest (known generation, has nodes): no warnings.
        assert!(manifest_warnings(Some(3), 5, 10).is_empty());
        // Previews-only: a legitimate blueprint with no formal graph.
        let previews_only = manifest_warnings(Some(2), 0, 12);
        assert!(previews_only.iter().any(|w| w.contains("previews-only")));
        // Nothing at all: wrong/drifted file.
        let drifted = manifest_warnings(Some(3), 0, 0);
        assert!(drifted.iter().any(|w| w.contains("wrong or drifted")));
        // Unknown / missing generation is flagged.
        assert!(manifest_warnings(Some(99), 5, 5)
            .iter()
            .any(|w| w.contains("unrecognized vbpInternalSchemaVersion")));
        assert!(manifest_warnings(None, 5, 5)
            .iter()
            .any(|w| w.contains("no vbpInternalSchemaVersion")));
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
        // The lossy raw status is preserved so "lives upstream in Mathlib" is
        // not indistinguishable from a locally-formalized statement.
        assert_eq!(m.source_statement_status.as_deref(), Some("mathlib"));
        let i = model.nodes.iter().find(|n| n.label == "i").unwrap();
        // An incomplete (sorried) proof is not a complete proof.
        assert_eq!(i.proof_status, ProofStatus::None);
        assert!(!i.proof_status.claims_proved());
        // ...but "sorried, in progress" is preserved distinct from "not started".
        assert_eq!(i.source_proof_status.as_deref(), Some("incomplete"));
        // A lossless status carries no raw source status.
        assert_eq!(m.source_proof_status, None);
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
