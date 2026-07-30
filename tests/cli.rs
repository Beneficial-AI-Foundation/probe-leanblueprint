//! CLI-level test: run the built `probe-leanblueprint extract` binary on the
//! Verso fixture and check the emitted envelopes.

use std::path::Path;
use std::process::Command;

fn fixture(rel: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(rel)
}

#[test]
fn cli_extract_verso_writes_envelopes() {
    let tmp = tempfile::tempdir().unwrap();
    let extract = tmp.path().join("extract.json");
    let summary = tmp.path().join("summary.json");

    let status = Command::new(env!("CARGO_BIN_EXE_probe-leanblueprint"))
        .args(["extract", ".", "--lean"])
        .arg(fixture("lean/erasure-codes-atoms.json"))
        .arg("--verso-manifest")
        .arg(fixture("verso/erasure-codes-manifest.json"))
        .arg("-o")
        .arg(&extract)
        .arg("--summary-output")
        .arg(&summary)
        .status()
        .unwrap();
    assert!(status.success(), "CLI exited non-zero");

    let extract_json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&extract).unwrap()).unwrap();
    assert_eq!(extract_json["schema"], "probe-leanblueprint/extract");
    assert_eq!(extract_json["schema-version"], "3.0");
    // Enriched bound atom carries blueprint metadata.
    assert_eq!(
        extract_json["data"]["probe:ErasureCode.Correct"]["blueprint-label"],
        "erasure_code_correctness"
    );
    // Synthetic planned atom exists.
    assert!(extract_json["data"]
        .get("probe:blueprint:reed_solomon_erasure_code")
        .is_some());

    let summary_json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&summary).unwrap()).unwrap();
    assert_eq!(summary_json["schema"], "probe-leanblueprint/summary");
    assert_eq!(summary_json["data"]["totals"]["nodes"], 4);
    assert_eq!(summary_json["data"]["totals"]["planned-only"], 2);

    // Node-complete extract: every summarized node has a record in the atoms.
    let node_count = summary_json["data"]["totals"]["nodes"].as_u64().unwrap();
    let labels: std::collections::BTreeSet<&str> = extract_json["data"]
        .as_object()
        .unwrap()
        .values()
        .filter_map(|a| a.get("blueprint-label").and_then(|v| v.as_str()))
        .collect();
    assert_eq!(
        labels.len() as u64,
        node_count,
        "every blueprint node must appear as a blueprint-label in the extract"
    );

    // Re-feeding the enriched extract as the atom base is rejected (self-ingestion).
    let reingest = Command::new(env!("CARGO_BIN_EXE_probe-leanblueprint"))
        .args(["extract", ".", "--lean"])
        .arg(&extract)
        .arg("--verso-manifest")
        .arg(fixture("verso/erasure-codes-manifest.json"))
        .arg("-o")
        .arg(tmp.path().join("reingest.json"))
        .arg("--summary-output")
        .arg(tmp.path().join("reingest_summary.json"))
        .output()
        .unwrap();
    assert!(
        !reingest.status.success(),
        "re-ingesting probe-leanblueprint's own extract must fail"
    );
    let stderr = String::from_utf8_lossy(&reingest.stderr);
    assert!(
        stderr.contains("probe-leanblueprint"),
        "error should explain the self-ingestion; got: {stderr}"
    );
}

