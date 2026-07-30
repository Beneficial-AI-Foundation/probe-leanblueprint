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

use std::collections::{BTreeMap, HashMap, HashSet};

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
    /// Subset of `decl_missing` proved upstream (the complement is a genuine
    /// gap). See `docs/SCHEMA.md` §Semantics → Node classification.
    pub decl_missing_upstream_proved: usize,
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
    /// Labels of fully-proved *theorems* the machine has not contradicted (bound,
    /// no `claims-proved-but-*` mismatch) — the "probe-lean-confirmed" bar. Exact
    /// definition: `docs/SCHEMA.md` §Semantics → Machine reconciliation (P26).
    pub probe_lean_confirmed_proved: Vec<String>,
    /// Labels of fully-proved *theorems* decl-missing here but proved upstream
    /// (see `docs/SCHEMA.md` §Machine reconciliation → upstream-proved).
    pub upstream_proved_theorems: Vec<String>,
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

#[allow(clippy::too_many_arguments)]
fn make_extensions(
    node: &BlueprintNode,
    uses_index: &HashMap<String, String>,
    mismatch: Option<String>,
    decl_missing: bool,
    decl_upstream_proved: bool,
    missing_decls: Vec<String>,
    upstream_decls: Vec<String>,
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
        source_statement_status: node.source_statement_status.clone(),
        source_proof_status: node.source_proof_status.clone(),
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
        decl_upstream_proved,
        missing_decls,
        // Wire evidence that part of a node's binding is proved upstream rather
        // than locally: the upstream-proved decls ABSENT from the atom base
        // (computed per-branch by the caller). Empty (skipped) for a fully-local
        // node; never lists a locally-present decl.
        upstream_decls,
        shadow,
    }
}

/// Every `blueprint-*` extension key this tool may write. Cleared before each
/// enrichment so omitted (`None`/`false`/empty) fields do not persist across
/// re-runs of an already-enriched atom base.
const BLUEPRINT_KEYS: &[&str] = &[
    "blueprint-label",
    "blueprint-kind",
    "blueprint-statement-status",
    "blueprint-proof-status",
    "blueprint-source-statement-status",
    "blueprint-source-proof-status",
    "blueprint-status-source",
    "blueprint-group",
    "blueprint-chapter",
    "blueprint-title",
    "blueprint-discussion",
    "blueprint-statement-uses",
    "blueprint-proof-uses",
    "blueprint-status-mismatch",
    "blueprint-decl-missing",
    "blueprint-decl-upstream-proved",
    "blueprint-missing-decls",
    "blueprint-upstream-decls",
    "blueprint-shadow",
];

fn insert_extensions(atom: &mut Atom, ext: &BlueprintExtensions) {
    // Clear any keys left by a prior enrichment so omitted (None/false/empty)
    // fields do not leak across runs.
    for key in BLUEPRINT_KEYS {
        atom.extensions.remove(*key);
    }
    // A struct of scalars/strings never fails to serialize; on the unreachable
    // error path, leave the blueprint keys cleared rather than panic.
    let value = serde_json::to_value(ext).unwrap_or(Value::Null);
    if let Value::Object(map) = value {
        for (k, v) in map {
            atom.extensions.insert(k, v);
        }
    }
}

