//! Resolution + packaging of the bundled Massot emitter (`blueprint_emit.py`).
//!
//! The script is embedded into the binary via [`include_str!`] so a
//! `cargo install`ed executable is self-contained. At runtime the emitter is
//! resolved in priority order: an explicit `--emitter`, a copy shipped next to
//! the executable, else the embedded source materialized to a temp file.

use std::io::Write;
use std::path::{Path, PathBuf};

use crate::error::BlueprintError;

/// The bundled emitter source, embedded at compile time.
pub const EMITTER_SOURCE: &str = include_str!("../scripts/blueprint_emit.py");

/// A resolved emitter script path. When [`ResolvedEmitter::Materialized`], the
/// temp file is kept alive for as long as this value lives, so hold onto it
/// until the emitter has run.
pub enum ResolvedEmitter {
    /// A path on disk (explicit `--emitter` or shipped next to the binary).
    Path(PathBuf),
    /// The embedded source materialized to a temp file.
    Materialized(tempfile::NamedTempFile),
}

impl ResolvedEmitter {
    /// The filesystem path to the emitter script.
    pub fn path(&self) -> &Path {
        match self {
            ResolvedEmitter::Path(p) => p.as_path(),
            ResolvedEmitter::Materialized(f) => f.path(),
        }
    }
}

/// Resolve the emitter script.
///
/// Priority: explicit `--emitter`, then `scripts/blueprint_emit.py` next to the
/// running executable, else the embedded source materialized to a temp file.
pub fn resolve_emitter(explicit: Option<&Path>) -> Result<ResolvedEmitter, BlueprintError> {
    if let Some(p) = explicit {
        return Ok(ResolvedEmitter::Path(p.to_path_buf()));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let candidate = dir.join("scripts").join("blueprint_emit.py");
            if candidate.exists() {
                return Ok(ResolvedEmitter::Path(candidate));
            }
        }
    }
    Ok(ResolvedEmitter::Materialized(materialize_emitter()?))
}

/// Write the embedded emitter source to a fresh temp file.
pub fn materialize_emitter() -> Result<tempfile::NamedTempFile, BlueprintError> {
    let map_err = |e: std::io::Error| BlueprintError::EmitterUnresolved(e.to_string());
    let mut file = tempfile::Builder::new()
        .prefix("blueprint_emit-")
        .suffix(".py")
        .tempfile()
        .map_err(map_err)?;
    file.write_all(EMITTER_SOURCE.as_bytes()).map_err(map_err)?;
    file.flush().map_err(map_err)?;
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_source_is_the_python_emitter() {
        assert!(EMITTER_SOURCE.contains("blueprint"));
        assert!(!EMITTER_SOURCE.trim().is_empty());
    }

    #[test]
    fn materializes_to_a_readable_path() {
        let emitter = materialize_emitter().unwrap();
        let path = emitter.path().to_path_buf();
        assert!(path.exists(), "materialized emitter should exist on disk");
        let contents = std::fs::read_to_string(&path).unwrap();
        assert_eq!(contents, EMITTER_SOURCE);
    }

    #[test]
    fn resolve_prefers_explicit_path() {
        let explicit = Path::new("/some/where/blueprint_emit.py");
        let resolved = resolve_emitter(Some(explicit)).unwrap();
        assert_eq!(resolved.path(), explicit);
    }
}
