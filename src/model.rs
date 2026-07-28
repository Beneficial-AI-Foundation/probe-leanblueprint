//! Normalized, adapter-independent blueprint model.
//!
//! Both the Verso and Massot adapters produce a [`BlueprintModel`]; the
//! enrichment core only ever sees this normalized shape. The two-axis status
//! vocabulary is canonical here and every source status is mapped into it.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// The kind of a blueprint node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    Definition,
    Theorem,
}

impl NodeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            NodeKind::Definition => "definition",
            NodeKind::Theorem => "theorem",
        }
    }

    /// Best-effort classification from a source kind string.
    /// Blueprint "definition" is a definition; lemma/proposition/theorem/
    /// corollary are all theorem-like.
    ///
    /// A *null* source kind (a mention of a node defined elsewhere) is handled
    /// by the caller as "unknown" ([`BlueprintNode::kind`] `= None`), not here.
    /// An empty string still defaults to theorem, and a *non-empty, unrecognized*
    /// kind is warned about before defaulting to theorem, so schema drift is
    /// surfaced instead of silently bucketed (mirroring the fail-noisy stance the
    /// status axes take).
    pub fn from_source(s: &str) -> NodeKind {
        let kind = s.trim().to_ascii_lowercase();
        match kind.as_str() {
            "definition" | "def" | "dfn" => NodeKind::Definition,
            // Known theorem-like kinds across Verso and leanblueprint/plasTeX.
            "" | "theorem" | "thm" | "lemma" | "proposition" | "prop" | "corollary" | "cor"
            | "claim" | "conjecture" | "fact" | "property" => NodeKind::Theorem,
            other => {
                eprintln!("warning: unknown blueprint node kind {other:?}; treating as theorem");
                NodeKind::Theorem
            }
        }
    }
}

/// Statement axis: is the *statement* of this node formalized in Lean?
///
/// Ordered worst -> best. Mirrors the leanblueprint / Verso border colors.
// @kb: kb/tools/probe-leanblueprint.md#two-axis-status-vocabulary-canonical
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum StatementStatus {
    /// Informal only; no Lean formalization planned/started yet (Verso `none`;
    /// Massot: none of `\leanok`/`can_state`/`\notready` set).
    NonePlanned,
    /// Prerequisites not ready to state (Verso `blocked`, leanblueprint
    /// `\notready`).
    Blocked,
    /// Ready to be formalized; all prerequisites are done (`can_state`).
    Ready,
    /// The statement is formalized in Lean (`\leanok` / Verso `formalized`).
    Formalized,
}

impl StatementStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            StatementStatus::NonePlanned => "none",
            StatementStatus::Blocked => "blocked",
            StatementStatus::Ready => "ready",
            StatementStatus::Formalized => "formalized",
        }
    }
}

/// Proof axis: is the *proof* of this node complete (sorry-free)?
///
/// Ordered worst -> best.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ProofStatus {
    /// No proof, or proof not ready.
    None,
    /// Ready to be formalized; all prerequisites are done (`can_prove`).
    Ready,
    /// Proof is formalized locally (sorry-free) (`proved`).
    Proved,
    /// Proof and all its ancestors are formalized (`fully_proved`).
    FullyProved,
}

impl ProofStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            ProofStatus::None => "none",
            ProofStatus::Ready => "ready",
            ProofStatus::Proved => "proved",
            ProofStatus::FullyProved => "fully-proved",
        }
    }

    /// Does this status claim the proof is complete (proved or fully-proved)?
    pub fn claims_proved(self) -> bool {
        matches!(self, ProofStatus::Proved | ProofStatus::FullyProved)
    }
}

/// The origin of the status information, recorded on each atom so downstream
/// consumers know how much to trust the blueprint proof axis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusSource {
    /// Human-authored `\leanok` (Massot leanblueprint). Not machine-checked.
    Declared,
    /// Code-derived by the Verso blueprint renderer.
    CodeDerived,
}

impl StatusSource {
    pub fn as_str(self) -> &'static str {
        match self {
            StatusSource::Declared => "declared",
            StatusSource::CodeDerived => "code-derived",
        }
    }
}

