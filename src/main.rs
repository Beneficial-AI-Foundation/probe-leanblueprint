use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use probe::types::{load_atom_file, load_envelope, Atom, InputProvenance, Source};

use probe_leanblueprint::adapters::{massot, verso};
use probe_leanblueprint::emit;
use probe_leanblueprint::emitter;
use probe_leanblueprint::enrich;
use probe_leanblueprint::error::BlueprintError;
use probe_leanblueprint::model::BlueprintModel;

#[derive(Parser)]
#[command(
    name = "probe-leanblueprint",
    version,
    about = "Enrich probe-lean atoms with Lean blueprint progress metadata"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Enrich probe-lean atoms with blueprint metadata and emit a
    /// `probe-leanblueprint/extract` envelope + summary sidecar.
    Extract(ExtractArgs),
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
enum Adapter {
    Auto,
    Verso,
    Massot,
}

/// A concretely-selected adapter (never `Auto`). Resolving `Adapter::Auto` into
/// this before dispatch removes the need for an `unreachable!` arm.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ResolvedAdapter {
    Verso,
    Massot,
}

#[derive(Parser)]
struct ExtractArgs {
    /// Path to the Lean project (used for auto-detection and orchestration).
    #[arg(default_value = ".")]
    project: PathBuf,

    /// Existing `probe-lean/extract` JSON to use as the atom base. If omitted,
    /// `probe-lean extract` is run against the project.
    #[arg(long)]
    lean: Option<PathBuf>,

    /// Which blueprint adapter to use.
    #[arg(long, value_enum, default_value_t = Adapter::Auto)]
    adapter: Adapter,

    /// Verso blueprint manifest file, or a directory to search recursively for
    /// `blueprint-manifest.json`.
    #[arg(long)]
    verso_manifest: Option<PathBuf>,

    /// Massot blueprint source: a `web.tex` file or the directory containing it
    /// (defaults to `<project>/blueprint/src/web.tex`).
    #[arg(long)]
    blueprint_src: Option<PathBuf>,

    /// Python interpreter used to run the Massot emitter.
    #[arg(long, default_value = "python3")]
    python: String,

    /// Path to the bundled `blueprint_emit.py` emitter.
    #[arg(long)]
    emitter: Option<PathBuf>,

    /// Output path for the enriched extract envelope.
    #[arg(long, short = 'o')]
    output: Option<PathBuf>,

    /// Output path for the summary sidecar.
    #[arg(long)]
    summary_output: Option<PathBuf>,

    /// Override the source package name (identity of the atom base). Use with
    /// `--source-version` to disambiguate a spine with multiple probe-lean
    /// inputs.
    #[arg(long)]
    source_package: Option<String>,

    /// Override the source package version. See `--source-package`.
    #[arg(long)]
    source_version: Option<String>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Extract(args) => run_extract(args),
    }
}

fn detect_adapter(args: &ExtractArgs) -> Result<ResolvedAdapter> {
    match args.adapter {
        Adapter::Verso => return Ok(ResolvedAdapter::Verso),
        Adapter::Massot => return Ok(ResolvedAdapter::Massot),
        Adapter::Auto => {}
    }
    if args.verso_manifest.is_some() {
        return Ok(ResolvedAdapter::Verso);
    }
    if args.blueprint_src.is_some() {
        return Ok(ResolvedAdapter::Massot);
    }
    // Auto-detect from the project layout. Check the Verso signal (a
    // `versoBlueprint` lakefile declaration) first: a Verso project may carry a
    // leftover/migrated Massot `blueprint/` tree, and defaulting to Massot in
    // that case would silently pick the wrong ecosystem.
    let has_web_tex = args.project.join("blueprint/src/web.tex").exists();
    let has_verso = ["lakefile.toml", "lakefile.lean"].iter().any(|lf| {
        std::fs::read_to_string(args.project.join(lf))
            .map(|text| text.contains("versoBlueprint") || text.contains("verso-blueprint"))
            .unwrap_or(false)
    });
    match (has_verso, has_web_tex) {
        (true, true) => {
            eprintln!(
                "warning: found both a versoBlueprint lakefile signal and \
                 blueprint/src/web.tex; using Verso (pass --adapter to override)"
            );
            Ok(ResolvedAdapter::Verso)
        }
        (true, false) => Ok(ResolvedAdapter::Verso),
        (false, true) => Ok(ResolvedAdapter::Massot),
        // No positive signal: fail loudly instead of defaulting to Verso and
        // then dying later with a confusing "no manifest" error.
        (false, false) => Err(BlueprintError::AdapterUndetected.into()),
    }
}