/// probe-lean <= v0.9.6 emits interchange `schema-version` "2.0", which the
/// pinned hub loader (3.x only) rejects; v0.10.0+ emits "3.0" natively. The tool
/// must re-stamp a 2.x `probe-lean/extract` to 3.0 (into a temp copy) so it still
/// reads older releases and extracts already on disk, not only 3.0 producers. The
/// re-stamped run must match the equivalent 3.0 run node-for-node.
///
/// The fixture `erasure-codes-atoms-v2.json` is the 3.0 erasure-codes atom set
/// with its `schema-version` set back to "2.0" (real probe-lean atom *fields*, a
/// 2.0 version stamp) — it exercises the 2->3 re-stamp path; it is not a pristine
/// capture of a specific probe-lean 0.9.6 run.
#[test]
fn cli_extract_accepts_schema_2_probe_lean_input() {
    let tmp = tempfile::tempdir().unwrap();
    let extract = tmp.path().join("extract.json");
    let summary = tmp.path().join("summary.json");

    let out = Command::new(env!("CARGO_BIN_EXE_probe-leanblueprint"))
        .args(["extract", ".", "--lean"])
        .arg(fixture("lean/erasure-codes-atoms-v2.json"))
        .arg("--verso-manifest")
        .arg(fixture("verso/erasure-codes-manifest.json"))
        .arg("-o")
        .arg(&extract)
        .arg("--summary-output")
        .arg(&summary)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "schema-2.0 probe-lean input must be accepted and migrated; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    // The re-stamp is announced so a stale-input surprise is visible.
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("schema-version 2.0 to 3.0"),
        "migration should be logged; got: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // Output is a clean 3.0 envelope, identical in shape to the 3.0-input run.
    let extract_json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&extract).unwrap()).unwrap();
    assert_eq!(extract_json["schema-version"], "3.0");
    assert_eq!(
        extract_json["data"]["probe:ErasureCode.Correct"]["blueprint-label"],
        "erasure_code_correctness"
    );

    let summary_json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&summary).unwrap()).unwrap();
    assert_eq!(summary_json["data"]["totals"]["nodes"], 4);
}

/// Guard: a 2.x `probe-lean/extract` whose atoms actually carry the renamed
/// `is-disabled` field must be refused, not silently re-stamped to 3.0 (that
/// would mislabel a 2.0-field file). This directly protects the one known 2->3
/// atom-field incompatibility instead of trusting "probe-lean never emitted it".
#[test]
fn cli_refuses_2x_input_carrying_is_disabled() {
    let tmp = tempfile::tempdir().unwrap();
    let lean = tmp.path().join("legacy.json");
    std::fs::write(
        &lean,
        r#"{"schema":"probe-lean/extract","schema-version":"2.0",
            "tool":{"name":"probe-lean","version":"0.9.6","command":"extract"},
            "source":{"repo":"","commit":"","language":"lean","package":"P","package-version":""},
            "timestamp":"2026-01-01T00:00:00Z",
            "data":{"probe:Foo":{"display-name":"Foo","dependencies":[],"code-module":"M",
              "code-path":"M.lean","code-text":{"lines-start":1,"lines-end":2},"kind":"def",
              "language":"lean","is-disabled":false}}}"#,
    )
    .unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_probe-leanblueprint"))
        .args(["extract", ".", "--adapter", "verso", "--lean"])
        .arg(&lean)
        .arg("--verso-manifest")
        .arg(fixture("verso/erasure-codes-manifest.json"))
        .arg("-o")
        .arg(tmp.path().join("o.json"))
        .arg("--summary-output")
        .arg(tmp.path().join("s.json"))
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "must refuse an is-disabled 2.x input"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("is-disabled"),
        "error should name the is-disabled field; got: {stderr}"
    );
}

/// A 2.x input of a *non*-`probe-lean/extract` schema (e.g. a merged spine) is
/// deliberately not auto-migrated; the failure must carry actionable guidance,
/// not just the hub's bare "expected 3.x".
#[test]
fn cli_2x_non_probe_lean_input_gets_actionable_error() {
    let tmp = tempfile::tempdir().unwrap();
    let lean = tmp.path().join("spine.json");
    std::fs::write(
        &lean,
        r#"{"schema":"probe/merged-atoms","schema-version":"2.0",
            "tool":{"name":"probe","version":"0","command":"merge"},
            "inputs":[],"timestamp":"2026-01-01T00:00:00Z","data":{}}"#,
    )
    .unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_probe-leanblueprint"))
        .args(["extract", ".", "--adapter", "verso", "--lean"])
        .arg(&lean)
        .arg("--verso-manifest")
        .arg(fixture("verso/erasure-codes-manifest.json"))
        .arg("-o")
        .arg(tmp.path().join("o.json"))
        .arg("--summary-output")
        .arg(tmp.path().join("s.json"))
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "a 2.x merged spine must fail to load"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("auto-migrated") && stderr.contains("v0.10.0"),
        "error should guide the user (auto-migration scope + re-extract); got: {stderr}"
    );
}
