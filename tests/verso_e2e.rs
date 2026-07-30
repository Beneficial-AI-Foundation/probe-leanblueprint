//! End-to-end test of the Verso path against a real `blueprint-manifest.json`
//! from baif/secure-messaging (Erasure-Codes chapter), joined onto a minimal
//! probe-lean atom base.

use std::collections::BTreeMap;
use std::path::Path;

use probe::types::{load_atom_file, Atom, CodeText};
use probe_leanblueprint::adapters::verso;
use probe_leanblueprint::{emit, enrich};

fn fixture(rel: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(rel)
}

/// Read a `blueprint-*` string-array extension, asserting each element is a
/// string — a malformed wire value must fail the invariant, not be silently
/// dropped (this is a wire-contract check).
fn str_list(atom: &Atom, key: &str) -> Vec<String> {
    match atom.extensions.get(key) {
        None => Vec::new(),
        Some(v) => v
            .as_array()
            .unwrap_or_else(|| panic!("{key} must be a JSON array"))
            .iter()
            .map(|e| {
                e.as_str()
                    .unwrap_or_else(|| panic!("{key} element must be a string"))
                    .to_string()
            })
            .collect(),
    }
}

/// Assert the upstream/missing wire invariants over every atom in an enriched
/// extract: (a) `blueprint-upstream-decls` and `blueprint-missing-decls` are
/// disjoint; (b) a listed upstream decl is never present locally; (c) the
/// `blueprint-decl-upstream-proved` bool implies a non-empty upstream list.
fn assert_wire_invariants(atoms: &BTreeMap<String, Atom>) {
    for (key, atom) in atoms {
        let upstream = str_list(atom, "blueprint-upstream-decls");
        let missing = str_list(atom, "blueprint-missing-decls");
        for d in &upstream {
            assert!(
                !missing.contains(d),
                "{key}: {d} is in both upstream-decls and missing-decls"
            );
            assert!(
                !atoms.contains_key(&format!("probe:{d}")),
                "{key}: upstream decl {d} is present locally, so must not be listed as upstream"
            );
        }
        let bool_set = atom
            .extensions
            .get("blueprint-decl-upstream-proved")
            .and_then(|v| v.as_bool())
            == Some(true);
        assert!(
            !bool_set || !upstream.is_empty(),
            "{key}: decl-upstream-proved is set but upstream-decls is empty"
        );
    }
}

/// A minimal `lean`-language atom carrying a `verification-status`.
fn lean_atom(status: &str) -> Atom {
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
    a.extensions
        .insert("verification-status".into(), status.to_string().into());
    a
}

#[test]
fn verso_erasure_codes_end_to_end() {
    let model = verso::load_manifest(&fixture("verso/erasure-codes-manifest.json")).unwrap();
    assert_eq!(model.nodes.len(), 4, "erasure-codes chapter has 4 nodes");

    let (mut atoms, _prov) = load_atom_file(&fixture("lean/erasure-codes-atoms.json")).unwrap();
    let report = enrich::enrich(&mut atoms, &model);

    assert_eq!(report.nodes_total, 4);
    assert_eq!(report.nodes_with_decl, 2);
    assert_eq!(report.planned_only, 2);
    assert_eq!(report.decl_missing, 0);
    assert_eq!(report.mismatches.len(), 0);

    // The correctness theorem got blueprint metadata but keeps machine status.
    let correct = &atoms["probe:ErasureCode.Correct"];
    assert_eq!(
        correct.extensions.get("blueprint-label").unwrap().as_str(),
        Some("erasure_code_correctness")
    );
    assert_eq!(
        correct
            .extensions
            .get("blueprint-proof-status")
            .unwrap()
            .as_str(),
        Some("fully-proved")
    );
    assert_eq!(
        correct
            .extensions
            .get("blueprint-status-source")
            .unwrap()
            .as_str(),
        Some("code-derived")
    );

    // A planned-only node (no Lean decl) becomes a synthetic non-stub atom.
    let planned = &atoms["probe:blueprint:reed_solomon_erasure_code_correctness"];
    assert_eq!(planned.language, "blueprint");
    assert!(!planned.is_stub());
    assert!(!planned.extensions.contains_key("verification-status"));

    let summary = enrich::summarize(&model, &report);
    assert_eq!(summary.all.statement.formalized, 2);
    assert_eq!(summary.totals.planned_only, 2);
}

