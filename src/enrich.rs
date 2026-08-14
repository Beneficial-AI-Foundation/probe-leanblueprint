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

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use probe::types::{Atom, CodeText};
use serde_json::Value;

use crate::model::{
    BlueprintExtensions, BlueprintModel, BlueprintNode, NodeKind, ProofStatus, StatementStatus,
};

/// Root folder of the virtual location hierarchy for node atoms: a node atom
/// lives at `blueprint/<chapter-slug>`, giving frontends a real place to file
/// paper items (one `blueprint/` tree beside the code folders, one subfolder
/// per chapter). Always non-empty, so P3 stub detection
/// (`code-path == "" && lines 0,0`) never misclassifies a node atom as a stub.
// @kb: kb/engineering/properties.md#p3-stub-detection-is-structural
const BLUEPRINT_CODE_PATH_ROOT: &str = "blueprint";

/// Location component for nodes whose blueprint gives no chapter — the same
/// word the summary's by-chapter grouping uses for them.
const UNGROUPED: &str = "ungrouped";

/// Longest slug a chapter or group contributes to a location. Keeps the
/// synthesized `code-path`/`code-module` far below consumers' path-column
/// limits (VeriLib stores paths in 512-char columns) even with both levels
/// present.
const LOCATION_COMPONENT_MAX: usize = 64;

/// One path/module component from a raw chapter or group name, or `None` when
/// nothing survives sanitization. Only alphanumerics (unicode included), `_`
/// and `-` pass through; every other char — whitespace, path separators, dots,
/// URL delimiters, control chars — collapses into a single `-`, so a raw name
/// can never add path segments, module levels, or unprintable bytes. Distinct
/// raw names may share a slug (`A B` and `A.B` both become `A-B`); that is
/// accepted — the slug is a display grouping, and the adapter-provided
/// originals stay in `blueprint-chapter` / `blueprint-group`.
fn location_component(raw: &str) -> Option<String> {
    let mut out = String::with_capacity(raw.len().min(LOCATION_COMPONENT_MAX));
    for c in raw.chars() {
        if out.len() >= LOCATION_COMPONENT_MAX {
            break;
        }
        if c.is_alphanumeric() || matches!(c, '_' | '-') {
            out.push(c);
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    let out = out.trim_end_matches('-');
    if out.is_empty() {
        None
    } else {
        Some(out.to_string())
    }
}

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
    /// For each bound node (by label), the present atom keys its binding
    /// resolves to. Consumed by [`derive_synthetic_verification`] so a bound
    /// node atom (collision shadows included) can aggregate the machine
    /// statuses of the decls it is genuinely bound to.
    pub bound_bindings: HashMap<String, Vec<String>>,
    /// Number of node atoms actually present after insertion — the
    /// per-blueprint-node record count. Equals `nodes_total` unless a
    /// duplicate label or a synthetic-key collision dropped one (both warn).
    pub node_atoms: usize,
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
    node_class: Option<&str>,
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
        node_class: node_class.map(str::to_string),
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
    "blueprint-node-class",
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
    let chapter = node
        .chapter
        .as_deref()
        .and_then(location_component)
        .unwrap_or_else(|| UNGROUPED.to_string());
    let mut code_module = format!("Blueprint.{chapter}");
    if let Some(group) = node.group.as_deref().and_then(location_component) {
        code_module.push('.');
        code_module.push_str(&group);
    }
    let mut atom = Atom {
        display_name,
        dependencies: Default::default(),
        code_module,
        code_path: format!("{BLUEPRINT_CODE_PATH_ROOT}/{chapter}"),
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
                         ({prev}, {}); the last binder keeps the atom (the earlier node's \
                         node atom becomes a shadow only if it owns no other atom)",
                        node.label
                    );
                }
            }
            owner.insert(cn.clone(), node.label.clone());
        }
    }
    let owns = |label: &str, cn: &String| owner.get(cn).map(|l| l == label).unwrap_or(false);

    // Pass B: two uses-edge resolutions, one per atom class (the resolution
    // rule is normative in docs/SCHEMA.md §Node atoms → Uses resolution):
    //
    // - `uses_index_lean` — today's resolution, used ONLY when enriching real
    //   Lean atoms (whose bytes are frozen): a used label resolves to its
    //   primary code representative — the first present atom it owns, else its
    //   synthetic key.
    // - `uses_index_node` — used on node atoms: a used label always resolves to
    //   that label's node-atom key, so the node atoms + their uses edges form a
    //   closed per-node graph matching the Verso blueprint.
    let mut uses_index_lean: HashMap<String, String> = HashMap::new();
    let mut uses_index_node: HashMap<String, String> = HashMap::new();
    for (node, present) in model.nodes.iter().zip(&present_by_node) {
        let key = present
            .iter()
            .find(|cn| owns(&node.label, cn))
            .cloned()
            .unwrap_or_else(|| synthetic_key(&node.label));
        uses_index_lean.insert(node.label.clone(), key);
        uses_index_node.insert(node.label.clone(), synthetic_key(&node.label));
    }
    // A used label with no model node still resolves to `probe:blueprint:<label>`
    // (historical fallback), leaving a dangling edge in the node graph — surface
    // each such label once so "closed graph" degradation is visible, not silent.
    {
        let mut dangling: BTreeSet<&str> = BTreeSet::new();
        for node in &model.nodes {
            for used in node.statement_uses.iter().chain(&node.proof_uses) {
                if !uses_index_node.contains_key(used) {
                    dangling.insert(used);
                }
            }
        }
        for label in dangling {
            eprintln!(
                "warning: uses edge references label {label:?} which has no blueprint node; \
                 emitting a dangling probe:blueprint:{label} target"
            );
        }
    }

    // Pass C: attach extensions to owned atoms, and synthesize exactly one node
    // atom per blueprint node (bound — collision shadows included — planned-only,
    // and decl-missing), so the extract carries one `language: "blueprint"`
    // record per Verso node.
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
                &uses_index_node,
                None,
                false,
                false,
                Vec::new(),
                Vec::new(),
                false,
                Some("planned-only"),
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
                &uses_index_node,
                None,
                true,
                upstream_proved,
                Vec::new(),
                node.external_upstream_proved.clone(),
                false,
                Some("decl-missing"),
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
        let is_shadow = owned.is_empty();
        if is_shadow {
            // Collision loser: every present atom was claimed by a later node.
            // Its node atom below is the only record carrying its label; the
            // `blueprint-shadow` flag marks that special case.
            report.collision_shadowed += 1;
        } else {
            // Per-decl enrichment layer: the owned real atoms gain the node's
            // blueprint-* fields, uses edges resolved to code representatives.
            // These bytes are frozen — node-class and node-edge resolution
            // exist only on the node atom.
            let ext = make_extensions(
                node,
                &uses_index_lean,
                mismatch.clone(),
                false,
                false,
                missing.clone(),
                upstream_absent.clone(),
                false,
                None,
            );
            for cn in owned {
                if let Some(atom) = atoms.get_mut(cn) {
                    insert_extensions(atom, &ext);
                }
            }
        }
        // Node-atom layer: every bound node leaves exactly one
        // `probe:blueprint:<label>` record (Verso-node parity), aggregating its
        // whole binding.
        report
            .bound_bindings
            .insert(node.label.clone(), present.clone());
        let ext = make_extensions(
            node,
            &uses_index_node,
            mismatch,
            false,
            false,
            missing,
            upstream_absent,
            is_shadow,
            Some("bound"),
        );
        let mut atom = synthetic_atom(node, &ext);
        // A bound node atom genuinely binds these present decls; carrying them
        // as dependencies keeps its derived verification-status stable under a
        // later `probe enrich` (the hub recomputes over the real closure
        // instead of vacuously upgrading an empty one). Edges point
        // synthetic -> real only, so real-atom statuses are unaffected.
        atom.dependencies = present.iter().cloned().collect();
        to_insert.push((synthetic_key(&node.label), atom));
        report.synthesized += 1;
    }

    // Insert synthetic atoms (idempotent re-run), and flag duplicate keys.
    let mut seen_synthetic: HashSet<String> = HashSet::new();
    for (key, atom) in to_insert {
        if !seen_synthetic.insert(key.clone()) {
            report.duplicate_synthetic += 1;
            eprintln!("warning: duplicate synthetic blueprint atom {key}; keeping the last");
        }
        // After the scrub only non-blueprint atoms survive, so a collision here
        // means the synthetic key would clobber a real/foreign atom from the
        // input (a decl or merged-spine key that happens to spell
        // `probe:blueprint:<label>`). Keep the input atom and drop the
        // synthetic rather than destroying data.
        if let Some(existing) = atoms.get(&key) {
            if existing.language != "blueprint" {
                eprintln!(
                    "warning: synthetic key {key} collides with an existing {} atom from the \
                     input; keeping the input atom and skipping the synthetic",
                    existing.language
                );
                continue;
            }
        }
        atoms.insert(key, atom);
    }
    report.node_atoms = atoms.values().filter(|a| a.language == "blueprint").count();

    report
}

