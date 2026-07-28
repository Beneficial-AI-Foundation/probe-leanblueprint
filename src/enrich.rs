//! Enrichment core: join a [`BlueprintModel`] onto probe-lean atoms.
//!
//! - Blueprint nodes are matched to atoms by `probe:` + Lean declaration name.
//! - Matched atoms gain `blueprint-*` extension fields (statement/proof status,
//!   uses, group, title, discussion), keeping probe-lean's machine
//!   `verification-status` authoritative on the proof axis.
//! - Nodes with no Lean binding become synthetic "planned" atoms so the
//!   statement axis (roadmap) is represented.
//! - A `blueprint-status-mismatch` flag is set when the blueprint claims a proof
//!   is done but probe-lean found it unverified/failed.

use std::collections::{BTreeMap, HashMap};

use probe::types::{Atom, CodeText};
use serde_json::Value;

use crate::model::{
    BlueprintExtensions, BlueprintModel, BlueprintNode, NodeKind, ProofStatus, StatementStatus,
};

/// Marker code-path for synthetic planned atoms. Non-empty so P3 stub detection
/// (`code-path == "" && lines 0,0`) never misclassifies a planned node as a stub.
// @kb: kb/engineering/properties.md#p3-stub-detection-is-structural
const BLUEPRINT_CODE_PATH: &str = "blueprint";

fn code_name_for_decl(decl: &str) -> String {
    format!("{}{decl}", crate::PROBE_PREFIX)
}

fn synthetic_key(label: &str) -> String {
    format!("{}blueprint:{label}", crate::PROBE_PREFIX)
}

/// Summary of what enrichment did, used to build the summary sidecar and logs.
#[derive(Debug, Default)]
pub struct EnrichReport {
    pub nodes_total: usize,
    pub nodes_with_decl: usize,
    pub planned_only: usize,
    pub decl_missing: usize,
    /// De-duplicated by node label (one entry per node that claims a proof the
    /// machine status contradicts), matching `blueprint_stats.py`'s per-label
    /// counting.
    pub mismatches: Vec<String>,
    pub synthesized: usize,
    /// Bound nodes for which some (but not all) `lean_decls` were absent from
    /// the atom base. Recorded per-node; the absent names go into the present
    /// atom(s)' `blueprint-missing-decls` field.
    pub partial_missing: usize,
    /// Count of present atoms bound by more than one blueprint node (later node
    /// wins on the real atom; the earlier node is preserved as a shadow).
    pub collisions: usize,
    /// Count of nodes that bind a present decl already claimed by a later node
    /// and were therefore emitted as a `blueprint-shadow` synthetic atom to keep
    /// the extract node-complete. Such a node is still counted in
    /// `nodes_with_decl` (it is genuinely bound).
    pub collision_shadowed: usize,
    /// Count of synthetic keys produced more than once in a single run (later
    /// wins). Indicates duplicate labels leaking past adapter de-duplication.
    pub duplicate_synthetic: usize,
}