/// Full-project e2e against the committed real run: all 9 `secure-messaging`
/// chapter manifests (freshly rendered at commit 6a4fce0 and trimmed to
/// adapter-relevant fields) joined onto the real probe-lean atom base (the
/// blueprint-bound decls with their real `verification-status`). Manifests and
/// atoms are from the *same commit*, so the join is fully consistent
/// (0 decl-missing, 0 mismatch), and these numbers match the deployed
/// per-chapter Blueprint-Summary pages (e.g. AEAD: 14 nodes, 5 theorems).
#[test]
fn verso_secure_messaging_full_project() {
    let model = verso::load_from_dir(&fixture("verso/secure-messaging")).unwrap();
    // 111 unique blueprint nodes after cross-chapter label de-duplication.
    assert_eq!(
        model.nodes.len(),
        111,
        "secure-messaging has 111 blueprint nodes"
    );

    let (mut atoms, prov) = load_atom_file(&fixture("lean/secure-messaging-atoms.json")).unwrap();

    // Schema 3.0 `source` passthrough: probe-lean marks this a security-protocol
    // via `source.class`, an unmodeled field the hub `Source` captures via
    // flatten. It must survive load -> emit instead of being dropped.
    let source = prov[0].source.clone();
    assert_eq!(
        source.extensions.get("class").and_then(|v| v.as_str()),
        Some("security-protocol"),
        "source.class is captured on load"
    );
    let extract_env = emit::build_extract_envelope(atoms.clone(), source);
    let env_json = serde_json::to_value(&extract_env).unwrap();
    assert_eq!(
        env_json["source"]["class"], "security-protocol",
        "source.class round-trips into the emitted extract"
    );

    let report = enrich::enrich(&mut atoms, &model);

    assert_eq!(report.nodes_total, 111);
    assert_eq!(report.nodes_with_decl, 33, "33 nodes bind a Lean decl");
    assert_eq!(report.planned_only, 78, "78 nodes are roadmap-only");
    assert_eq!(
        report.decl_missing, 0,
        "same-commit render: every bound decl exists in probe-lean output"
    );
    assert_eq!(
        report.mismatches.len(),
        0,
        "no blueprint proof claim contradicts the machine status (no sorry on a tracked decl)"
    );

    let summary = enrich::summarize(&model, &report);
    // 58 definitions + 53 theorems = 111 nodes. Three definitions
    // (prf_prng_scheme, prf_prng_security, scka_scheme) are also *mentioned* by
    // earlier-sorting chapters with a null kind; the defining copy wins the kind
    // during merge, so they count as definitions (scka_scheme is fully proved,
    // hence 8 proved theorems, not 9).
    assert_eq!(summary.headline.theorems_total, 53);
    assert_eq!(summary.headline.theorems_fully_proved, 8);
    // Verso status is code-derived, so every claim is machine-backed: confirmed
    // equals claimed (no `blueprint-status-mismatch`).
    assert_eq!(
        summary.headline.theorems_fully_proved_probe_lean_confirmed, 8,
        "code-derived: probe-lean-confirmed equals claimed"
    );

    // Per-chapter breakdown is populated and consistent with the whole.
    let chapters = &summary.by_chapter;
    assert_eq!(chapters.len(), 9, "one bucket per rendered chapter");
    let chapter_nodes: usize = chapters.values().map(|c| c.nodes).sum();
    assert_eq!(
        chapter_nodes, 111,
        "chapter node counts partition all nodes"
    );
    // AEAD matches the deployed site's AEAD Blueprint-Summary page: 14 entries,
    // 5 theorems of which 3 are fully proved.
    let aead = chapters
        .get("Authenticated-Encryption-with-Associated-Data")
        .expect("AEAD chapter present");
    assert_eq!(aead.nodes, 14);
    assert_eq!(aead.theorems_total, 5);
    assert_eq!(aead.theorems_fully_proved, 3);

    // A real bound theorem keeps its machine status while gaining blueprint fields.
    let correct = &atoms["probe:ErasureCode.Correct"];
    assert_eq!(
        correct
            .extensions
            .get("blueprint-chapter")
            .unwrap()
            .as_str(),
        Some("Erasure-Codes")
    );
    assert_eq!(
        correct
            .extensions
            .get("verification-status")
            .unwrap()
            .as_str(),
        Some("transitively-verified")
    );

    // Wire invariants for the upstream/missing partition. secure-messaging has no
    // out-of-workspace decls, so this only exercises the trivial (empty) case
    // here; `verso_mixed_upstream_wire_evidence` fires it on real upstream data.
    assert_wire_invariants(&atoms);
}

/// A node binding one in-workspace decl (present in the atom base) plus one
/// out-of-workspace-proved decl (absent) — the *mixed* case. Exercises the full
/// verso-adapter -> enrich pipeline on a NON-empty `blueprint-upstream-decls`
/// (the secure-messaging fixture has none), so the wire invariants and the
/// absence-based semantics are actually checked end-to-end.
#[test]
fn verso_mixed_upstream_wire_evidence() {
    let model = verso::load_manifest(&fixture("verso/upstream-mixed-manifest.json")).unwrap();

    let mut atoms: BTreeMap<String, Atom> = BTreeMap::new();
    // The in-workspace decl is present locally; the upstream one is absent.
    atoms.insert("probe:MyProj.local".to_string(), lean_atom("verified"));

    let report = enrich::enrich(&mut atoms, &model);
    assert_eq!(
        report.nodes_with_decl, 1,
        "bound via the present local decl"
    );
    assert_eq!(
        report.partial_missing, 0,
        "the absent decl is upstream-proved, not a gap"
    );
    assert_eq!(
        report.probe_lean_confirmed_proved,
        vec!["thm:mixed"],
        "a mixed node whose whole binding is present-or-upstream is confirmed"
    );

    let bound = &atoms["probe:MyProj.local"];
    assert_eq!(
        str_list(bound, "blueprint-upstream-decls"),
        vec!["Nat.upstream"],
        "the absent upstream decl is surfaced on the wire"
    );
    assert!(
        !bound.extensions.contains_key("blueprint-missing-decls"),
        "the upstream decl is not a partial-missing gap"
    );

    // The invariants now fire against a non-empty upstream list.
    assert_wire_invariants(&atoms);
}