/// How a node atom classifies for status derivation, read off the join outcome
/// by the caller.
enum SyntheticClass<'a> {
    Bound {
        /// Present atom keys the node binds (collision shadows included: the
        /// atoms lost to later nodes).
        bindings: &'a [String],
        /// Count of genuinely-missing bound decls (`blueprint-missing-decls`).
        missing_count: usize,
        /// Count of upstream-proved absent decls (`blueprint-upstream-decls`).
        upstream_count: usize,
    },
    DeclMissing {
        upstream_proved: bool,
    },
    PlannedOnly,
}

/// Derived `verification-status` (and `trusted-reason`) for one node atom.
/// Returns `(status, reason)`.
///
/// A bound node atom's status aggregates its whole binding, one *component*
/// per bound decl: each present decl contributes its final machine status,
/// each genuinely-missing decl contributes `unverified`, and each
/// upstream-proved absent decl contributes `trusted`. Aggregation is
/// failure-first and trust-sticky: any `failed` component → `failed`; else any
/// `unverified`/absent/unknown component → `unverified`; else any `trusted`
/// component → `trusted` (an attestation anywhere in the binding caps the
/// whole at attested — ranking `trusted` between the machine rungs would
/// instead hide the trust boundary); else any `verified` → `verified`; else
/// `transitively-verified`. The reason is emitted only when the contributing
/// trusted components agree on a single one (present decls' own
/// `trusted-reason`s, plus `"upstream-proved"` for upstream components);
/// disagreement omits it.
fn derived_status_for(
    node: &BlueprintNode,
    class: SyntheticClass<'_>,
    atoms: &BTreeMap<String, Atom>,
) -> (String, Option<String>) {
    use crate::model::StatusSource;

    // A human `\leanok` claim of a complete proof: human-attested, so `trusted`
    // rather than a machine-vocabulary status (it may well be proven in another
    // repo, but nothing here checked it).
    let declared_proved =
        node.status_source == StatusSource::Declared && node.proof_status.claims_proved();

    if let SyntheticClass::Bound {
        bindings,
        missing_count,
        upstream_count,
    } = class
    {
        let statuses: Vec<Option<String>> = bindings
            .iter()
            .map(|key| atoms.get(key).and_then(machine_status))
            .collect();
        let has = |s: &str| statuses.iter().any(|st| st.as_deref() == Some(s));
        let has_unknown = statuses.iter().any(|st| {
            !matches!(
                st.as_deref(),
                Some("failed" | "unverified" | "verified" | "trusted" | "transitively-verified")
            )
        });
        let status = if bindings.is_empty() && missing_count == 0 && upstream_count == 0 {
            // Degenerate: nothing to aggregate (should not happen for a bound node).
            "unverified"
        } else if has("failed") {
            "failed"
        } else if has("unverified") || has_unknown || missing_count > 0 {
            "unverified"
        } else if has("trusted") || upstream_count > 0 {
            "trusted"
        } else if has("verified") {
            "verified"
        } else {
            "transitively-verified"
        };
        // Keep inherited trust auditable, but only when unambiguous: collect
        // the distinct reasons across the trusted components and emit the
        // single agreed one, or nothing.
        let reason = if status == "trusted" {
            let mut reasons: Vec<String> = bindings
                .iter()
                .filter_map(|key| {
                    let atom = atoms.get(key)?;
                    if machine_status(atom).as_deref() == Some("trusted") {
                        atom.extensions
                            .get("trusted-reason")
                            .and_then(|v| v.as_str())
                            .map(str::to_string)
                    } else {
                        None
                    }
                })
                .collect();
            if upstream_count > 0 {
                reasons.push("upstream-proved".to_string());
            }
            reasons.sort();
            reasons.dedup();
            match reasons.as_slice() {
                [only] => Some(only.clone()),
                _ => None,
            }
        } else {
            None
        };
        return (status.to_string(), reason);
    }
    if let SyntheticClass::DeclMissing { upstream_proved } = class {
        if upstream_proved {
            // The renderer proved every binding out-of-workspace (present +
            // proved in a dependency): machine-checked elsewhere, so `trusted`.
            return ("trusted".to_string(), Some("upstream-proved".to_string()));
        }
        if declared_proved {
            return ("trusted".to_string(), Some("declared".to_string()));
        }
        return ("unverified".to_string(), None);
    }
    // Planned-only: no binding at all — there is nothing checkable behind the
    // node, so no machine-vocabulary status is ever minted here. A code-derived
    // proved claim on an unbound node is a contradiction (the Verso renderer
    // requires associated code to judge a proof), i.e. likely manifest drift or
    // lost preview linkage: losing binding evidence must not *improve* the
    // status, so it derives `unverified`, loudly.
    if node.status_source == StatusSource::CodeDerived && node.proof_status.claims_proved() {
        eprintln!(
            "warning: code-derived planned-only node {} claims proof status {:?} but binds no \
             declaration; deriving \"unverified\" (possible manifest drift)",
            node.label, node.proof_status
        );
    }
    if declared_proved {
        return ("trusted".to_string(), Some("declared".to_string()));
    }
    ("unverified".to_string(), None)
}