fn load_atoms(args: &ExtractArgs) -> Result<(std::collections::BTreeMap<String, Atom>, Source)> {
    let lean_path = match &args.lean {
        Some(p) => p.clone(),
        None => run_probe_lean(&args.project)?,
    };
    // Reject this tool's own output as the atom base: enriching it again would
    // double-count blueprint nodes. A merged spine (schema `probe/merged-*`)
    // that happens to carry old blueprint atoms is fine — `enrich` scrubs it.
    let meta = load_envelope(&lean_path)
        .map_err(|e| anyhow::anyhow!("failed to load {}: {e}", lean_path.display()))?;
    if meta.schema.starts_with("probe-leanblueprint") {
        return Err(BlueprintError::NotLeanBase {
            path: lean_path.clone(),
            schema: meta.schema,
        }
        .into());
    }
    let (atoms, provenance) = load_atom_file(&lean_path).map_err(|e| {
        anyhow::anyhow!(
            "failed to load probe-lean atoms from {}: {e}",
            lean_path.display()
        )
    })?;
    let source = select_source(
        &provenance,
        args.source_package.as_deref(),
        args.source_version.as_deref(),
    )?;
    Ok((atoms, source))
}

/// Pick the provenance `Source` that best identifies the atom base.
///
/// Prefer the `probe-lean` input (the base this tool enriches) over blindly
/// taking the first `inputs` entry, which for a merged spine could belong to an
/// unrelated language. The `probe-lean/` prefix is matched with its delimiter so
/// this tool's own `probe-leanblueprint/*` provenance never counts as a lean
/// input. Error when multiple probe-lean inputs disagree on package identity (a
/// genuinely ambiguous spine), unless the caller supplies BOTH `pkg_override`
/// and `ver_override`, which fully disambiguate identity.
fn select_source(
    provenance: &[InputProvenance],
    pkg_override: Option<&str>,
    ver_override: Option<&str>,
) -> Result<Source> {
    let lean: Vec<&InputProvenance> = provenance
        .iter()
        .filter(|p| p.schema.starts_with("probe-lean/"))
        .collect();
    let both_overridden = pkg_override.is_some() && ver_override.is_some();

    let mut source = if let Some(first) = lean.first() {
        if !both_overridden {
            let all_same = lean.iter().all(|p| {
                p.source.package == first.source.package
                    && p.source.package_version == first.source.package_version
            });
            if !all_same {
                anyhow::bail!(
                    "ambiguous probe-lean provenance: {} distinct probe-lean inputs; \
                     pass --source-package and --source-version to disambiguate, or a \
                     --lean file with a single probe-lean input",
                    lean.len()
                );
            }
        }
        first.source.clone()
    } else if let Some(p) = provenance.first() {
        p.source.clone()
    } else {
        Source {
            repo: String::new(),
            commit: String::new(),
            language: "lean".to_string(),
            package: "unknown".to_string(),
            package_version: String::new(),
        }
    };

    if let Some(pkg) = pkg_override {
        source.package = pkg.to_string();
    }
    if let Some(ver) = ver_override {
        source.package_version = ver.to_string();
    }
    Ok(source)
}