/// A single blueprint node (a definition or theorem in the blueprint DAG).
#[derive(Debug, Clone)]
pub struct BlueprintNode {
    /// The blueprint label (e.g. `thm:sphere_eversion` or `aead_correctness`).
    pub label: String,
    /// The node kind, or `None` when the source gave no kind. In the per-chapter
    /// Verso layout a chapter that merely *mentions* a node defined elsewhere
    /// emits a copy with a `null` kind; keeping that distinct from a real kind
    /// lets the defining copy win the kind/title/chapter during merge instead of
    /// freezing an arbitrary guess (see [`merge_node`]).
    pub kind: Option<NodeKind>,
    /// Fully-qualified Lean declaration names this node binds (`\lean{...}` /
    /// Verso `codeData.external.decls[].canonical`). Empty for planned-only
    /// nodes.
    pub lean_decls: Vec<String>,
    pub statement_status: StatementStatus,
    pub proof_status: ProofStatus,
    /// Labels used by the statement of this node.
    pub statement_uses: Vec<String>,
    /// Labels used by the proof of this node.
    pub proof_uses: Vec<String>,
    /// Sub-construction grouping label (Verso `parent`), if any.
    pub group: Option<String>,
    /// Chapter the node belongs to (one Verso manifest = one chapter), if known.
    pub chapter: Option<String>,
    /// Display title (e.g. "Theorem 2.3"), if any.
    pub title: Option<String>,
    /// GitHub discussion issue number (`\discussion`), if any.
    pub discussion: Option<String>,
    pub status_source: StatusSource,
}

impl BlueprintNode {
    /// The kind to count/emit this node as. A node that was only ever *mentioned*
    /// (null kind everywhere, never defined) resolves to `Theorem`, preserving
    /// the historical default for genuinely-unknown nodes.
    pub fn display_kind(&self) -> NodeKind {
        self.kind.unwrap_or(NodeKind::Theorem)
    }
}

/// The full normalized blueprint model produced by an adapter.
#[derive(Debug, Clone, Default)]
pub struct BlueprintModel {
    pub nodes: Vec<BlueprintNode>,
}

impl BlueprintModel {
    /// Merge another model into this one, de-duplicating nodes by label.
    ///
    /// A blueprint label can legitimately recur across per-chapter Verso
    /// manifests (cross-references). The merge policy is deliberately narrow and
    /// count-preserving so aggregate totals do not depend on manifest order:
    ///
    /// - `statement_status` / `proof_status`: take the maximum (best-known)
    ///   status on each axis.
    /// - `lean_decls`, `statement_uses`, `proof_uses`: set-union (order-
    ///   preserving, de-duplicated). Merging decls is the load-bearing fix —
    ///   different manifests may each expose a subset of a node's bindings.
    /// - `kind`, `chapter`, `group`, `title`, `discussion`: DEFINING-COPY-WINS.
    ///   A copy carrying a known `kind` is the defining occurrence and its
    ///   identity beats a null-kind *mention* regardless of manifest order;
    ///   between two copies of equal standing it is first-wins, which is
    ///   deterministic because `load_from_dir` sorts manifest paths.
    pub fn merge_from(&mut self, other: BlueprintModel) {
        // Index existing nodes by label once so each incoming node is a single
        // lookup rather than a linear scan — keeps merges linear on large
        // multi-chapter manifests while preserving the first-wins merge policy.
        let mut index_by_label: HashMap<String, usize> = self
            .nodes
            .iter()
            .enumerate()
            .map(|(i, n)| (n.label.clone(), i))
            .collect();
        for node in other.nodes {
            if let Some(&idx) = index_by_label.get(&node.label) {
                merge_node(&mut self.nodes[idx], node);
            } else {
                index_by_label.insert(node.label.clone(), self.nodes.len());
                self.nodes.push(node);
            }
        }
    }
}

