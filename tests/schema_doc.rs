//! Guard against `docs/SCHEMA.md` drifting from the committed example artifacts.
//!
//! `docs/SCHEMA.md` is the normative semantics reference and shows a concrete
//! `probe-leanblueprint/summary` example labeled with the real `SecureMessaging`
//! package. That example silently went stale once (it kept pre-fix 9/56 counts
//! after the real numbers became 8/53). This test re-pins the concrete parts of
//! the documented example to the committed
//! `examples/verso-secure-messaging/extract.summary.json`, so any future
//! regeneration of the example that forgets the doc fails CI.
//!
//! Only the fully-concrete sub-objects are compared (`totals`, `all`, `headline`,
//! and the one spelled-out `by-chapter` entry); the doc deliberately elides the
//! rest with `"...": ...` placeholders.

use serde_json::Value;

const AEAD_CHAPTER: &str = "Authenticated-Encryption-with-Associated-Data";

/// Extract the single fenced ```json block in `md` that contains `needle`.
fn json_block_containing(md: &str, needle: &str) -> String {
    let mut blocks = Vec::new();
    let mut in_block = false;
    let mut cur = String::new();
    for line in md.lines() {
        if in_block {
            if line.trim_start().starts_with("```") {
                blocks.push(std::mem::take(&mut cur));
                in_block = false;
            } else {
                cur.push_str(line);
                cur.push('\n');
            }
        } else if line.trim_start().starts_with("```json") {
            in_block = true;
        }
    }
    let matching: Vec<&String> = blocks.iter().filter(|b| b.contains(needle)).collect();
    assert_eq!(
        matching.len(),
        1,
        "expected exactly one ```json block containing {needle:?}, found {}",
        matching.len()
    );
    matching[0].clone()
}

#[test]
fn schema_summary_example_matches_committed_artifact() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let md = std::fs::read_to_string(root.join("docs/SCHEMA.md")).unwrap();
    let artifact =
        std::fs::read_to_string(root.join("examples/verso-secure-messaging/extract.summary.json"))
            .unwrap();

    let doc: Value = serde_json::from_str(&json_block_containing(
        &md,
        r#""schema": "probe-leanblueprint/summary""#,
    ))
    .expect("SCHEMA.md summary example must be valid JSON");
    let real: Value = serde_json::from_str(&artifact).unwrap();

    let doc_data = &doc["data"];
    let real_data = &real["data"];

    for field in ["totals", "all", "headline"] {
        assert_eq!(
            doc_data[field], real_data[field],
            "docs/SCHEMA.md summary example `data.{field}` is stale; regenerate it \
             from examples/verso-secure-messaging/extract.summary.json"
        );
    }
    assert_eq!(
        doc_data["by-chapter"][AEAD_CHAPTER], real_data["by-chapter"][AEAD_CHAPTER],
        "docs/SCHEMA.md summary example `by-chapter.{AEAD_CHAPTER}` is stale"
    );
}