/// Drop synthetic blueprint atoms and clear all `blueprint-*` keys from the
/// rest, so enriching an already-enriched atom base (or a merged spine carrying
/// old blueprint atoms) rebuilds cleanly rather than accumulating stale entries.
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
        // Invariant the decl-missing branch relies on: an upstream-proved decl is
        // always one of the node's bindings (the Verso adapter derives it by
        // filtering `lean_decls`). Assert it so a future adapter change can't
        // silently emit `blueprint-upstream-decls` for a decl the node doesn't bind.
        debug_assert!(
            node.external_upstream_proved
                .iter()
                .all(|d| node.lean_decls.contains(d)),
            "external_upstream_proved must be a subset of lean_decls (node {})",
            node.label
        );
        if node.lean_decls.is_empty() {
            // Planned-only: no Lean binding at all.
            report.planned_only += 1;
            let ext = make_extensions(
                node,
                &uses_index,
                None,
                false,
                false,
                Vec::new(),
                Vec::new(),
                false,
            );
            to_insert.push((synthetic_key(&node.label), synthetic_atom(node, &ext)));
            report.synthesized += 1;
            continue;
        }
        if present.is_empty() {
            // All bound decls absent from the atom base: represent as a
            // decl-missing synthetic node rather than fabricating a code atom.
            //
            // Distinguish two very different decl-missing cases: a node whose
            // every binding is an *upstream* decl the renderer proved (Verso
            // `outWorkspace` + present + proved) is machine-proved elsewhere and
            // just out of this project's extract scope — not a genuine gap. Only
            // possible for code-derived (Verso) status; Massot never sets it.
            report.decl_missing += 1;
            let upstream_proved = !node.external_upstream_proved.is_empty()
                && node
                    .lean_decls
                    .iter()
                    .all(|d| node.external_upstream_proved.contains(d));
            if upstream_proved {
                report.decl_missing_upstream_proved += 1;
                if node.display_kind() == NodeKind::Theorem
                    && node.proof_status == ProofStatus::FullyProved
                {
                    report.upstream_proved_theorems.push(node.label.clone());
                }
            }
            // Every binding is absent here, so the absent-upstream set is exactly
            // the node's upstream-proved decls (lists them whether or not ALL
            // bindings are upstream — a partial gap still names its upstream part).
            let ext = make_extensions(
                node,
                &uses_index,
                None,
                true,
                upstream_proved,
                Vec::new(),
                node.external_upstream_proved.clone(),
                false,
            );
            to_insert.push((synthetic_key(&node.label), synthetic_atom(node, &ext)));
            report.synthesized += 1;
            continue;
        }

        // Node binds >=1 present atom, so it is genuinely bound (whether or not
        // it wins any atom against a colliding later node). Compute the ext
        // content once, the same way for the bound and collision-shadow cases.
        report.nodes_with_decl += 1;
        // Partition the absent bindings into a genuine gap (`missing`) vs. decls
        // the renderer proved out-of-workspace (`upstream_absent`). An
        // upstream-proved decl is expected to be absent here (it lives in a
        // dependency), so it is NOT a gap: it is recorded in
        // `blueprint-upstream-decls` instead. Without this split a *mixed* node
        // (one present local decl + one absent upstream-proved decl) would be
        // mislabeled partial-missing and dropped from the confirmed count. A
        // present decl (even if upstream-proved, e.g. via a merged spine) is
        // neither, so it never appears in either list.
        let mut missing: Vec<String> = Vec::new();
        let mut upstream_absent: Vec<String> = Vec::new();
        for d in &node.lean_decls {
            if atoms.contains_key(&code_name_for_decl(d)) {
                continue; // present locally
            }
            if node.external_upstream_proved.contains(d) {
                upstream_absent.push(d.clone());
            } else {
                missing.push(d.clone());
            }
        }
        if !missing.is_empty() {
            report.partial_missing += 1;
        }
        // A fully-proved theorem whose *entire* Lean binding is present counts as
        // probe-lean-confirmed unless the machine contradicts it (recorded below).
        // `missing.is_empty()` excludes partial-missing nodes: if any bound decl is
        // absent from the extract, probe-lean can't back the whole claim, so it is
        // not confirmed (it stays a partial-missing side count). Definitions and
        // unbound/decl-missing nodes never reach here. The bar is "not
        // contradicted", not "affirmatively verified" — normative definition in
        // docs/SCHEMA.md §Semantics → Machine reconciliation. NB this scores before
        // `enrich_verification_status` propagation in main; harmless under this bar
        // (a `verified` status never fires a mismatch), but would be load-bearing
        // if the bar ever required an affirmative status.
        if node.display_kind() == NodeKind::Theorem
            && node.proof_status == ProofStatus::FullyProved
            && missing.is_empty()
        {
            // Provisionally confirmed; removed just below if a mismatch fires.
            report.probe_lean_confirmed_proved.push(node.label.clone());
        }
        // Mismatch is per-node: check every present atom this node binds (owned
        // or not) and record it once so counts match `blueprint_stats.py`.
        let mismatch = present.iter().find_map(|cn| {
            let machine = atoms.get(cn).and_then(machine_status);
            mismatch_marker(node.proof_status, machine.as_deref())
        });
        if let Some(m) = &mismatch {
            report.mismatches.push(format!("{}: {m}", node.label));
            // The machine contradicts the proof claim, so it is not confirmed.
            report
                .probe_lean_confirmed_proved
                .retain(|l| l != &node.label);
        }

        let owned: Vec<&String> = present.iter().filter(|cn| owns(&node.label, cn)).collect();
        if owned.is_empty() {
            // Collision loser: every present atom was claimed by a later node.
            // Preserve this node as a shadow synthetic atom so the extract stays
            // node-complete (and keeps its mismatch / missing-decls signal).
            report.collision_shadowed += 1;
            let ext = make_extensions(
                node,
                &uses_index,
                mismatch,
                false,
                false,
                missing,
                upstream_absent,
                true,
            );
            to_insert.push((synthetic_key(&node.label), synthetic_atom(node, &ext)));
            report.synthesized += 1;
        } else {
            let ext = make_extensions(
                node,
                &uses_index,
                mismatch,
                false,
                false,
                missing,
                upstream_absent,
                false,
            );
            for cn in owned {
                if let Some(atom) = atoms.get_mut(cn) {
                    insert_extensions(atom, &ext);
                }
            }
        }
    }

    // Insert synthetic atoms (idempotent re-run), and flag duplicate keys.
    let mut seen_synthetic: HashSet<String> = HashSet::new();
    for (key, atom) in to_insert {
        if !seen_synthetic.insert(key.clone()) {
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
    /// Subset of `decl-missing` proved upstream; the rest are genuine gaps.
    /// `docs/SCHEMA.md` §Semantics → Node classification.
    #[serde(rename = "decl-missing-upstream-proved")]
    pub decl_missing_upstream_proved: usize,
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
    /// Theorems the *blueprint* claims fully proved; can over-claim for `declared`
    /// (Massot) blueprints. `docs/SCHEMA.md` §headline.
    #[serde(rename = "theorems-fully-proved")]
    pub theorems_fully_proved: usize,
    /// Fully-proved theorems the machine has not refuted (bound + no mismatch) —
    /// the honest headline number. Exact "not-refuted, not affirmatively-verified"
    /// bar: `docs/SCHEMA.md` §Semantics → Machine reconciliation (P26).
    #[serde(rename = "theorems-fully-proved-probe-lean-confirmed")]
    pub theorems_fully_proved_probe_lean_confirmed: usize,
    /// Fully-proved theorems decl-missing here but proved out-of-workspace per the
    /// renderer (a dependency): neither confirmed locally nor a gap. Surfaced as
    /// `+K upstream-proved`; 0 for Massot. `docs/SCHEMA.md` §Machine reconciliation.
    #[serde(rename = "theorems-fully-proved-upstream-proved")]
    pub theorems_fully_proved_upstream_proved: usize,
    /// Fraction of theorems the blueprint claims fully proved.
    pub fraction: f64,
    /// Fraction of theorems probe-lean-confirmed fully proved.
    #[serde(rename = "fraction-probe-lean-confirmed")]
    pub fraction_probe_lean_confirmed: f64,
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

    let theorems_fully_proved_probe_lean_confirmed = report.probe_lean_confirmed_proved.len();
    let fraction = if theorems_total > 0 {
        theorems_fully_proved as f64 / theorems_total as f64
    } else {
        0.0
    };
    let fraction_probe_lean_confirmed = if theorems_total > 0 {
        theorems_fully_proved_probe_lean_confirmed as f64 / theorems_total as f64
    } else {
        0.0
    };

    Summary {
        totals: Totals {
            nodes: report.nodes_total,
            with_lean_decl: report.nodes_with_decl,
            planned_only: report.planned_only,
            decl_missing: report.decl_missing,
            decl_missing_upstream_proved: report.decl_missing_upstream_proved,
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
            theorems_fully_proved_probe_lean_confirmed,
            theorems_fully_proved_upstream_proved: report.upstream_proved_theorems.len(),
            fraction,
            fraction_probe_lean_confirmed,
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
            external_upstream_proved: vec![],
            statement_status: stmt,
            proof_status: proof,
            source_statement_status: None,
            source_proof_status: None,
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
        // A plain missing decl is a genuine gap, not upstream-proved.
        assert_eq!(report.decl_missing_upstream_proved, 0);
        assert!(!a.extensions.contains_key("blueprint-decl-upstream-proved"));
    }

    /// A fully-proved theorem bound only to an upstream (out-of-workspace) decl
    /// the renderer proved is decl-missing *here* but counts as upstream-proved,
    /// not probe-lean-confirmed and not a genuine gap.
    #[test]
    fn classifies_upstream_proved_decl_missing() {
        let mut atoms: BTreeMap<String, Atom> = BTreeMap::new();
        let mut model = BlueprintModel::default();
        let mut n = node(
            "thm:upstream",
            &["Nat.mul_assoc"],
            StatementStatus::Formalized,
            ProofStatus::FullyProved,
        );
        n.external_upstream_proved = vec!["Nat.mul_assoc".to_string()];
        model.nodes.push(n);

        let report = enrich(&mut atoms, &model);
        assert_eq!(report.decl_missing, 1);
        assert_eq!(report.decl_missing_upstream_proved, 1);
        assert_eq!(report.upstream_proved_theorems, vec!["thm:upstream"]);
        // Upstream-proved is NOT local machine-confirmation.
        assert!(report.probe_lean_confirmed_proved.is_empty());
        let a = &atoms["probe:blueprint:thm:upstream"];
        assert_eq!(
            a.extensions
                .get("blueprint-decl-upstream-proved")
                .and_then(|v| v.as_bool()),
            Some(true)
        );
        // The (all-absent, all-upstream) binding is listed on the wire too.
        assert_eq!(
            a.extensions
                .get("blueprint-upstream-decls")
                .and_then(|v| v.as_array())
                .map(|a| a.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>()),
            Some(vec!["Nat.mul_assoc"])
        );
    }

    /// A *partially*-upstream decl-missing node (binds one out-of-workspace-proved
    /// decl plus one genuinely-absent decl, none present) is a genuine gap: it must
    /// NOT be flagged `blueprint-decl-upstream-proved`, but it must still list its
    /// upstream part in `blueprint-upstream-decls`. This is the shape Finding 2
    /// was about — the field is "upstream-proved AND absent", not "all bindings
    /// upstream".
    #[test]
    fn partially_upstream_decl_missing_lists_upstream_without_bool() {
        let mut atoms: BTreeMap<String, Atom> = BTreeMap::new();
        let mut model = BlueprintModel::default();
        let mut n = node(
            "thm:partial_upstream",
            &["Up.done", "Foo.absent"],
            StatementStatus::Formalized,
            ProofStatus::FullyProved,
        );
        n.external_upstream_proved = vec!["Up.done".to_string()];
        model.nodes.push(n);

        let report = enrich(&mut atoms, &model);
        assert_eq!(report.decl_missing, 1);
        // Not ALL bindings are upstream, so it is a genuine gap.
        assert_eq!(report.decl_missing_upstream_proved, 0);
        assert!(report.upstream_proved_theorems.is_empty());
        assert!(report.probe_lean_confirmed_proved.is_empty());
        let a = &atoms["probe:blueprint:thm:partial_upstream"];
        assert!(
            !a.extensions.contains_key("blueprint-decl-upstream-proved"),
            "a partial gap is not a fully-upstream node"
        );
        assert_eq!(
            a.extensions
                .get("blueprint-upstream-decls")
                .and_then(|v| v.as_array())
                .map(|a| a.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>()),
            Some(vec!["Up.done"]),
            "the upstream part is still listed, the genuine gap is not"
        );
    }

    /// A fully-proved theorem binding two decls with only one present is bound
    /// (so not decl-missing) but *partial-missing*, and must NOT count as
    /// probe-lean-confirmed: part of its Lean binding is absent, so probe-lean
    /// can't back the whole claim.
    #[test]
    fn partial_missing_fully_proved_is_not_confirmed() {
        let mut atoms = BTreeMap::new();
        atoms.insert(
            "probe:Foo.present".to_string(),
            atom_with_status(Some("verified")),
        );
        let mut model = BlueprintModel::default();
        model.nodes.push(node(
            "thm:partial",
            &["Foo.present", "Foo.absent"],
            StatementStatus::Formalized,
            ProofStatus::FullyProved,
        ));

        let report = enrich(&mut atoms, &model);
        assert_eq!(report.nodes_with_decl, 1, "bound: one decl present");
        assert_eq!(report.partial_missing, 1);
        assert!(
            report.probe_lean_confirmed_proved.is_empty(),
            "a partial-missing fully-proved theorem must not be confirmed"
        );
    }

    /// A *mixed* fully-proved theorem — one present local decl plus one absent
    /// decl the renderer proved out-of-workspace — is fully backed (present
    /// locally and proved upstream), NOT partial-missing. The upstream decl must
    /// be excluded from `missing` so the node is confirmed and that decl isn't
    /// mislabeled a gap.
    #[test]
    fn mixed_local_present_plus_upstream_proved_is_confirmed() {
        let mut atoms = BTreeMap::new();
        atoms.insert(
            "probe:MyProj.thm".to_string(),
            atom_with_status(Some("verified")),
        );
        let mut model = BlueprintModel::default();
        let mut n = node(
            "thm:mixed",
            &["MyProj.thm", "Nat.mul_assoc"],
            StatementStatus::Formalized,
            ProofStatus::FullyProved,
        );
        n.external_upstream_proved = vec!["Nat.mul_assoc".to_string()];
        model.nodes.push(n);

        let report = enrich(&mut atoms, &model);
        assert_eq!(
            report.nodes_with_decl, 1,
            "bound via the present local decl"
        );
        assert_eq!(
            report.partial_missing, 0,
            "the absent decl is upstream-proved, not a gap"
        );
        assert_eq!(report.probe_lean_confirmed_proved, vec!["thm:mixed"]);
        // Wire contract (SCHEMA.md §Node classification / Machine reconciliation):
        // the upstream decl must NOT appear in blueprint-missing-decls, and MUST
        // be surfaced in blueprint-upstream-decls so a consumer can tell a mixed
        // (local + upstream) binding from a fully-local one.
        let a = &atoms["probe:MyProj.thm"];
        assert!(
            !a.extensions.contains_key("blueprint-missing-decls"),
            "upstream decl is not a partial-missing gap"
        );
        assert_eq!(
            a.extensions
                .get("blueprint-upstream-decls")
                .and_then(|v| v.as_array())
                .map(|a| a.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>()),
            Some(vec!["Nat.mul_assoc"]),
            "mixed node must carry its upstream decls on the wire"
        );
    }

    /// A fully-LOCAL confirmed node carries no `blueprint-upstream-decls` (the
    /// field is the discriminator between fully-local and mixed backing). This
    /// pins the SCHEMA contract: no upstream marker unless part of the binding
    /// is genuinely upstream.
    #[test]
    fn fully_local_confirmed_node_has_no_upstream_marker() {
        let mut atoms = BTreeMap::new();
        atoms.insert(
            "probe:Foo.local".to_string(),
            atom_with_status(Some("verified")),
        );
        let mut model = BlueprintModel::default();
        model.nodes.push(node(
            "thm:local",
            &["Foo.local"],
            StatementStatus::Formalized,
            ProofStatus::FullyProved,
        ));
        let report = enrich(&mut atoms, &model);
        assert_eq!(report.probe_lean_confirmed_proved, vec!["thm:local"]);
        assert!(!atoms["probe:Foo.local"]
            .extensions
            .contains_key("blueprint-upstream-decls"));
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