/// Merge `incoming` into `existing` (same label) using the model merge policy:
/// max status on each axis, set-union of decls/uses, first-wins for descriptive
/// and structural fields. Used for both cross-manifest and within-manifest
/// de-duplication.
pub fn merge_node(existing: &mut BlueprintNode, incoming: BlueprintNode) {
    union_extend(&mut existing.lean_decls, &incoming.lean_decls);
    union_extend(&mut existing.statement_uses, &incoming.statement_uses);
    union_extend(&mut existing.proof_uses, &incoming.proof_uses);
    if incoming.statement_status > existing.statement_status {
        existing.statement_status = incoming.statement_status;
    }
    if incoming.proof_status > existing.proof_status {
        existing.proof_status = incoming.proof_status;
    }
    // Identity (kind/title/chapter/group/discussion): a copy carrying a KNOWN
    // kind is the *defining* occurrence; a null-kind copy is a mere mention
    // (a chapter referencing a node defined elsewhere). Let the defining copy's
    // identity win over a mention's, regardless of manifest order — otherwise a
    // mention that sorts first would freeze the node as the wrong kind, in the
    // wrong chapter, with the label as its title.
    if existing.kind.is_none() && incoming.kind.is_some() {
        existing.kind = incoming.kind;
        // Defining copy wins, but fall back to the mention's value if the
        // defining copy happens to omit a field.
        if incoming.group.is_some() {
            existing.group = incoming.group;
        }
        if incoming.chapter.is_some() {
            existing.chapter = incoming.chapter;
        }
        if incoming.title.is_some() {
            existing.title = incoming.title;
        }
        if incoming.discussion.is_some() {
            existing.discussion = incoming.discussion;
        }
    } else {
        // First-wins for descriptive/structural fields (deterministic given
        // sorted manifest order). Also adopts a kind when neither side defines.
        if existing.kind.is_none() {
            existing.kind = incoming.kind;
        }
        if existing.group.is_none() {
            existing.group = incoming.group;
        }
        if existing.chapter.is_none() {
            existing.chapter = incoming.chapter;
        }
        if existing.title.is_none() {
            existing.title = incoming.title;
        }
        if existing.discussion.is_none() {
            existing.discussion = incoming.discussion;
        }
    }
}

