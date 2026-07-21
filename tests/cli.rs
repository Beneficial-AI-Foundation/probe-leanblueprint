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
    assert_eq!(extract_json["schema-version"], "2.0");
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
