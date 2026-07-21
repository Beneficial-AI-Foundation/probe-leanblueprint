//! Typed errors for the probe-leanblueprint library.
//!
//! The library layers (adapters + IO) return [`BlueprintError`] so callers can
//! match on the failure mode; `main.rs` keeps `anyhow` at the CLI boundary and
//! lifts these via `?` (they implement [`std::error::Error`]). The enrichment
//! core stays infallible.

use std::path::PathBuf;

use thiserror::Error;

/// A recoverable failure in a probe-leanblueprint library operation.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum BlueprintError {
    /// A filesystem read failed.
    #[error("failed to read {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// No `blueprint-manifest.json` was found under a search root.
    #[error(
        "no blueprint-manifest.json found under {0}; render the Verso docs first or pass --verso-manifest"
    )]
    NoManifest(PathBuf),

    /// A Verso `blueprint-manifest.json` failed to parse.
    #[error("failed to parse Verso blueprint-manifest.json: {0}")]
    ManifestParse(#[source] serde_json::Error),

    /// A Verso node carried a status string outside the known vocabulary
    /// (schema drift): failing loudly beats silently under-counting progress.
    #[error("unknown Verso {axis} status {value:?} (schema drift; expected one of {expected})")]
    UnknownStatus {
        axis: &'static str,
        value: String,
        expected: &'static str,
    },

    /// The Massot blueprint `web.tex` entry point was not found.
    #[error("blueprint web.tex not found at {0}; pass --blueprint-src")]
    WebTexNotFound(PathBuf),

    /// The bundled emitter script could not be located or materialized.
    #[error("could not locate blueprint_emit.py; pass --emitter <path>: {0}")]
    EmitterUnresolved(String),

    /// The Massot emitter process could not be spawned.
    #[error("failed to run {python} {script}: {source}")]
    EmitterSpawn {
        python: String,
        script: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// The Massot emitter ran but exited non-zero.
    #[error("blueprint_emit.py failed ({status}):\n{stderr}")]
    EmitterFailed { status: String, stderr: String },

    /// The Massot emitter produced non-UTF-8 output.
    #[error("emitter output was not valid UTF-8: {0}")]
    EmitterNonUtf8(#[source] std::string::FromUtf8Error),

    /// The Massot emitter output failed to parse.
    #[error("failed to parse blueprint_emit.py output: {0}")]
    EmitterParse(#[source] serde_json::Error),

    /// No blueprint adapter could be auto-detected.
    #[error("could not detect blueprint adapter; pass --adapter verso|massot")]
    AdapterUndetected,

    /// The `--lean` atom base is this tool's own output (self-ingestion), which
    /// would double-count blueprint nodes. Pass a `probe-lean/extract` (or a
    /// merged spine) instead.
    #[error(
        "--lean input {path} has schema {schema:?}; expected a probe-lean atom base, \
         not probe-leanblueprint's own output"
    )]
    NotLeanBase { path: PathBuf, schema: String },
}
