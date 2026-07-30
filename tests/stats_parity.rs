//! Parity test: `scripts/blueprint_stats.py` (which reconstructs nodes from the
//! enriched atoms) must agree with the summary sidecar (computed from the model)
//! on the meaningful aggregate numbers, including the same-decl collision case.
//! Skips gracefully when `python3` is absent.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

use probe::types::{Atom, CodeText, Source};
use probe_leanblueprint::emit;
use probe_leanblueprint::enrich;
use probe_leanblueprint::model::{
    BlueprintModel, BlueprintNode, NodeKind, ProofStatus, StatementStatus, StatusSource,
};

fn atom(status: &str) -> Atom {
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
    a.extensions.insert(
        "verification-status".into(),
        serde_json::Value::String(status.into()),
    );
    a
}

fn node(label: &str, decls: &[&str], kind: NodeKind, proof: ProofStatus) -> BlueprintNode {
    BlueprintNode {
        label: label.into(),
        kind: Some(kind),
        lean_decls: decls.iter().map(|s| s.to_string()).collect(),
        external_upstream_proved: vec![],
        statement_status: StatementStatus::Formalized,
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
fn stats_py_agrees_with_summary_under_collision() {
    let python = "python3";
    if Command::new(python).arg("--version").output().is_err() {
        eprintln!("skipping stats parity test: python3 not available");
        return;
    }
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/blueprint_stats.py");

    let mut atoms = BTreeMap::new();
    atoms.insert("probe:Foo.bar".to_string(), atom("verified"));
    atoms.insert("probe:Foo.def".to_string(), atom("verified"));
    atoms.insert("probe:Foo.partial".to_string(), atom("verified"));
    atoms.insert("probe:Foo.mixedlocal".to_string(), atom("verified"));

    let mut model = BlueprintModel::default();
    model.nodes.push(node(
        "def:d",
        &["Foo.def"],
        NodeKind::Definition,
        ProofStatus::None,
    ));
    // thm:a and thm:b collide on Foo.bar; thm:b (later) wins the real atom,
    // thm:a becomes a shadow. Both must remain visible to stats.py.
    model.nodes.push(node(
        "thm:a",
        &["Foo.bar"],
        NodeKind::Theorem,
        ProofStatus::FullyProved,
    ));
    model.nodes.push(node(
        "thm:b",
        &["Foo.bar"],
        NodeKind::Theorem,
        ProofStatus::None,
    ));
    model.nodes.push(node(
        "thm:planned",
        &[],
        NodeKind::Theorem,
        ProofStatus::None,
    ));
    model.nodes.push(node(
        "thm:ghost",
        &["Foo.absent"],
        NodeKind::Theorem,
        ProofStatus::None,
    ));
    // Partial-missing fully-proved theorem: one decl present, one absent. Bound
    // but not fully backed, so NOT probe-lean-confirmed on either side — this is
    // the case that would drift if Python dropped the `missing_decls` check.
    model.nodes.push(node(
        "thm:partial",
        &["Foo.partial", "Foo.absent2"],
        NodeKind::Theorem,
        ProofStatus::FullyProved,
    ));
    // Mixed node: present local decl + absent upstream-proved decl. Confirmed on
    // both sides (fully backed), and the upstream decl must not appear as a
    // missing decl — the parity case for the bound-branch upstream exclusion.
    let mut mixed = node(
        "thm:mixed",
        &["Foo.mixedlocal", "Up.mul"],
        NodeKind::Theorem,
        ProofStatus::FullyProved,
    );
    mixed.external_upstream_proved = vec!["Up.mul".to_string()];
    model.nodes.push(mixed);

    let report = enrich::enrich(&mut atoms, &model);
    let summary = enrich::summarize(&model, &report);
    let source = Source {
        repo: String::new(),
        commit: String::new(),
        language: "lean".into(),
        package: "p".into(),
        package_version: String::new(),
        extensions: Default::default(),
    };
    let extract_env = emit::build_extract_envelope(atoms, source);

    let tmp = tempfile::tempdir().unwrap();
    let extract_path = tmp.path().join("extract.json");
    std::fs::write(
        &extract_path,
        serde_json::to_string_pretty(&extract_env).unwrap(),
    )
    .unwrap();

    let out = Command::new(python)
        .arg(&script)
        .arg(&extract_path)
        .arg("--json")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "blueprint_stats.py failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stats: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();

    // Node count: node-complete extract means stats sees every model node.
    assert_eq!(
        stats["totals"]["nodes"].as_u64().unwrap() as usize,
        summary.totals.nodes,
        "node count parity"
    );
    // Bound == with-lean-decl (shadow atoms count as bound).
    assert_eq!(
        stats["totals"]["bound"].as_u64().unwrap() as usize,
        summary.totals.with_lean_decl,
        "bound parity"
    );
    assert_eq!(
        stats["totals"]["planned-only"].as_u64().unwrap() as usize,
        summary.totals.planned_only,
        "planned-only parity"
    );
    assert_eq!(
        stats["totals"]["decl-missing"].as_u64().unwrap() as usize,
        summary.totals.decl_missing,
        "decl-missing parity"
    );
    assert_eq!(
        stats["totals"]["mismatches"].as_u64().unwrap() as usize,
        summary.totals.mismatches,
        "mismatches parity"
    );
    assert_eq!(
        stats["totals"]["partial-missing"].as_u64().unwrap() as usize,
        summary.totals.partial_missing,
        "partial-missing parity"
    );
    // Headline theorem numbers.
    assert_eq!(
        stats["headline"]["theorems-total"].as_u64().unwrap() as usize,
        summary.headline.theorems_total,
        "theorems-total parity"
    );
    assert_eq!(
        stats["headline"]["theorems-fully-proved"].as_u64().unwrap() as usize,
        summary.headline.theorems_fully_proved,
        "theorems-fully-proved parity"
    );
    assert_eq!(
        stats["headline"]["theorems-fully-proved-probe-lean-confirmed"]
            .as_u64()
            .unwrap() as usize,
        summary.headline.theorems_fully_proved_probe_lean_confirmed,
        "probe-lean-confirmed parity"
    );
    // Per-axis "all" formalized-statement count.
    assert_eq!(
        stats["statement"]["all"]["formalized"].as_u64().unwrap() as usize,
        summary.all.statement.formalized,
        "statement formalized parity"
    );
    assert_eq!(
        stats["proof"]["all"]["fully-proved"].as_u64().unwrap() as usize,
        summary.all.proof.fully_proved,
        "proof fully-proved parity"
    );

    // The machine-readable `upstream-decls-list` must reconstruct the mixed
    // node's absent-upstream binding from the wire (the only upstream decl in
    // this model). This is what makes blueprint_stats.py a faithful cross-check
    // of the new `blueprint-upstream-decls` field.
    assert_eq!(
        stats["upstream-decls-list"],
        serde_json::json!([["thm:mixed", ["Up.mul"]]]),
        "upstream-decls-list parity: the mixed node's upstream binding"
    );
}
