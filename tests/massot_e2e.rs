//! End-to-end test of the Massot path. Uses a committed emitter-output fixture
//! (captured from the real plasTeX emitter) so CI needs no Python, plus an
//! `#[ignore]`d test that runs the live emitter when a suitable Python is set
//! via `PROBE_LEANBLUEPRINT_PYTHON`.

use std::path::Path;

use probe::types::load_atom_file;
use probe_leanblueprint::adapters::massot;
use probe_leanblueprint::enrich;

fn fixture(rel: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(rel)
}

#[test]
fn massot_enrichment_from_emitter_fixture() {
    let text = std::fs::read_to_string(fixture("massot/emitter-output.json")).unwrap();
    let model = massot::parse_emitter_json(&text).unwrap();
    assert_eq!(model.nodes.len(), 4);

    let (mut atoms, _prov) = load_atom_file(&fixture("lean/massot-atoms.json")).unwrap();
    let report = enrich::enrich(&mut atoms, &model);

    assert_eq!(report.nodes_with_decl, 2, "foo + bar are bound");
    assert_eq!(report.planned_only, 1, "qux has no lean decl");
    assert_eq!(report.decl_missing, 1, "baz binds an absent decl");
    assert_eq!(report.mismatches.len(), 1, "bar over-claims proof");

    // bar declares its proof done but probe-lean says unverified.
    let bar = &atoms["probe:Foo.bar"];
    assert_eq!(
        bar.extensions
            .get("blueprint-status-mismatch")
            .unwrap()
            .as_str(),
        Some("claims-proved-but-unverified")
    );
    assert_eq!(
        bar.extensions.get("blueprint-discussion").unwrap().as_str(),
        Some("42")
    );
    // Status source records that this is a human claim, not code-derived.
    assert_eq!(
        bar.extensions
            .get("blueprint-status-source")
            .unwrap()
            .as_str(),
        Some("declared")
    );

    let baz = &atoms["probe:blueprint:def:baz"];
    assert_eq!(
        baz.extensions
            .get("blueprint-decl-missing")
            .unwrap()
            .as_bool(),
        Some(true)
    );

    // `thm:qux` uses the decl-missing `def:baz`; the resolved code-name must be
    // the synthetic key that actually exists in the atom map, not the absent
    // `probe:Foo.baz`. Locks in the dangling-uses fix.
    let qux = &atoms["probe:blueprint:thm:qux"];
    let uses: Vec<&str> = qux
        .extensions
        .get("blueprint-statement-uses")
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(
        uses.contains(&"probe:blueprint:def:baz"),
        "uses should resolve to the synthetic key, got {uses:?}"
    );
    for cn in &uses {
        assert!(
            atoms.contains_key(*cn),
            "every resolved uses target must be a real atom key: {cn}"
        );
    }

    // Headline must not count the contradicted claim: `thm:bar` is fully-proved
    // in the blueprint but unverified by the machine, so it is claimed-but-not-
    // confirmed. The honest headline is 0/2, not 1/2 (P26).
    let summary = enrich::summarize(&model, &report);
    assert_eq!(summary.headline.theorems_total, 2);
    assert_eq!(
        summary.headline.theorems_fully_proved, 1,
        "blueprint claims bar fully proved"
    );
    assert_eq!(
        summary.headline.theorems_fully_proved_machine_confirmed, 0,
        "machine contradicts bar, so it is not confirmed"
    );
}

/// Live emitter run. Requires a Python with plasTeX + leanblueprint installed.
/// Run with `PROBE_LEANBLUEPRINT_PYTHON=/path/to/venv/bin/python cargo test -- --ignored`.
#[test]
#[ignore]
fn massot_live_emitter() {
    let python = std::env::var("PROBE_LEANBLUEPRINT_PYTHON").unwrap_or_else(|_| "python3".into());
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/blueprint_emit.py");
    let web_tex = fixture("massot/web.tex");
    let model = massot::run(&python, &script, &web_tex).unwrap();
    assert_eq!(model.nodes.len(), 4);
}