fn machine_status(atom: &Atom) -> Option<String> {
    atom.extensions
        .get("verification-status")
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

/// Compute the mismatch marker: the blueprint claims the proof is done but
/// probe-lean's machine status contradicts it. Returns `None` when consistent.
// @kb: kb/engineering/properties.md#p26-blueprint-status-is-additive-machine-verification-status-stays-authoritative
fn mismatch_marker(proof: ProofStatus, machine: Option<&str>) -> Option<String> {
    if proof.claims_proved() {
        match machine {
            Some(s @ ("unverified" | "failed")) => Some(format!("claims-proved-but-{s}")),
            _ => None,
        }
    } else {
        None
    }
}

fn make_extensions(
    node: &BlueprintNode,
    uses_index: &HashMap<String, String>,
    mismatch: Option<String>,
    decl_missing: bool,
    missing_decls: Vec<String>,
    shadow: bool,
) -> BlueprintExtensions {
    let resolve = |labels: &[String]| -> Vec<String> {
        labels
            .iter()
            .map(|l| {
                uses_index
                    .get(l)
                    .cloned()
                    .unwrap_or_else(|| synthetic_key(l))
            })
            .collect()
    };
    BlueprintExtensions {
        label: node.label.clone(),
        kind: node.display_kind().as_str().to_string(),
        statement_status: node.statement_status.as_str().to_string(),
        proof_status: node.proof_status.as_str().to_string(),
        status_source: node.status_source.as_str().to_string(),
        group: node.group.clone(),
        chapter: node.chapter.clone(),
        title: node.title.clone(),
        discussion: node.discussion.clone(),
        statement_uses: resolve(&node.statement_uses),
        proof_uses: resolve(&node.proof_uses),
        status_mismatch: mismatch,
        decl_missing,
        missing_decls,
        shadow,
    }
}

/// Every `blueprint-*` extension key this tool may write. Used to clear stale
/// keys on re-enrichment so a second pass over already-enriched atoms is
/// idempotent (`skip_serializing_if` would otherwise leave a prior run's
/// `blueprint-status-mismatch` / `blueprint-decl-missing` in place).
const BLUEPRINT_KEYS: &[&str] = &[
    "blueprint-label",
    "blueprint-kind",
    "blueprint-statement-status",
    "blueprint-proof-status",
    "blueprint-status-source",
    "blueprint-group",
    "blueprint-chapter",
    "blueprint-title",
    "blueprint-discussion",
    "blueprint-statement-uses",
    "blueprint-proof-uses",
    "blueprint-status-mismatch",
    "blueprint-decl-missing",
    "blueprint-missing-decls",
    "blueprint-shadow",
];

fn insert_extensions(atom: &mut Atom, ext: &BlueprintExtensions) {
    // Clear any keys left by a prior enrichment so omitted (None/false/empty)
    // fields do not leak across runs.
    for key in BLUEPRINT_KEYS {
        atom.extensions.remove(*key);
    }
    // A plain `#[derive(Serialize)]` struct of scalars/strings never fails to
    // serialize; fall back to leaving the atom's blueprint keys cleared rather
    // than panicking (the `Value::Null` arm below is unreachable in practice).
    let value = serde_json::to_value(ext).unwrap_or(Value::Null);
    if let Value::Object(map) = value {
        for (k, v) in map {
            atom.extensions.insert(k, v);
        }
    }
}

/// Remove every trace of a prior enrichment so a re-run over an already-enriched
/// atom base (or a merged spine that carries old blueprint atoms) is idempotent:
/// drop synthetic blueprint atoms and clear all `blueprint-*` keys from the
/// atoms that remain. Without this, deleting/renaming a blueprint node between
/// runs would leak the node's stale synthetic atom and stale labels forever.
fn scrub_prior_enrichment(atoms: &mut BTreeMap<String, Atom>) {
    atoms.retain(|_, atom| atom.language != "blueprint");
    for atom in atoms.values_mut() {
        for key in BLUEPRINT_KEYS {
            atom.extensions.remove(*key);
        }
    }
}

fn synthetic_atom(node: &BlueprintNode, ext: &BlueprintExtensions) -> Atom {
    let display_name = node
        .label
        .rsplit(&[':', '.'][..])
        .next()
        .unwrap_or(&node.label)
        .to_string();
    let mut atom = Atom {
        display_name,
        dependencies: Default::default(),
        code_module: node.group.clone().unwrap_or_default(),
        code_path: BLUEPRINT_CODE_PATH.to_string(),
        code_text: CodeText {
            lines_start: 0,
            lines_end: 0,
        },
        kind: format!("blueprint-{}", node.display_kind().as_str()),
        language: "blueprint".to_string(),
        extensions: BTreeMap::new(),
    };
    insert_extensions(&mut atom, ext);
    atom
}

/// Join the blueprint model onto the atom map in place.
// @kb: kb/tools/probe-leanblueprint.md#the-join
// @kb: kb/engineering/properties.md#p26-blueprint-status-is-additive-machine-verification-status-stays-authoritative
pub fn enrich(atoms: &mut BTreeMap<String, Atom>, model: &BlueprintModel) -> EnrichReport {
    // Idempotency: drop prior synthetic atoms and stale blueprint-* fields so a
    // re-run (including over a merged spine) rebuilds cleanly from this model.
    scrub_prior_enrichment(atoms);

    let mut report = EnrichReport {
        nodes_total: model.nodes.len(),
        ..Default::default()
    };

    // The present-in-the-atom-base code-names each node binds, computed once.
    let present_by_node: Vec<Vec<String>> = model
        .nodes
        .iter()
        .map(|node| {
            node.lean_decls
                .iter()
                .map(|d| code_name_for_decl(d))
                .filter(|cn| atoms.contains_key(cn))
                .collect()
        })
        .collect();

    // Pass A: ownership. `owner[cn]` is the label of the LAST node that binds
    // present atom `cn` (keep-last). Each re-binding of an already-claimed atom
    // by a different node is a same-decl collision.
    let mut owner: HashMap<String, String> = HashMap::new();
    for (node, present) in model.nodes.iter().zip(&present_by_node) {
        for cn in present {
            if let Some(prev) = owner.get(cn) {
                if prev != &node.label {
                    report.collisions += 1;
                    eprintln!(
                        "warning: atom {cn} is bound by multiple blueprint nodes \
                         ({prev}, {}); keeping the last, preserving the earlier as a shadow",
                        node.label
                    );
                }
            }
            owner.insert(cn.clone(), node.label.clone());
        }
    }
    let owns = |label: &str, cn: &String| owner.get(cn).map(|l| l == label).unwrap_or(false);

    // Pass B: resolve each label to its PRIMARY key (where its record lives):
    // the first present atom it owns, else the synthetic key that will hold it.
    // This guarantees `uses` edges always resolve to a real atom key.
    let mut uses_index: HashMap<String, String> = HashMap::new();
    for (node, present) in model.nodes.iter().zip(&present_by_node) {
        let key = present
            .iter()
            .find(|cn| owns(&node.label, cn))
            .cloned()
            .unwrap_or_else(|| synthetic_key(&node.label));
        uses_index.insert(node.label.clone(), key);
    }

    // Pass C: attach extensions to owned atoms; synthesize planned, decl-missing
    // and collision-shadow atoms.
    let mut to_insert: Vec<(String, Atom)> = Vec::new();
    for (node, present) in model.nodes.iter().zip(&present_by_node) {
        if node.lean_decls.is_empty() {
            // Planned-only: no Lean binding at all.
            report.planned_only += 1;
            let ext = make_extensions(node, &uses_index, None, false, Vec::new(), false);
            to_insert.push((synthetic_key(&node.label), synthetic_atom(node, &ext)));
            report.synthesized += 1;
            continue;
        }
        if present.is_empty() {
            // All bound decls absent from the atom base: represent as a
            // decl-missing synthetic node rather than fabricating a code atom.
            report.decl_missing += 1;
            let ext = make_extensions(node, &uses_index, None, true, Vec::new(), false);
            to_insert.push((synthetic_key(&node.label), synthetic_atom(node, &ext)));
            report.synthesized += 1;
            continue;
        }

        // Node binds >=1 present atom, so it is genuinely bound (whether or not
        // it wins any atom against a colliding later node). Compute the ext
        // content once, the same way for the bound and collision-shadow cases.
        report.nodes_with_decl += 1;
        let missing: Vec<String> = node
            .lean_decls
            .iter()
            .filter(|d| !atoms.contains_key(&code_name_for_decl(d)))
            .cloned()
            .collect();
        if !missing.is_empty() {
            report.partial_missing += 1;
        }
        // Mismatch is per-node: check every present atom this node binds (owned
        // or not) and record it once so counts match `blueprint_stats.py`.
        let mismatch = present.iter().find_map(|cn| {
            let machine = atoms.get(cn).and_then(machine_status);
            mismatch_marker(node.proof_status, machine.as_deref())
        });
        if let Some(m) = &mismatch {
            report.mismatches.push(format!("{}: {m}", node.label));
        }

        let owned: Vec<&String> = present.iter().filter(|cn| owns(&node.label, cn)).collect();
        if owned.is_empty() {
            // Collision loser: every present atom was claimed by a later node.
            // Preserve this node as a shadow synthetic atom so the extract stays
            // node-complete (and keeps its mismatch / missing-decls signal).
            report.collision_shadowed += 1;
            let ext = make_extensions(node, &uses_index, mismatch, false, missing, true);
            to_insert.push((synthetic_key(&node.label), synthetic_atom(node, &ext)));
            report.synthesized += 1;
        } else {
            let ext = make_extensions(node, &uses_index, mismatch, false, missing, false);
            for cn in owned {
                if let Some(atom) = atoms.get_mut(cn) {
                    insert_extensions(atom, &ext);
                }
            }
        }
    }

    // Insert synthetic atoms (idempotent re-run), and flag duplicate keys.
    let mut seen_synthetic: HashMap<String, ()> = HashMap::new();
    for (key, atom) in to_insert {
        if seen_synthetic.insert(key.clone(), ()).is_some() {
            report.duplicate_synthetic += 1;
            eprintln!("warning: duplicate synthetic blueprint atom {key}; keeping the last");
        }
        atoms.insert(key, atom);
    }

    report
}

/// A two-axis histogram over blueprint nodes.
#[derive(Debug, Default, serde::Serialize)]
pub struct AxisCounts {
    pub statement: StatementCounts,
    pub proof: ProofCounts,
}

#[derive(Debug, Default, serde::Serialize)]
pub struct StatementCounts {
    pub none: usize,
    pub blocked: usize,
    pub ready: usize,
    pub formalized: usize,
}

#[derive(Debug, Default, serde::Serialize)]
pub struct ProofCounts {
    pub none: usize,
    pub ready: usize,
    pub proved: usize,
    #[serde(rename = "fully-proved")]
    pub fully_proved: usize,
}

impl AxisCounts {
    fn tally(&mut self, node: &BlueprintNode) {
        match node.statement_status {
            StatementStatus::NonePlanned => self.statement.none += 1,
            StatementStatus::Blocked => self.statement.blocked += 1,
            StatementStatus::Ready => self.statement.ready += 1,
            StatementStatus::Formalized => self.statement.formalized += 1,
        }
        match node.proof_status {
            ProofStatus::None => self.proof.none += 1,
            ProofStatus::Ready => self.proof.ready += 1,
            ProofStatus::Proved => self.proof.proved += 1,
            ProofStatus::FullyProved => self.proof.fully_proved += 1,
        }
    }
}

/// Per-chapter progress, keyed by blueprint chapter.
#[derive(Debug, Default, serde::Serialize)]
pub struct ChapterSummary {
    pub nodes: usize,
    #[serde(flatten)]
    pub axes: AxisCounts,
    #[serde(rename = "theorems-total")]
    pub theorems_total: usize,
    #[serde(rename = "theorems-fully-proved")]
    pub theorems_fully_proved: usize,
}

/// Aggregate progress counts computed from the blueprint model, for the summary
/// sidecar.
#[derive(Debug, serde::Serialize)]
pub struct Summary {
    pub totals: Totals,
    pub all: AxisCounts,
    pub definitions: AxisCounts,
    pub theorems: AxisCounts,
    pub headline: Headline,
    #[serde(rename = "by-chapter")]
    pub by_chapter: BTreeMap<String, ChapterSummary>,
}

#[derive(Debug, Default, serde::Serialize)]
pub struct Totals {
    pub nodes: usize,
    #[serde(rename = "with-lean-decl")]
    pub with_lean_decl: usize,
    #[serde(rename = "planned-only")]
    pub planned_only: usize,
    #[serde(rename = "decl-missing")]
    pub decl_missing: usize,
    /// Bound nodes with a partial decl miss (see `blueprint-missing-decls`).
    #[serde(rename = "partial-missing")]
    pub partial_missing: usize,
    /// Present atoms bound by more than one blueprint node.
    pub collisions: usize,
    pub mismatches: usize,
}

#[derive(Debug, Default, serde::Serialize)]
pub struct Headline {
    #[serde(rename = "theorems-total")]
    pub theorems_total: usize,
    #[serde(rename = "theorems-fully-proved")]
    pub theorems_fully_proved: usize,
    /// Fraction of theorems whose proof is fully formalized (blueprint's
    /// `fully_proved`). This is the headline progress number.
    pub fraction: f64,
}

/// Build the summary from the model and the enrichment report.
pub fn summarize(model: &BlueprintModel, report: &EnrichReport) -> Summary {
    let mut all = AxisCounts::default();
    let mut definitions = AxisCounts::default();
    let mut theorems = AxisCounts::default();
    let mut theorems_total = 0usize;
    let mut theorems_fully_proved = 0usize;
    let mut by_chapter: BTreeMap<String, ChapterSummary> = BTreeMap::new();

    for node in &model.nodes {
        all.tally(node);
        let chapter = by_chapter
            .entry(
                node.chapter
                    .clone()
                    .unwrap_or_else(|| "ungrouped".to_string()),
            )
            .or_default();
        chapter.nodes += 1;
        chapter.axes.tally(node);
        match node.display_kind() {
            NodeKind::Definition => definitions.tally(node),
            NodeKind::Theorem => {
                theorems.tally(node);
                theorems_total += 1;
                chapter.theorems_total += 1;
                if node.proof_status == ProofStatus::FullyProved {
                    theorems_fully_proved += 1;
                    chapter.theorems_fully_proved += 1;
                }
            }
        }
    }

    let fraction = if theorems_total > 0 {
        theorems_fully_proved as f64 / theorems_total as f64
    } else {
        0.0
    };

    Summary {
        totals: Totals {
            nodes: report.nodes_total,
            with_lean_decl: report.nodes_with_decl,
            planned_only: report.planned_only,
            decl_missing: report.decl_missing,
            partial_missing: report.partial_missing,
            collisions: report.collisions,
            mismatches: report.mismatches.len(),
        },
        all,
        definitions,
        theorems,
        headline: Headline {
            theorems_total,
            theorems_fully_proved,
            fraction,
        },
        by_chapter,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::StatusSource;

    fn atom_with_status(status: Option<&str>) -> Atom {
        let mut a = Atom {
            display_name: "x".into(),
            dependencies: Default::default(),
            code_module: String::new(),
            code_path: "Foo.lean".into(),
            code_text: CodeText {
                lines_start: 1,
                lines_end: 2,
            },
            kind: "theorem".into(),
            language: "lean".into(),
            extensions: BTreeMap::new(),
        };
        if let Some(s) = status {
            a.extensions
                .insert("verification-status".into(), Value::String(s.to_string()));
        }
        a
    }

    fn node(
        label: &str,
        decls: &[&str],
        stmt: StatementStatus,
        proof: ProofStatus,
    ) -> BlueprintNode {
        BlueprintNode {
            label: label.into(),
            kind: Some(if label.starts_with("def") {
                NodeKind::Definition
            } else {
                NodeKind::Theorem
            }),
            lean_decls: decls.iter().map(|s| s.to_string()).collect(),
            statement_status: stmt,
            proof_status: proof,
            statement_uses: vec![],
            proof_uses: vec![],
            group: None,
            chapter: None,
            title: None,
            discussion: None,
            status_source: StatusSource::CodeDerived,
        }
    }

    #[test]
    fn enriches_matched_atom() {
        let mut atoms = BTreeMap::new();
        atoms.insert(
            "probe:Foo.bar".to_string(),
            atom_with_status(Some("verified")),
        );
        let mut model = BlueprintModel::default();
        model.nodes.push(node(
            "thm:bar",
            &["Foo.bar"],
            StatementStatus::Formalized,
            ProofStatus::Proved,
        ));

        let report = enrich(&mut atoms, &model);
        assert_eq!(report.nodes_with_decl, 1);
        let a = &atoms["probe:Foo.bar"];
        assert_eq!(
            a.extensions.get("blueprint-label").unwrap().as_str(),
            Some("thm:bar")
        );
        assert_eq!(
            a.extensions.get("blueprint-proof-status").unwrap().as_str(),
            Some("proved")
        );
        // verification-status stays machine-authoritative.
        assert_eq!(
            a.extensions.get("verification-status").unwrap().as_str(),
            Some("verified")
        );
    }

    #[test]
    fn synthesizes_planned_node() {
        let mut atoms: BTreeMap<String, Atom> = BTreeMap::new();
        let mut model = BlueprintModel::default();
        model.nodes.push(node(
            "thm:planned",
            &[],
            StatementStatus::NonePlanned,
            ProofStatus::None,
        ));

        let report = enrich(&mut atoms, &model);
        assert_eq!(report.planned_only, 1);
        let a = &atoms["probe:blueprint:thm:planned"];
        assert_eq!(a.language, "blueprint");
        assert_eq!(a.kind, "blueprint-theorem");
        assert!(!a.is_stub(), "planned atoms must not be stubs");
        assert!(!a.extensions.contains_key("verification-status"));
    }

    #[test]
    fn flags_missing_decl() {
        let mut atoms: BTreeMap<String, Atom> = BTreeMap::new();
        let mut model = BlueprintModel::default();
        model.nodes.push(node(
            "thm:ghost",
            &["Foo.ghost"],
            StatementStatus::Formalized,
            ProofStatus::Proved,
        ));

        let report = enrich(&mut atoms, &model);
        assert_eq!(report.decl_missing, 1);
        let a = &atoms["probe:blueprint:thm:ghost"];
        assert_eq!(
            a.extensions
                .get("blueprint-decl-missing")
                .unwrap()
                .as_bool(),
            Some(true)
        );
    }

    #[test]
    fn flags_status_mismatch() {
        let mut atoms = BTreeMap::new();
        atoms.insert(
            "probe:Foo.bar".to_string(),
            atom_with_status(Some("unverified")),
        );
        let mut model = BlueprintModel::default();
        model.nodes.push(node(
            "thm:bar",
            &["Foo.bar"],
            StatementStatus::Formalized,
            ProofStatus::Proved,
        ));

        let report = enrich(&mut atoms, &model);
        assert_eq!(report.mismatches.len(), 1);
        let a = &atoms["probe:Foo.bar"];
        assert_eq!(
            a.extensions
                .get("blueprint-status-mismatch")
                .unwrap()
                .as_str(),
            Some("claims-proved-but-unverified")
        );
    }

    #[test]
    fn re_enrich_is_idempotent() {
        // A prior pass leaves a mismatch flag; a second pass with a consistent
        // status must clear it (no stale key leak).
        let mut atoms = BTreeMap::new();
        atoms.insert(
            "probe:Foo.bar".to_string(),
            atom_with_status(Some("unverified")),
        );
        let mut model = BlueprintModel::default();
        model.nodes.push(node(
            "thm:bar",
            &["Foo.bar"],
            StatementStatus::Formalized,
            ProofStatus::Proved,
        ));
        let _ = enrich(&mut atoms, &model);
        assert!(atoms["probe:Foo.bar"]
            .extensions
            .contains_key("blueprint-status-mismatch"));

        // Now the machine status agrees: re-enrich must remove the stale flag.
        atoms.get_mut("probe:Foo.bar").unwrap().extensions.insert(
            "verification-status".into(),
            Value::String("verified".into()),
        );
        let _ = enrich(&mut atoms, &model);
        assert!(
            !atoms["probe:Foo.bar"]
                .extensions
                .contains_key("blueprint-status-mismatch"),
            "stale mismatch flag must be cleared on re-enrich"
        );
    }

    #[test]
    fn detects_same_decl_collision() {
        let mut atoms = BTreeMap::new();
        atoms.insert(
            "probe:Foo.bar".to_string(),
            atom_with_status(Some("verified")),
        );
        let mut model = BlueprintModel::default();
        model.nodes.push(node(
            "thm:one",
            &["Foo.bar"],
            StatementStatus::Formalized,
            ProofStatus::Proved,
        ));
        model.nodes.push(node(
            "thm:two",
            &["Foo.bar"],
            StatementStatus::Formalized,
            ProofStatus::Proved,
        ));
        let report = enrich(&mut atoms, &model);
        assert_eq!(report.collisions, 1);
        assert_eq!(report.collision_shadowed, 1, "the loser is preserved");
        assert_eq!(report.nodes_with_decl, 2, "both nodes are bound");
        // Keep-last: the second node wins the real atom.
        assert_eq!(
            atoms["probe:Foo.bar"]
                .extensions
                .get("blueprint-label")
                .unwrap()
                .as_str(),
            Some("thm:two")
        );
        // Node-complete extract: the loser survives as a shadow atom so it is
        // still visible to blueprint_stats.py's per-label grouping.
        let shadow = &atoms["probe:blueprint:thm:one"];
        assert_eq!(shadow.language, "blueprint");
        assert_eq!(
            shadow.extensions.get("blueprint-shadow").unwrap().as_bool(),
            Some(true)
        );
        assert_eq!(
            shadow.extensions.get("blueprint-label").unwrap().as_str(),
            Some("thm:one")
        );
    }

    #[test]
    fn collision_shadow_keeps_mismatch_signal() {
        // Two nodes bind the same present atom whose machine status contradicts
        // the blueprint proof claim. Both must be flagged, including the loser
        // (preserved as a shadow), so no over-claim is silently dropped.
        let mut atoms = BTreeMap::new();
        atoms.insert(
            "probe:Foo.bar".to_string(),
            atom_with_status(Some("unverified")),
        );
        let mut model = BlueprintModel::default();
        model.nodes.push(node(
            "thm:one",
            &["Foo.bar"],
            StatementStatus::Formalized,
            ProofStatus::Proved,
        ));
        model.nodes.push(node(
            "thm:two",
            &["Foo.bar"],
            StatementStatus::Formalized,
            ProofStatus::Proved,
        ));
        let report = enrich(&mut atoms, &model);
        assert_eq!(report.mismatches.len(), 2, "winner and loser both flagged");
        assert_eq!(
            atoms["probe:blueprint:thm:one"]
                .extensions
                .get("blueprint-status-mismatch")
                .unwrap()
                .as_str(),
            Some("claims-proved-but-unverified"),
            "the shadowed loser keeps its mismatch flag"
        );
    }

    #[test]
    fn uses_resolve_to_real_keys_for_decl_missing_target() {
        // A node uses a decl-missing node; the resolved code-name must be the
        // synthetic key that actually exists, not the absent decl's code-name.
        let mut atoms: BTreeMap<String, Atom> = BTreeMap::new();
        let mut model = BlueprintModel::default();
        let mut ghost = node(
            "def:ghost",
            &["Foo.ghost"], // absent from the atom base
            StatementStatus::Formalized,
            ProofStatus::None,
        );
        ghost.kind = Some(NodeKind::Definition);
        model.nodes.push(ghost);
        let mut user = node(
            "thm:user",
            &[], // planned-only, uses the ghost
            StatementStatus::NonePlanned,
            ProofStatus::None,
        );
        user.statement_uses = vec!["def:ghost".to_string()];
        model.nodes.push(user);

        enrich(&mut atoms, &model);
        let uses = atoms["probe:blueprint:thm:user"]
            .extensions
            .get("blueprint-statement-uses")
            .unwrap()
            .as_array()
            .unwrap();
        assert_eq!(uses.len(), 1);
        let resolved = uses[0].as_str().unwrap();
        assert_eq!(resolved, "probe:blueprint:def:ghost");
        assert!(
            atoms.contains_key(resolved),
            "resolved uses target must be a real atom key"
        );
    }

    #[test]
    fn re_enrich_changed_model_drops_stale_synthetics() {
        // A synthetic atom for a node that disappears from the model must not
        // survive a re-run over the already-enriched atom base.
        let mut atoms: BTreeMap<String, Atom> = BTreeMap::new();
        let mut first = BlueprintModel::default();
        first.nodes.push(node(
            "thm:old",
            &[],
            StatementStatus::NonePlanned,
            ProofStatus::None,
        ));
        enrich(&mut atoms, &first);
        assert!(atoms.contains_key("probe:blueprint:thm:old"));

        let mut second = BlueprintModel::default();
        second.nodes.push(node(
            "thm:new",
            &[],
            StatementStatus::NonePlanned,
            ProofStatus::None,
        ));
        let report = enrich(&mut atoms, &second);
        assert!(
            !atoms.contains_key("probe:blueprint:thm:old"),
            "stale synthetic atom must be scrubbed on re-enrich"
        );
        assert!(atoms.contains_key("probe:blueprint:thm:new"));
        assert_eq!(report.nodes_total, 1);
        assert_eq!(atoms.len(), 1, "no leaked atoms");
    }

    #[test]
    fn re_enrich_changed_model_clears_stale_fields_on_bound_atom() {
        // An atom bound in run 1 but not in run 2 must lose its blueprint-* keys.
        let mut atoms = BTreeMap::new();
        atoms.insert(
            "probe:Foo.bar".to_string(),
            atom_with_status(Some("verified")),
        );
        let mut first = BlueprintModel::default();
        first.nodes.push(node(
            "thm:bar",
            &["Foo.bar"],
            StatementStatus::Formalized,
            ProofStatus::Proved,
        ));
        enrich(&mut atoms, &first);
        assert!(atoms["probe:Foo.bar"]
            .extensions
            .contains_key("blueprint-label"));

        // Run 2: the node is gone; the atom must be de-enriched.
        let empty = BlueprintModel::default();
        enrich(&mut atoms, &empty);
        assert!(
            !atoms["probe:Foo.bar"]
                .extensions
                .contains_key("blueprint-label"),
            "stale blueprint-* fields must be cleared when the node disappears"
        );
    }

    #[test]
    fn records_partial_missing_decls() {
        let mut atoms = BTreeMap::new();
        atoms.insert(
            "probe:Foo.present".to_string(),
            atom_with_status(Some("verified")),
        );
        let mut model = BlueprintModel::default();
        model.nodes.push(node(
            "thm:multi",
            &["Foo.present", "Foo.absent"],
            StatementStatus::Formalized,
            ProofStatus::Proved,
        ));
        let report = enrich(&mut atoms, &model);
        assert_eq!(report.nodes_with_decl, 1, "still counts as bound");
        assert_eq!(report.decl_missing, 0, "not the all-absent case");
        assert_eq!(report.partial_missing, 1);
        let missing = atoms["probe:Foo.present"]
            .extensions
            .get("blueprint-missing-decls")
            .unwrap()
            .as_array()
            .unwrap();
        assert_eq!(missing.len(), 1);
        assert_eq!(missing[0].as_str(), Some("Foo.absent"));
    }

    #[test]
    fn mismatch_counted_once_per_node() {
        // A node binding two present decls that both mismatch must count once.
        let mut atoms = BTreeMap::new();
        atoms.insert(
            "probe:Foo.a".to_string(),
            atom_with_status(Some("unverified")),
        );
        atoms.insert("probe:Foo.b".to_string(), atom_with_status(Some("failed")));
        let mut model = BlueprintModel::default();
        model.nodes.push(node(
            "thm:pair",
            &["Foo.a", "Foo.b"],
            StatementStatus::Formalized,
            ProofStatus::Proved,
        ));
        let report = enrich(&mut atoms, &model);
        assert_eq!(report.mismatches.len(), 1, "one entry per node label");
    }

    #[test]
    fn summary_headline_fraction() {
        let mut model = BlueprintModel::default();
        model.nodes.push(node(
            "thm:a",
            &["Foo.a"],
            StatementStatus::Formalized,
            ProofStatus::FullyProved,
        ));
        model.nodes.push(node(
            "thm:b",
            &["Foo.b"],
            StatementStatus::Formalized,
            ProofStatus::None,
        ));
        model.nodes.push(node(
            "def:c",
            &["Foo.c"],
            StatementStatus::Formalized,
            ProofStatus::None,
        ));
        let report = EnrichReport {
            nodes_total: 3,
            ..Default::default()
        };
        let summary = summarize(&model, &report);
        assert_eq!(summary.headline.theorems_total, 2);
        assert_eq!(summary.headline.theorems_fully_proved, 1);
        assert!((summary.headline.fraction - 0.5).abs() < 1e-9);
        assert_eq!(summary.definitions.statement.formalized, 1);
    }
}