/// Append items from `src` to `dst` that are not already present, preserving
/// `dst`'s order (a small, order-stable set union).
fn union_extend(dst: &mut Vec<String>, src: &[String]) {
    for item in src {
        if !dst.contains(item) {
            dst.push(item.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(label: &str, decls: &[&str], uses: &[&str]) -> BlueprintNode {
        BlueprintNode {
            label: label.into(),
            kind: Some(NodeKind::Theorem),
            lean_decls: decls.iter().map(|s| s.to_string()).collect(),
            statement_status: StatementStatus::NonePlanned,
            proof_status: ProofStatus::None,
            statement_uses: uses.iter().map(|s| s.to_string()).collect(),
            proof_uses: vec![],
            group: None,
            chapter: None,
            title: None,
            discussion: None,
            status_source: StatusSource::CodeDerived,
        }
    }

    #[test]
    fn merge_from_unions_decls_and_uses() {
        let mut a = BlueprintModel::default();
        a.nodes.push(node("thm:x", &["Foo.a"], &["l1"]));
        let mut b = BlueprintModel::default();
        b.nodes
            .push(node("thm:x", &["Foo.b", "Foo.a"], &["l2", "l1"]));

        a.merge_from(b);
        assert_eq!(a.nodes.len(), 1, "same label is de-duplicated");
        let n = &a.nodes[0];
        assert_eq!(n.lean_decls, vec!["Foo.a", "Foo.b"], "decls set-unioned");
        assert_eq!(n.statement_uses, vec!["l1", "l2"], "uses set-unioned");
    }

    #[test]
    fn merge_from_takes_status_max() {
        let mut a = BlueprintModel::default();
        let mut lo = node("thm:x", &[], &[]);
        lo.statement_status = StatementStatus::Blocked;
        lo.proof_status = ProofStatus::Ready;
        a.nodes.push(lo);
        let mut b = BlueprintModel::default();
        let mut hi = node("thm:x", &[], &[]);
        hi.statement_status = StatementStatus::Formalized;
        hi.proof_status = ProofStatus::None;
        b.nodes.push(hi);

        a.merge_from(b);
        assert_eq!(a.nodes[0].statement_status, StatementStatus::Formalized);
        assert_eq!(a.nodes[0].proof_status, ProofStatus::Ready, "max, not last");
    }

    /// A null-kind *mention* must never freeze a node's identity: the defining
    /// copy (with a known kind) wins its kind/chapter/title regardless of which
    /// side is merged first.
    #[test]
    fn defining_copy_wins_over_mention_either_order() {
        let mention = || {
            let mut n = node("prf_prng_scheme", &[], &[]);
            n.kind = None; // a chapter that merely references the node
            n.chapter = Some("Forward-Secure-AEAD".into());
            n.title = Some("prf_prng_scheme".into());
            n
        };
        let defining = || {
            let mut n = node("prf_prng_scheme", &[], &[]);
            n.kind = Some(NodeKind::Definition);
            n.chapter = Some("PRF-PRNG".into());
            n.title = Some("Definition 1.1".into());
            n.group = Some("prf_prng".into());
            n
        };

        for (first, second) in [(mention(), defining()), (defining(), mention())] {
            let mut a = BlueprintModel::default();
            a.nodes.push(first);
            let mut b = BlueprintModel::default();
            b.nodes.push(second);
            a.merge_from(b);
            let n = &a.nodes[0];
            assert_eq!(n.kind, Some(NodeKind::Definition), "defining kind wins");
            assert_eq!(
                n.chapter.as_deref(),
                Some("PRF-PRNG"),
                "defining chapter wins"
            );
            assert_eq!(
                n.title.as_deref(),
                Some("Definition 1.1"),
                "defining title wins"
            );
            assert_eq!(n.group.as_deref(), Some("prf_prng"), "defining group wins");
        }
    }

    /// A node that is only ever mentioned (null kind everywhere) resolves to the
    /// historical default (theorem) rather than being dropped.
    #[test]
    fn pure_mention_defaults_to_theorem() {
        let mut n = node("dangling", &[], &[]);
        n.kind = None;
        assert_eq!(n.display_kind(), NodeKind::Theorem);
    }
}

/// The blueprint extension fields attached to an atom, serialized (flattened)
/// into the atom JSON per KB P10. Field names are the canonical `blueprint-*`
/// keys.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BlueprintExtensions {
    #[serde(rename = "blueprint-label")]
    pub label: String,
    #[serde(rename = "blueprint-kind")]
    pub kind: String,
    #[serde(rename = "blueprint-statement-status")]
    pub statement_status: String,
    #[serde(rename = "blueprint-proof-status")]
    pub proof_status: String,
    #[serde(rename = "blueprint-status-source")]
    pub status_source: String,
    #[serde(rename = "blueprint-group", skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    #[serde(rename = "blueprint-chapter", skip_serializing_if = "Option::is_none")]
    pub chapter: Option<String>,
    #[serde(rename = "blueprint-title", skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(
        rename = "blueprint-discussion",
        skip_serializing_if = "Option::is_none"
    )]
    pub discussion: Option<String>,
    #[serde(
        rename = "blueprint-statement-uses",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub statement_uses: Vec<String>,
    #[serde(rename = "blueprint-proof-uses", skip_serializing_if = "Vec::is_empty")]
    pub proof_uses: Vec<String>,
    #[serde(
        rename = "blueprint-status-mismatch",
        skip_serializing_if = "Option::is_none"
    )]
    pub status_mismatch: Option<String>,
    #[serde(
        rename = "blueprint-decl-missing",
        skip_serializing_if = "std::ops::Not::not"
    )]
    pub decl_missing: bool,
    /// For a BOUND node, the subset of `lean_decls` that probe-lean did not emit
    /// (partial miss). Distinct from `decl_missing`, which flags the all-absent
    /// synthetic-node case. Empty when every bound decl is present.
    #[serde(
        rename = "blueprint-missing-decls",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub missing_decls: Vec<String>,
    /// `true` on a synthetic **shadow** atom emitted for a node that binds a
    /// present Lean decl already claimed by a later node (a same-decl
    /// collision). The shadow keeps the losing node's label in the extract so
    /// the atom set stays node-complete (every model node has a record), which
    /// is what makes `blueprint_stats.py` a faithful cross-check of the summary
    /// sidecar. A shadowed node is genuinely bound (it has a present decl), so
    /// consumers should count it as bound despite its `language: "blueprint"`.
    #[serde(
        rename = "blueprint-shadow",
        skip_serializing_if = "std::ops::Not::not"
    )]
    pub shadow: bool,
}