/// Stamp a derived `verification-status` (plus `trusted-reason` where trust is
/// attested) onto every node atom (`language: "blueprint"`), so the field is
/// total across the node atoms. Real (`language: "lean"`) atoms are never
/// touched — their machine status stays authoritative (P26).
///
/// Must run AFTER the hub's `enrich_verification_status` propagation, for two
/// reasons: an unbound node atom's derived `"verified"` must not be vacuously
/// upgraded to `"transitively-verified"` (empty dependency lists, so the
/// reverse-BFS would see a trivially clean closure), and a bound node atom
/// aggregates its decls' *final* (post-propagation) machine statuses. Statuses
/// can never feed back into real-atom propagation: no code atom depends on a
/// node atom (`blueprint-*-uses` edges are extension-only).
///
/// Each node class derives from the strongest evidence available (normative
/// decision tree: `docs/SCHEMA.md` §Derived verification-status). Returns the
/// number of synthetic atoms stamped.
pub fn derive_synthetic_verification(
    atoms: &mut BTreeMap<String, Atom>,
    model: &BlueprintModel,
    report: &EnrichReport,
) -> usize {
    let node_by_label: HashMap<&str, &BlueprintNode> =
        model.nodes.iter().map(|n| (n.label.as_str(), n)).collect();

    // Two phases: shadow inheritance reads other atoms, so collect the updates
    // over an immutable view first, then apply.
    let mut updates: Vec<(String, String, Option<String>)> = Vec::new();
    for (key, atom) in atoms.iter() {
        if atom.language != "blueprint" {
            continue;
        }
        let ext_str = |k: &str| atom.extensions.get(k).and_then(|v| v.as_str());
        let ext_bool = |k: &str| atom.extensions.get(k).and_then(|v| v.as_bool()) == Some(true);
        let Some(node) = ext_str("blueprint-label").and_then(|l| node_by_label.get(l)) else {
            // A synthetic without a resolvable node (should not happen: enrich
            // builds every synthetic from a model node in the same run).
            continue;
        };
        let ext_len = |k: &str| {
            atom.extensions
                .get(k)
                .and_then(|v| v.as_array())
                .map_or(0, |a| a.len())
        };
        // Classify by the explicit discriminator. `enrich()` writes it on every
        // node atom in the same run (the only supported input to this pass), so
        // anything else is a malformed input: derive conservatively and warn
        // rather than guess from older flags.
        let class = match ext_str("blueprint-node-class") {
            Some("bound") => SyntheticClass::Bound {
                bindings: report
                    .bound_bindings
                    .get(&node.label)
                    .map(|b| b.as_slice())
                    .unwrap_or(&[]),
                missing_count: ext_len("blueprint-missing-decls"),
                upstream_count: ext_len("blueprint-upstream-decls"),
            },
            Some("decl-missing") => SyntheticClass::DeclMissing {
                upstream_proved: ext_bool("blueprint-decl-upstream-proved"),
            },
            Some("planned-only") => SyntheticClass::PlannedOnly,
            other => {
                eprintln!(
                    "warning: node atom {key} has {} blueprint-node-class; \
                     deriving \"unverified\"",
                    other.map_or("no".to_string(), |o| format!("unknown {o:?}"))
                );
                updates.push((key.clone(), "unverified".to_string(), None));
                continue;
            }
        };
        let (status, reason) = derived_status_for(node, class, atoms);
        updates.push((key.clone(), status, reason));
    }

    let count = updates.len();
    for (key, status, reason) in updates {
        if let Some(atom) = atoms.get_mut(&key) {
            atom.extensions
                .insert("verification-status".to_string(), Value::String(status));
            match reason {
                Some(r) => {
                    atom.extensions
                        .insert("trusted-reason".to_string(), Value::String(r));
                }
                None => {
                    atom.extensions.remove("trusted-reason");
                }
            }
        }
    }
    count
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
    /// Node atoms emitted (`language: "blueprint"`, one per blueprint node) —
    /// the checkable invariant against the Verso node count: equals `nodes`
    /// unless a duplicate label or synthetic-key collision dropped one.
    #[serde(rename = "node-atoms")]
    pub node_atoms: usize,
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
                    .as_deref()
                    .map(str::trim)
                    .filter(|c| !c.is_empty())
                    .map(str::to_string)
                    .unwrap_or_else(|| UNGROUPED.to_string()),
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
            node_atoms: report.node_atoms,
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

    // --- derived verification-status for synthetic atoms ---

    /// Run the same pipeline order as `run_extract`: join, hub propagation,
    /// then derived statuses for synthetics.
    fn enrich_propagate_derive(
        atoms: &mut BTreeMap<String, Atom>,
        model: &BlueprintModel,
    ) -> EnrichReport {
        let report = enrich(atoms, model);
        probe::commands::propagate::enrich_verification_status(atoms);
        derive_synthetic_verification(atoms, model, &report);
        report
    }

    fn vs(atom: &Atom) -> Option<&str> {
        atom.extensions
            .get("verification-status")
            .and_then(|v| v.as_str())
    }

    fn trusted_reason(atom: &Atom) -> Option<&str> {
        atom.extensions
            .get("trusted-reason")
            .and_then(|v| v.as_str())
    }

    /// A planned-only node binds nothing, so no machine-vocabulary status is
    /// ever minted for it — a code-derived proved/fully-proved claim on an
    /// unbound node signals manifest drift (losing binding evidence must not
    /// improve the status), and derives "unverified" like the rest.
    #[test]
    fn derives_planned_only_code_derived_always_unverified() {
        for proof in [
            ProofStatus::None,
            ProofStatus::Ready,
            ProofStatus::Proved,
            ProofStatus::FullyProved,
        ] {
            let mut atoms: BTreeMap<String, Atom> = BTreeMap::new();
            let mut model = BlueprintModel::default();
            model
                .nodes
                .push(node("thm:p", &[], StatementStatus::Ready, proof));
            enrich_propagate_derive(&mut atoms, &model);
            let a = &atoms["probe:blueprint:thm:p"];
            assert_eq!(vs(a), Some("unverified"), "proof rung {proof:?}");
            assert_eq!(trusted_reason(a), None);
        }
    }

    /// A shadow's derived status must be stable under a LATER hub propagation
    /// over the emitted file: the shadow carries its present bindings as
    /// dependencies, so re-running `enrich_verification_status` recomputes over
    /// the real closure instead of vacuously upgrading "verified" (which an
    /// empty dependency list would).
    #[test]
    fn shadow_verified_survives_second_propagation_pass() {
        let mut atoms = BTreeMap::new();
        // Foo.v is locally verified but contaminated (depends on unverified
        // Foo.bad), so it stays "verified" through propagation.
        let mut v = atom_with_status(Some("verified"));
        v.dependencies = ["probe:Foo.bad".to_string()].into();
        atoms.insert("probe:Foo.v".to_string(), v);
        atoms.insert(
            "probe:Foo.bad".to_string(),
            atom_with_status(Some("unverified")),
        );
        let mut model = BlueprintModel::default();
        model.nodes.push(node(
            "thm:loser",
            &["Foo.v"],
            StatementStatus::Formalized,
            ProofStatus::Proved,
        ));
        model.nodes.push(node(
            "thm:winner",
            &["Foo.v"],
            StatementStatus::Formalized,
            ProofStatus::Proved,
        ));
        enrich_propagate_derive(&mut atoms, &model);
        let shadow = &atoms["probe:blueprint:thm:loser"];
        assert_eq!(vs(shadow), Some("verified"));
        assert_eq!(
            shadow.dependencies,
            ["probe:Foo.v".to_string()].into(),
            "shadow carries its present bindings as dependencies"
        );
        // A downstream consumer runs `probe enrich` over the file again.
        probe::commands::propagate::enrich_verification_status(&mut atoms);
        assert_eq!(
            vs(&atoms["probe:blueprint:thm:loser"]),
            Some("verified"),
            "second propagation must not vacuously upgrade the shadow"
        );
    }

    /// A shadow with a genuinely-missing binding must not report a clean status
    /// from its present decls alone: the missing decl is an unverified
    /// component of the aggregate.
    #[test]
    fn shadow_with_missing_binding_is_unverified() {
        let mut atoms = BTreeMap::new();
        atoms.insert(
            "probe:Foo.a".to_string(),
            atom_with_status(Some("verified")), // upgraded to transitively-verified
        );
        let mut model = BlueprintModel::default();
        model.nodes.push(node(
            "thm:loser",
            &["Foo.a", "Foo.ghost"],
            StatementStatus::Formalized,
            ProofStatus::FullyProved,
        ));
        model.nodes.push(node(
            "thm:winner",
            &["Foo.a"],
            StatementStatus::Formalized,
            ProofStatus::FullyProved,
        ));
        enrich_propagate_derive(&mut atoms, &model);
        let shadow = &atoms["probe:blueprint:thm:loser"];
        assert_eq!(vs(shadow), Some("unverified"));
        assert_eq!(trusted_reason(shadow), None);
    }

    /// A mixed local/upstream shadow: the upstream part is renderer-proved in a
    /// dependency, so it contributes a `trusted` component — the aggregate is
    /// capped at "trusted" (with the upstream reason), not the local machine
    /// status.
    #[test]
    fn shadow_with_upstream_binding_is_trusted_upstream_proved() {
        let mut atoms = BTreeMap::new();
        atoms.insert(
            "probe:Foo.a".to_string(),
            atom_with_status(Some("verified")), // upgraded to transitively-verified
        );
        let mut model = BlueprintModel::default();
        let mut loser = node(
            "thm:loser",
            &["Foo.a", "Up.done"],
            StatementStatus::Formalized,
            ProofStatus::FullyProved,
        );
        loser.external_upstream_proved = vec!["Up.done".to_string()];
        model.nodes.push(loser);
        model.nodes.push(node(
            "thm:winner",
            &["Foo.a"],
            StatementStatus::Formalized,
            ProofStatus::FullyProved,
        ));
        enrich_propagate_derive(&mut atoms, &model);
        let shadow = &atoms["probe:blueprint:thm:loser"];
        assert_eq!(vs(shadow), Some("trusted"));
        assert_eq!(trusted_reason(shadow), Some("upstream-proved"));
    }

    /// `trusted` is not ranked between the machine rungs: an attestation
    /// anywhere in the binding caps the aggregate at "trusted", so a
    /// {verified, trusted} binding is trusted (not a machine-looking
    /// "verified" that hides the axiom).
    #[test]
    fn shadow_trust_is_sticky_over_verified() {
        let mut atoms = BTreeMap::new();
        let mut v = atom_with_status(Some("verified"));
        v.dependencies = ["probe:Foo.bad".to_string()].into(); // keeps it locally-verified
        atoms.insert("probe:Foo.v".to_string(), v);
        atoms.insert(
            "probe:Foo.bad".to_string(),
            atom_with_status(Some("verified")),
        );
        let mut ax = atom_with_status(Some("trusted"));
        ax.extensions
            .insert("trusted-reason".into(), Value::String("axiom".into()));
        atoms.insert("probe:Foo.ax".to_string(), ax);
        let mut model = BlueprintModel::default();
        model.nodes.push(node(
            "def:loser",
            &["Foo.v", "Foo.ax"],
            StatementStatus::Formalized,
            ProofStatus::FullyProved,
        ));
        model.nodes.push(node(
            "def:winner",
            &["Foo.v", "Foo.ax"],
            StatementStatus::Formalized,
            ProofStatus::FullyProved,
        ));
        enrich_propagate_derive(&mut atoms, &model);
        let shadow = &atoms["probe:blueprint:def:loser"];
        assert_eq!(vs(shadow), Some("trusted"));
        assert_eq!(trusted_reason(shadow), Some("axiom"));
    }

    /// Disagreeing trusted reasons across the binding are not resolved by
    /// picking one arbitrarily — the reason is omitted.
    #[test]
    fn shadow_with_ambiguous_trusted_reasons_omits_reason() {
        let mut atoms = BTreeMap::new();
        let mut ax = atom_with_status(Some("trusted"));
        ax.extensions
            .insert("trusted-reason".into(), Value::String("axiom".into()));
        atoms.insert("probe:Foo.ax".to_string(), ax);
        let mut ext = atom_with_status(Some("trusted"));
        ext.extensions
            .insert("trusted-reason".into(), Value::String("external".into()));
        atoms.insert("probe:Foo.ext".to_string(), ext);
        let mut model = BlueprintModel::default();
        model.nodes.push(node(
            "def:loser",
            &["Foo.ax", "Foo.ext"],
            StatementStatus::Formalized,
            ProofStatus::FullyProved,
        ));
        model.nodes.push(node(
            "def:winner",
            &["Foo.ax", "Foo.ext"],
            StatementStatus::Formalized,
            ProofStatus::FullyProved,
        ));
        enrich_propagate_derive(&mut atoms, &model);
        let shadow = &atoms["probe:blueprint:def:loser"];
        assert_eq!(vs(shadow), Some("trusted"));
        assert_eq!(trusted_reason(shadow), None);
    }

    #[test]
    fn derives_trusted_declared_for_massot_proved_claims() {
        for (proof, expected, reason) in [
            (ProofStatus::Ready, "unverified", None),
            (ProofStatus::Proved, "trusted", Some("declared")),
            (ProofStatus::FullyProved, "trusted", Some("declared")),
        ] {
            let mut atoms: BTreeMap<String, Atom> = BTreeMap::new();
            let mut model = BlueprintModel::default();
            let mut n = node("thm:m", &[], StatementStatus::Ready, proof);
            n.status_source = StatusSource::Declared;
            model.nodes.push(n);
            enrich_propagate_derive(&mut atoms, &model);
            let a = &atoms["probe:blueprint:thm:m"];
            assert_eq!(vs(a), Some(expected), "proof rung {proof:?}");
            assert_eq!(trusted_reason(a), reason);
        }
    }

    #[test]
    fn derives_unverified_for_genuine_gap_despite_proved_claim() {
        // Code-derived decl-missing with no upstream evidence: a fully-proved
        // claim must NOT mint a machine-looking status.
        let mut atoms: BTreeMap<String, Atom> = BTreeMap::new();
        let mut model = BlueprintModel::default();
        model.nodes.push(node(
            "thm:gap",
            &["Foo.ghost"],
            StatementStatus::Formalized,
            ProofStatus::FullyProved,
        ));
        enrich_propagate_derive(&mut atoms, &model);
        let a = &atoms["probe:blueprint:thm:gap"];
        assert_eq!(vs(a), Some("unverified"));
        assert_eq!(trusted_reason(a), None);
    }

    #[test]
    fn derives_trusted_upstream_proved_for_upstream_decl_missing() {
        let mut atoms: BTreeMap<String, Atom> = BTreeMap::new();
        let mut model = BlueprintModel::default();
        let mut n = node(
            "thm:up",
            &["Nat.mul_assoc"],
            StatementStatus::Formalized,
            ProofStatus::FullyProved,
        );
        n.external_upstream_proved = vec!["Nat.mul_assoc".to_string()];
        model.nodes.push(n);
        enrich_propagate_derive(&mut atoms, &model);
        let a = &atoms["probe:blueprint:thm:up"];
        assert_eq!(vs(a), Some("trusted"));
        assert_eq!(trusted_reason(a), Some("upstream-proved"));
    }

    #[test]
    fn derives_trusted_declared_for_massot_decl_missing_proved_claim() {
        // Massot \lean{Foo} + \leanok with Foo absent: the human claim may well
        // be "proven in another repo" — trusted, attributed to the declaration.
        let mut atoms: BTreeMap<String, Atom> = BTreeMap::new();
        let mut model = BlueprintModel::default();
        let mut n = node(
            "thm:elsewhere",
            &["Foo.ghost"],
            StatementStatus::Formalized,
            ProofStatus::FullyProved,
        );
        n.status_source = StatusSource::Declared;
        model.nodes.push(n);
        enrich_propagate_derive(&mut atoms, &model);
        let a = &atoms["probe:blueprint:thm:elsewhere"];
        assert_eq!(vs(a), Some("trusted"));
        assert_eq!(trusted_reason(a), Some("declared"));
    }

    #[test]
    fn shadow_inherits_final_post_propagation_status() {
        // Foo.a is "verified" with no dependencies, so the hub propagation
        // upgrades it to "transitively-verified"; the shadow must inherit the
        // FINAL value, proving the pass runs after propagation.
        let mut atoms = BTreeMap::new();
        atoms.insert(
            "probe:Foo.a".to_string(),
            atom_with_status(Some("verified")),
        );
        let mut model = BlueprintModel::default();
        model.nodes.push(node(
            "thm:loser",
            &["Foo.a"],
            StatementStatus::Formalized,
            ProofStatus::FullyProved,
        ));
        model.nodes.push(node(
            "thm:winner",
            &["Foo.a"],
            StatementStatus::Formalized,
            ProofStatus::FullyProved,
        ));
        let report = enrich_propagate_derive(&mut atoms, &model);
        assert_eq!(report.collision_shadowed, 1);
        let shadow = &atoms["probe:blueprint:thm:loser"];
        assert_eq!(
            shadow.extensions.get("blueprint-shadow").unwrap().as_bool(),
            Some(true)
        );
        assert_eq!(vs(shadow), Some("transitively-verified"));
        // The winner's real atom itself is of course untouched by the derive pass.
        assert_eq!(vs(&atoms["probe:Foo.a"]), Some("transitively-verified"));
    }

    #[test]
    fn shadow_inherits_weakest_status_across_bindings() {
        let mut atoms = BTreeMap::new();
        atoms.insert(
            "probe:Foo.a".to_string(),
            atom_with_status(Some("verified")), // upgraded to transitively-verified
        );
        atoms.insert(
            "probe:Foo.b".to_string(),
            atom_with_status(Some("unverified")),
        );
        let mut model = BlueprintModel::default();
        model.nodes.push(node(
            "thm:loser",
            &["Foo.a", "Foo.b"],
            StatementStatus::Formalized,
            ProofStatus::Proved,
        ));
        model.nodes.push(node(
            "thm:winner",
            &["Foo.a", "Foo.b"],
            StatementStatus::Formalized,
            ProofStatus::Proved,
        ));
        enrich_propagate_derive(&mut atoms, &model);
        assert_eq!(vs(&atoms["probe:blueprint:thm:loser"]), Some("unverified"));
    }

    #[test]
    fn shadow_falls_back_to_unverified_on_statusless_base() {
        // e.g. an atom base produced with --skip-verify.
        let mut atoms = BTreeMap::new();
        atoms.insert("probe:Foo.a".to_string(), atom_with_status(None));
        let mut model = BlueprintModel::default();
        model.nodes.push(node(
            "thm:loser",
            &["Foo.a"],
            StatementStatus::Formalized,
            ProofStatus::FullyProved,
        ));
        model.nodes.push(node(
            "thm:winner",
            &["Foo.a"],
            StatementStatus::Formalized,
            ProofStatus::FullyProved,
        ));
        enrich_propagate_derive(&mut atoms, &model);
        assert_eq!(vs(&atoms["probe:blueprint:thm:loser"]), Some("unverified"));
    }

    #[test]
    fn shadow_copies_trusted_reason_from_binding() {
        let mut atoms = BTreeMap::new();
        let mut axiom = atom_with_status(Some("trusted"));
        axiom
            .extensions
            .insert("trusted-reason".into(), Value::String("axiom".into()));
        atoms.insert("probe:Foo.ax".to_string(), axiom);
        let mut model = BlueprintModel::default();
        model.nodes.push(node(
            "def:loser",
            &["Foo.ax"],
            StatementStatus::Formalized,
            ProofStatus::FullyProved,
        ));
        model.nodes.push(node(
            "def:winner",
            &["Foo.ax"],
            StatementStatus::Formalized,
            ProofStatus::FullyProved,
        ));
        enrich_propagate_derive(&mut atoms, &model);
        let shadow = &atoms["probe:blueprint:def:loser"];
        assert_eq!(vs(shadow), Some("trusted"));
        assert_eq!(trusted_reason(shadow), Some("axiom"));
    }

    #[test]
    fn real_atoms_are_untouched_by_derivation() {
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
            ProofStatus::None,
        ));
        let before = serde_json::to_value(&atoms["probe:Foo.bar"].extensions).unwrap();
        enrich_propagate_derive(&mut atoms, &model);
        let after = &atoms["probe:Foo.bar"];
        // The bound atom gained blueprint-* fields but its machine status and
        // trusted-reason are exactly as before.
        assert_eq!(vs(after), Some("unverified"));
        assert!(!after.extensions.contains_key("trusted-reason"));
        assert_eq!(
            before.get("verification-status"),
            after.extensions.get("verification-status")
        );
    }

    #[test]
    fn derivation_is_idempotent_across_reruns() {
        let mut atoms = BTreeMap::new();
        atoms.insert(
            "probe:Foo.a".to_string(),
            atom_with_status(Some("verified")),
        );
        let mut model = BlueprintModel::default();
        model.nodes.push(node(
            "thm:bound",
            &["Foo.a"],
            StatementStatus::Formalized,
            ProofStatus::FullyProved,
        ));
        model.nodes.push(node(
            "thm:planned",
            &[],
            StatementStatus::Ready,
            ProofStatus::Ready,
        ));
        enrich_propagate_derive(&mut atoms, &model);
        let first = serde_json::to_value(&atoms).unwrap();
        // Re-run over the already-enriched, already-stamped map.
        enrich_propagate_derive(&mut atoms, &model);
        let second = serde_json::to_value(&atoms).unwrap();
        assert_eq!(first, second, "re-run must be a fixed point");
    }

    // --- node atoms (one per blueprint node, Verso-node parity) ---

    fn ext_str<'a>(atom: &'a Atom, key: &str) -> Option<&'a str> {
        atom.extensions.get(key).and_then(|v| v.as_str())
    }

    fn uses_of(atom: &Atom, key: &str) -> Vec<String> {
        atom.extensions
            .get(key)
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Every blueprint node — bound, planned-only, decl-missing — leaves exactly
    /// one node atom, with the right class discriminator.
    #[test]
    fn node_atom_location_from_chapter_and_group() {
        let mut model = BlueprintModel::default();
        let mut with_both = node(
            "thm:located",
            &[],
            StatementStatus::Ready,
            ProofStatus::Ready,
        );
        with_both.chapter = Some("Core Constructions 2.1".to_string());
        with_both.group = Some("cmz_amac".to_string());
        model.nodes.push(with_both);
        model.nodes.push(node(
            "thm:bare",
            &[],
            StatementStatus::Ready,
            ProofStatus::Ready,
        ));

        let mut atoms = BTreeMap::new();
        enrich(&mut atoms, &model);

        let located = &atoms["probe:blueprint:thm:located"];
        assert_eq!(located.code_path, "blueprint/Core-Constructions-2-1");
        assert_eq!(
            located.code_module,
            "Blueprint.Core-Constructions-2-1.cmz_amac"
        );

        let bare = &atoms["probe:blueprint:thm:bare"];
        assert_eq!(bare.code_path, "blueprint/ungrouped");
        assert_eq!(bare.code_module, "Blueprint.ungrouped");
    }

    #[test]
    fn location_component_sanitizes_to_one_segment() {
        assert_eq!(
            location_component("Hecke algebras").as_deref(),
            Some("Hecke-algebras")
        );
        assert_eq!(
            location_component(" a/b\\c.d  e ").as_deref(),
            Some("a-b-c-d-e")
        );
        assert_eq!(location_component("Core").as_deref(), Some("Core"));
        assert_eq!(location_component("µCMZ").as_deref(), Some("µCMZ"));
        assert_eq!(
            location_component("x?y#z%2Fw\u{0}\u{200b}v").as_deref(),
            Some("x-y-z-2Fw-v")
        );
        assert_eq!(location_component("  /. "), None);
        assert_eq!(location_component(""), None);
        let long = location_component(&"a".repeat(500)).unwrap();
        assert!(long.len() <= 68, "capped, got {}", long.len());
    }

    #[test]
    fn node_atom_location_degenerate_group_and_chapter() {
        let mut model = BlueprintModel::default();
        // A group literally named "ungrouped" is a real group and keeps its
        // module level; a group that sanitizes to nothing contributes none.
        let mut named_ungrouped =
            node("thm:named", &[], StatementStatus::Ready, ProofStatus::Ready);
        named_ungrouped.chapter = Some("Core".to_string());
        named_ungrouped.group = Some("ungrouped".to_string());
        model.nodes.push(named_ungrouped);
        let mut degenerate = node(
            "thm:degenerate",
            &[],
            StatementStatus::Ready,
            ProofStatus::Ready,
        );
        degenerate.chapter = Some(" /. ".to_string());
        degenerate.group = Some(" /. ".to_string());
        model.nodes.push(degenerate);

        let mut atoms = BTreeMap::new();
        enrich(&mut atoms, &model);

        let named = &atoms["probe:blueprint:thm:named"];
        assert_eq!(named.code_module, "Blueprint.Core.ungrouped");

        let degenerate = &atoms["probe:blueprint:thm:degenerate"];
        assert_eq!(degenerate.code_path, "blueprint/ungrouped");
        assert_eq!(degenerate.code_module, "Blueprint.ungrouped");
    }

    #[test]
    fn one_node_atom_per_node_with_class() {
        let mut atoms = BTreeMap::new();
        atoms.insert(
            "probe:Foo.a".to_string(),
            atom_with_status(Some("verified")),
        );
        let mut model = BlueprintModel::default();
        model.nodes.push(node(
            "thm:bound",
            &["Foo.a"],
            StatementStatus::Formalized,
            ProofStatus::FullyProved,
        ));
        model.nodes.push(node(
            "thm:planned",
            &[],
            StatementStatus::Ready,
            ProofStatus::Ready,
        ));
        model.nodes.push(node(
            "thm:ghost",
            &["Foo.ghost"],
            StatementStatus::Formalized,
            ProofStatus::Proved,
        ));

        let report = enrich(&mut atoms, &model);
        let node_atoms: Vec<(&String, &Atom)> = atoms
            .iter()
            .filter(|(_, a)| a.language == "blueprint")
            .collect();
        assert_eq!(
            node_atoms.len(),
            model.nodes.len(),
            "one node atom per node"
        );
        assert_eq!(report.node_atoms, model.nodes.len());
        assert_eq!(
            ext_str(&atoms["probe:blueprint:thm:bound"], "blueprint-node-class"),
            Some("bound")
        );
        assert_eq!(
            ext_str(
                &atoms["probe:blueprint:thm:planned"],
                "blueprint-node-class"
            ),
            Some("planned-only")
        );
        assert_eq!(
            ext_str(&atoms["probe:blueprint:thm:ghost"], "blueprint-node-class"),
            Some("decl-missing")
        );
        // The bound node atom carries its binding as dependencies; it is not a shadow.
        let bound = &atoms["probe:blueprint:thm:bound"];
        assert_eq!(bound.dependencies, ["probe:Foo.a".to_string()].into());
        assert!(!bound.extensions.contains_key("blueprint-shadow"));
    }

    /// A plain bound node atom (no collision) aggregates its decls' final
    /// machine statuses, exactly like shadows do.
    #[test]
    fn bound_node_atom_aggregates_binding_status() {
        // All decls clean -> transitively-verified (after propagation upgrade).
        let mut atoms = BTreeMap::new();
        atoms.insert(
            "probe:Foo.a".to_string(),
            atom_with_status(Some("verified")),
        );
        let mut model = BlueprintModel::default();
        model.nodes.push(node(
            "thm:clean",
            &["Foo.a"],
            StatementStatus::Formalized,
            ProofStatus::FullyProved,
        ));
        enrich_propagate_derive(&mut atoms, &model);
        assert_eq!(
            vs(&atoms["probe:blueprint:thm:clean"]),
            Some("transitively-verified")
        );

        // One contaminated decl caps the node atom at unverified.
        let mut atoms = BTreeMap::new();
        atoms.insert(
            "probe:Foo.a".to_string(),
            atom_with_status(Some("verified")),
        );
        atoms.insert(
            "probe:Foo.b".to_string(),
            atom_with_status(Some("unverified")),
        );
        let mut model = BlueprintModel::default();
        model.nodes.push(node(
            "thm:mixed",
            &["Foo.a", "Foo.b"],
            StatementStatus::Formalized,
            ProofStatus::Proved,
        ));
        enrich_propagate_derive(&mut atoms, &model);
        assert_eq!(vs(&atoms["probe:blueprint:thm:mixed"]), Some("unverified"));
    }

    /// Uses-edge resolution is class-dependent: node atoms resolve node-to-node
    /// (closed per-node graph); enriched lean atoms keep today's resolution to
    /// code representatives, byte-for-byte.
    #[test]
    fn uses_resolution_splits_by_atom_class() {
        let mut atoms = BTreeMap::new();
        atoms.insert(
            "probe:Foo.a".to_string(),
            atom_with_status(Some("verified")),
        );
        atoms.insert(
            "probe:Foo.b".to_string(),
            atom_with_status(Some("verified")),
        );
        let mut model = BlueprintModel::default();
        let mut used = node(
            "thm:used",
            &["Foo.a"],
            StatementStatus::Formalized,
            ProofStatus::FullyProved,
        );
        used.statement_uses = vec![];
        model.nodes.push(used);
        let mut user_bound = node(
            "thm:user_bound",
            &["Foo.b"],
            StatementStatus::Formalized,
            ProofStatus::FullyProved,
        );
        user_bound.statement_uses = vec!["thm:used".to_string()];
        model.nodes.push(user_bound);
        let mut user_planned = node(
            "thm:user_planned",
            &[],
            StatementStatus::Ready,
            ProofStatus::Ready,
        );
        user_planned.statement_uses = vec!["thm:used".to_string()];
        model.nodes.push(user_planned);

        enrich(&mut atoms, &model);
        // Lean atom: unchanged resolution — used label -> its real atom.
        assert_eq!(
            uses_of(&atoms["probe:Foo.b"], "blueprint-statement-uses"),
            vec!["probe:Foo.a".to_string()]
        );
        assert!(!atoms["probe:Foo.b"]
            .extensions
            .contains_key("blueprint-node-class"));
        // Node atoms (bound and planned alike): node-to-node resolution.
        assert_eq!(
            uses_of(
                &atoms["probe:blueprint:thm:user_bound"],
                "blueprint-statement-uses"
            ),
            vec!["probe:blueprint:thm:used".to_string()]
        );
        assert_eq!(
            uses_of(
                &atoms["probe:blueprint:thm:user_planned"],
                "blueprint-statement-uses"
            ),
            vec!["probe:blueprint:thm:used".to_string()]
        );
    }

    /// A collision produces node atoms for BOTH labels; only the loser is
    /// flagged as a shadow, and both aggregate over the same decl.
    #[test]
    fn collision_yields_node_atoms_for_winner_and_loser() {
        let mut atoms = BTreeMap::new();
        atoms.insert(
            "probe:Foo.a".to_string(),
            atom_with_status(Some("verified")),
        );
        let mut model = BlueprintModel::default();
        model.nodes.push(node(
            "thm:loser",
            &["Foo.a"],
            StatementStatus::Formalized,
            ProofStatus::FullyProved,
        ));
        model.nodes.push(node(
            "thm:winner",
            &["Foo.a"],
            StatementStatus::Formalized,
            ProofStatus::FullyProved,
        ));
        let report = enrich_propagate_derive(&mut atoms, &model);
        assert_eq!(report.collision_shadowed, 1);
        assert_eq!(report.node_atoms, 2);
        let loser = &atoms["probe:blueprint:thm:loser"];
        let winner = &atoms["probe:blueprint:thm:winner"];
        assert_eq!(
            loser
                .extensions
                .get("blueprint-shadow")
                .and_then(|v| v.as_bool()),
            Some(true)
        );
        assert!(!winner.extensions.contains_key("blueprint-shadow"));
        assert_eq!(ext_str(loser, "blueprint-node-class"), Some("bound"));
        assert_eq!(ext_str(winner, "blueprint-node-class"), Some("bound"));
        assert_eq!(vs(loser), Some("transitively-verified"));
        assert_eq!(vs(winner), Some("transitively-verified"));
    }

    /// The summary sidecar exposes the node-atom count as the checkable
    /// invariant against the Verso node count.
    #[test]
    fn summary_reports_node_atoms() {
        let mut atoms = BTreeMap::new();
        atoms.insert(
            "probe:Foo.a".to_string(),
            atom_with_status(Some("verified")),
        );
        let mut model = BlueprintModel::default();
        model.nodes.push(node(
            "thm:bound",
            &["Foo.a"],
            StatementStatus::Formalized,
            ProofStatus::FullyProved,
        ));
        model.nodes.push(node(
            "thm:planned",
            &[],
            StatementStatus::Ready,
            ProofStatus::Ready,
        ));
        let report = enrich(&mut atoms, &model);
        let summary = summarize(&model, &report);
        assert_eq!(summary.totals.node_atoms, 2);
        assert_eq!(summary.totals.node_atoms, summary.totals.nodes);
    }
}
