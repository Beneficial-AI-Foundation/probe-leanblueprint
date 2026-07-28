//! End-to-end test of the Verso path against a real `blueprint-manifest.json`
//! from baif/secure-messaging (Erasure-Codes chapter), joined onto a minimal
//! probe-lean atom base.

use std::path::Path;

use probe::types::load_atom_file;
use probe_leanblueprint::adapters::verso;
use probe_leanblueprint::enrich;

fn fixture(rel: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(rel)
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

    let (mut atoms, _prov) = load_atom_file(&fixture("lean/secure-messaging-atoms.json")).unwrap();
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
    // 58 definitions + 53 theorems = 111 nodes. Earlier this pinned 56/9: three
    // definitions (prf_prng_scheme, prf_prng_security, scka_scheme) are *mentioned*
    // by chapters that sort before their defining chapter, and those mention copies
    // carry a null kind. The defining copy now wins the kind during merge, so they
    // are no longer miscounted as theorems (scka_scheme is fully proved, which is
    // why the proved count drops from 9 to 8).
    assert_eq!(summary.headline.theorems_total, 53);
    assert_eq!(summary.headline.theorems_fully_proved, 8);
    // Verso status is code-derived, so every claim is machine-backed: confirmed
    // equals claimed (no `blueprint-status-mismatch`).
    assert_eq!(
        summary.headline.theorems_fully_proved_machine_confirmed, 8,
        "code-derived: machine-confirmed equals claimed"
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
}