/// Run `probe-lean extract` against the project (single incremental build) and
/// return the path to the produced JSON.
fn run_probe_lean(project: &Path) -> Result<PathBuf> {
    let out = project.join(".verilib/probes/probe-lean-extract.json");
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    eprintln!("Running probe-lean extract on {} ...", project.display());
    let status = Command::new("probe-lean")
        .arg("extract")
        .arg(project)
        .arg("-o")
        .arg(&out)
        .status()
        .context("failed to run probe-lean (is it installed and on PATH?)")?;
    if !status.success() {
        anyhow::bail!("probe-lean extract failed with status {status}");
    }
    Ok(out)
}

fn build_model(args: &ExtractArgs, adapter: ResolvedAdapter) -> Result<BlueprintModel> {
    match adapter {
        ResolvedAdapter::Verso => build_verso_model(args),
        ResolvedAdapter::Massot => build_massot_model(args),
    }
}

fn build_verso_model(args: &ExtractArgs) -> Result<BlueprintModel> {
    let model = match &args.verso_manifest {
        Some(p) if p.is_file() => verso::load_manifest(p)?,
        Some(p) => verso::load_from_dir(p)?,
        None => verso::load_from_dir(&args.project)?,
    };
    Ok(model)
}

fn build_massot_model(args: &ExtractArgs) -> Result<BlueprintModel> {
    let web_tex = match &args.blueprint_src {
        Some(p) if p.is_file() => p.clone(),
        Some(p) => p.join("web.tex"),
        None => args.project.join("blueprint/src/web.tex"),
    };
    if !web_tex.exists() {
        return Err(BlueprintError::WebTexNotFound(web_tex).into());
    }
    let emitter = emitter::resolve_emitter(args.emitter.as_deref())?;
    let model = massot::run(&args.python, emitter.path(), &web_tex)?;
    Ok(model)
}

/// Sanitize a provenance field for use in a filename, mirroring
/// `probe-rust`'s helper so package names with `/`, `\`, or `..` cannot escape
/// the output directory.
fn sanitize_for_filename(s: &str) -> String {
    s.replace(['/', '\\'], "_").replace("..", "_")
}

fn default_output(project: &Path, source: &Source, suffix: &str) -> PathBuf {
    let pkg = if source.package.is_empty() {
        "project".to_string()
    } else {
        sanitize_for_filename(&source.package)
    };
    let name = if source.package_version.is_empty() {
        format!("leanblueprint_{pkg}{suffix}.json")
    } else {
        format!(
            "leanblueprint_{pkg}_{}{suffix}.json",
            sanitize_for_filename(&source.package_version)
        )
    };
    project.join(".verilib/probes").join(name)
}

fn write_json<T: serde::Serialize>(path: &Path, value: &T) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let json = serde_json::to_string_pretty(value).context("failed to serialize output")?;
    std::fs::write(path, json).with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

fn run_extract(args: ExtractArgs) -> Result<()> {
    let adapter = detect_adapter(&args)?;

    let (mut atoms, source) = load_atoms(&args)?;
    let model = build_model(&args, adapter)?;

    let report = enrich::enrich(&mut atoms, &model);

    // Reuse the hub's transitive-verification enrichment (idempotent). Machine
    // verification-status remains authoritative on the proof axis.
    let (transitive, local, missing) =
        probe::commands::propagate::enrich_verification_status(&mut atoms);
    eprintln!(
        "Verification propagation: {transitive} transitively-verified, {local} locally-verified, \
         {} missing dependencies",
        missing.len()
    );

    let summary = enrich::summarize(&model, &report);

    let extract_path = args
        .output
        .clone()
        .unwrap_or_else(|| default_output(&args.project, &source, ""));
    let summary_path = args
        .summary_output
        .clone()
        .unwrap_or_else(|| default_output(&args.project, &source, "_summary"));

    let extract_env = emit::build_extract_envelope(atoms, source.clone());
    let summary_env = emit::build_summary_envelope(summary, source);

    write_json(&extract_path, &extract_env)?;
    write_json(&summary_path, &summary_env)?;

    eprintln!(
        "Blueprint nodes: {} ({} bound, {} planned-only, {} decl-missing, {} partial-missing, \
         {} collisions); mismatches: {}",
        report.nodes_total,
        report.nodes_with_decl,
        report.planned_only,
        report.decl_missing,
        report.partial_missing,
        report.collisions,
        report.mismatches.len()
    );
    eprintln!(
        "Headline: {}/{} theorems fully proved ({:.1}%)",
        summary_env.data.headline.theorems_fully_proved,
        summary_env.data.headline.theorems_total,
        summary_env.data.headline.fraction * 100.0
    );
    eprintln!("Wrote {}", extract_path.display());
    eprintln!("Wrote {}", summary_path.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_output_sanitizes_path_traversal() {
        let source = Source {
            repo: String::new(),
            commit: String::new(),
            language: "lean".to_string(),
            package: "../../etc".to_string(),
            package_version: "a/b\\c".to_string(),
        };
        let out = default_output(Path::new("/proj"), &source, "");
        let name = out.file_name().unwrap().to_str().unwrap();
        assert!(!name.contains(".."), "{name} must not contain ..");
        assert!(!name.contains('/'), "{name} must not contain /");
        assert!(!name.contains('\\'), "{name} must not contain backslash");
        assert_eq!(out.parent().unwrap(), Path::new("/proj/.verilib/probes"));
    }

    #[test]
    fn select_source_prefers_probe_lean_input() {
        let provenance = vec![
            InputProvenance {
                schema: "probe-rust/extract".to_string(),
                source: Source {
                    repo: String::new(),
                    commit: String::new(),
                    language: "rust".to_string(),
                    package: "rustpkg".to_string(),
                    package_version: String::new(),
                },
            },
            InputProvenance {
                schema: "probe-lean/extract".to_string(),
                source: Source {
                    repo: String::new(),
                    commit: String::new(),
                    language: "lean".to_string(),
                    package: "leanpkg".to_string(),
                    package_version: String::new(),
                },
            },
        ];
        let source = select_source(&provenance, None, None).unwrap();
        assert_eq!(source.package, "leanpkg");
    }

    #[test]
    fn select_source_ignores_own_schema_prefix() {
        // "probe-leanblueprint/*" must not be treated as a probe-lean input.
        let provenance = vec![
            InputProvenance {
                schema: "probe-leanblueprint/extract".to_string(),
                source: Source {
                    repo: String::new(),
                    commit: String::new(),
                    language: "blueprint".to_string(),
                    package: "blueprintpkg".to_string(),
                    package_version: String::new(),
                },
            },
            InputProvenance {
                schema: "probe-lean/extract".to_string(),
                source: Source {
                    repo: String::new(),
                    commit: String::new(),
                    language: "lean".to_string(),
                    package: "leanpkg".to_string(),
                    package_version: String::new(),
                },
            },
        ];
        let source = select_source(&provenance, None, None).unwrap();
        assert_eq!(source.package, "leanpkg");
    }

    #[test]
    fn select_source_overrides_disambiguate() {
        // Two distinct probe-lean inputs would be ambiguous, but supplying both
        // overrides bypasses the check and sets identity directly.
        let provenance = vec![
            InputProvenance {
                schema: "probe-lean/extract".to_string(),
                source: Source {
                    repo: String::new(),
                    commit: String::new(),
                    language: "lean".to_string(),
                    package: "a".to_string(),
                    package_version: "1".to_string(),
                },
            },
            InputProvenance {
                schema: "probe-lean/extract".to_string(),
                source: Source {
                    repo: String::new(),
                    commit: String::new(),
                    language: "lean".to_string(),
                    package: "b".to_string(),
                    package_version: "2".to_string(),
                },
            },
        ];
        assert!(select_source(&provenance, None, None).is_err());
        let source = select_source(&provenance, Some("chosen"), Some("9")).unwrap();
        assert_eq!(source.package, "chosen");
        assert_eq!(source.package_version, "9");
    }
}
