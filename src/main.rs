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
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ResolvedAdapter {
    Verso,
    Massot,
}

/// Outcome of adapter detection: which adapter, plus the directory whose lakefile
/// declares `versoBlueprint`. That directory (the project root or its `docs/`
/// subproject) is where the Verso render runs and where its `_out/site` output
/// lives — distinct from the math-project root that probe-lean extracts. `None`
/// for Massot or when no Verso signal was found (render/discovery then use the
/// project root).
#[derive(Debug)]
struct Detected {
    adapter: ResolvedAdapter,
    verso_blueprint_root: Option<PathBuf>,
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

    /// Command used to render the Verso docs when no `blueprint-manifest.json`
    /// is found under the project (run via `sh -c` in the project directory).
    /// Defaults to `lake exe vbp build`, the render entry point that every
    /// `versoBlueprint`-dependent project exposes.
    #[arg(long)]
    verso_render_cmd: Option<String>,

    /// Do not attempt to render Verso docs; require a pre-existing manifest
    /// (for callers that render the docs themselves before invoking this tool).
    #[arg(long)]
    no_render: bool,

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

/// The directory whose lakefile declares `versoBlueprint`, if any. The
/// dependency is frequently declared in a dedicated blueprint subproject rather
/// than the root lakefile (e.g. KVAC's `docs/lakefile.toml`), so scan the root
/// *and* the conventional `docs/` subproject.
fn verso_blueprint_root(project: &Path) -> Option<PathBuf> {
    for sub in ["", "docs"] {
        let dir = project.join(sub);
        let signal = ["lakefile.toml", "lakefile.lean"].iter().any(|lf| {
            std::fs::read_to_string(dir.join(lf))
                .map(|text| text.contains("versoBlueprint") || text.contains("verso-blueprint"))
                .unwrap_or(false)
        });
        if signal {
            return Some(dir);
        }
    }
    None
}

fn detect_adapter(args: &ExtractArgs) -> Result<Detected> {
    match args.adapter {
        Adapter::Verso => {
            return Ok(Detected {
                adapter: ResolvedAdapter::Verso,
                // Honor an explicit docs/ subproject even when forced.
                verso_blueprint_root: verso_blueprint_root(&args.project),
            });
        }
        Adapter::Massot => {
            return Ok(Detected {
                adapter: ResolvedAdapter::Massot,
                verso_blueprint_root: None,
            })
        }
        Adapter::Auto => {}
    }
    if args.verso_manifest.is_some() {
        return Ok(Detected {
            adapter: ResolvedAdapter::Verso,
            verso_blueprint_root: verso_blueprint_root(&args.project),
        });
    }
    if args.blueprint_src.is_some() {
        return Ok(Detected {
            adapter: ResolvedAdapter::Massot,
            verso_blueprint_root: None,
        });
    }
    // Auto-detect from the project layout. Check the Verso signal (a
    // `versoBlueprint` lakefile declaration) first: a Verso project may carry a
    // leftover/migrated Massot `blueprint/` tree, and defaulting to Massot in
    // that case would silently pick the wrong ecosystem.
    let has_web_tex = args.project.join("blueprint/src/web.tex").exists();
    let verso_root = verso_blueprint_root(&args.project);
    match (verso_root, has_web_tex) {
        (Some(root), true) => {
            eprintln!(
                "warning: found both a versoBlueprint lakefile signal and \
                 blueprint/src/web.tex; using Verso (pass --adapter to override)"
            );
            Ok(Detected {
                adapter: ResolvedAdapter::Verso,
                verso_blueprint_root: Some(root),
            })
        }
        (Some(root), false) => Ok(Detected {
            adapter: ResolvedAdapter::Verso,
            verso_blueprint_root: Some(root),
        }),
        (None, true) => Ok(Detected {
            adapter: ResolvedAdapter::Massot,
            verso_blueprint_root: None,
        }),
        // No positive signal: fail loudly instead of defaulting to Verso and
        // then dying later with a confusing "no manifest" error.
        (None, false) => Err(BlueprintError::AdapterUndetected.into()),
    }
}

/// Loads the atom base and returns it with its provenance-derived `Source` and
/// the resolved atom-file path (explicit `--lean`, or the file `probe-lean`
/// generated). The path is returned so output-collision validation can also
/// protect a generated atom base, not just an explicit one.
fn load_atoms(
    args: &ExtractArgs,
) -> Result<(std::collections::BTreeMap<String, Atom>, Source, PathBuf)> {
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
    Ok((atoms, source, lean_path))
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
            extensions: Default::default(),
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

fn build_model(
    args: &ExtractArgs,
    detected: &Detected,
) -> Result<(BlueprintModel, emit::BlueprintProvenance)> {
    match detected.adapter {
        ResolvedAdapter::Verso => {
            // Render + manifest discovery happen in the blueprint subproject (the
            // dir whose lakefile declares versoBlueprint), which may be `docs/`,
            // not the math-project root that probe-lean extracts.
            let render_root = detected
                .verso_blueprint_root
                .clone()
                .unwrap_or_else(|| args.project.clone());
            let model = build_verso_model(args, &render_root)?;
            // Record exactly which manifest(s) fed this run (post-render).
            let manifest_paths = match &args.verso_manifest {
                Some(p) if p.is_file() => vec![p.clone()],
                Some(p) => verso::discover_manifests(p)?,
                None => verso::discover_manifests(&render_root.join(VERSO_SITE_SUBDIR))?,
            };
            let provenance = emit::BlueprintProvenance {
                adapter: "verso".to_string(),
                manifests: manifest_paths.iter().map(|p| manifest_ref(p)).collect(),
                web_tex: None,
            };
            Ok((model, provenance))
        }
        ResolvedAdapter::Massot => {
            let model = build_massot_model(args)?;
            let provenance = emit::BlueprintProvenance {
                adapter: "massot".to_string(),
                manifests: Vec::new(),
                web_tex: Some(massot_web_tex(args).display().to_string()),
            };
            Ok((model, provenance))
        }
    }
}

/// Build a provenance record for one manifest: its path, SHA-256, and the
/// `vbpInternalSchemaVersion` it declares (best-effort; unreadable/unparsable
/// files still yield a path so the record is never silently empty).
fn manifest_ref(path: &Path) -> emit::ManifestRef {
    use sha2::{Digest, Sha256};
    let bytes = std::fs::read(path).unwrap_or_default();
    let sha256 = hex(&Sha256::digest(&bytes));
    let vbp_internal_schema_version = serde_json::from_slice::<serde_json::Value>(&bytes)
        .ok()
        .and_then(|v| v.get("vbpInternalSchemaVersion").and_then(|n| n.as_u64()));
    emit::ManifestRef {
        path: path.display().to_string(),
        sha256,
        vbp_internal_schema_version,
    }
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// Default Verso render command. `vbp` is shipped by the `versoBlueprint`
/// dependency, so `lake exe vbp build` is available in any project that requires
/// it; it auto-discovers the project's generator entry point and writes the
/// site (including `blueprint-manifest.json`) under `_out/site/`.
const DEFAULT_VERSO_RENDER_CMD: &str = "lake exe vbp build";

/// Canonical Verso render-output subdirectory, relative to the blueprint root.
/// `lake exe vbp build` writes the site (and its `blueprint-manifest.json`s)
/// here; scoping discovery to it — rather than walking the whole project — is
/// what keeps stale sibling renders (e.g. `_out/site-v430`) from being merged in.
const VERSO_SITE_SUBDIR: &str = "_out/site";

fn build_verso_model(args: &ExtractArgs, render_root: &Path) -> Result<BlueprintModel> {
    // An explicit `--verso-manifest` is authoritative and never triggers a
    // render (the caller has pointed us at the artifact directly).
    if let Some(p) = &args.verso_manifest {
        return Ok(if p.is_file() {
            verso::load_manifest(p)?
        } else {
            verso::load_from_dir(p)?
        });
    }

    // Discovery is scoped to the canonical render-output root so a single fresh
    // generation is read, never a merge of whatever `blueprint-manifest.json`s
    // happen to litter the project (stale `_out/site-v430` leftovers, etc.).
    let site = render_root.join(VERSO_SITE_SUBDIR);
    match verso::load_from_dir(&site) {
        Ok(model) => Ok(model),
        Err(BlueprintError::NoManifest(_)) => {
            if args.no_render {
                return Err(BlueprintError::NoManifest(site).into());
            }
            render_verso_docs(args, render_root)?;
            // Retry: the render must have produced at least one manifest under
            // the canonical site root.
            Ok(verso::load_from_dir(&site)?)
        }
        Err(e) => Err(e.into()),
    }
}

/// Render the Verso docs in-place so `blueprint-manifest.json` exists. Runs the
/// render command through `sh -c` in the blueprint root (the dir whose lakefile
/// declares versoBlueprint — possibly `docs/`, where `lake exe vbp build` can
/// actually resolve `vbp`). This is what makes the Verso path work from a bare
/// Lean project, mirroring the Massot path's embedded plasTeX emitter.
fn render_verso_docs(args: &ExtractArgs, render_root: &Path) -> Result<()> {
    let cmd = args
        .verso_render_cmd
        .as_deref()
        .unwrap_or(DEFAULT_VERSO_RENDER_CMD);
    eprintln!(
        "No blueprint-manifest.json under {}; rendering Verso docs with `{cmd}` in {} ...",
        render_root.join(VERSO_SITE_SUBDIR).display(),
        render_root.display()
    );
    let output = Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .current_dir(render_root)
        .output()
        .map_err(|source| BlueprintError::VersoRenderSpawn {
            cmd: cmd.to_string(),
            source,
        })?;
    if !output.status.success() {
        return Err(BlueprintError::VersoRenderFailed {
            cmd: cmd.to_string(),
            status: output.status.to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        }
        .into());
    }
    Ok(())
}

/// Resolve the Massot `web.tex` path: an explicit file, a directory's `web.tex`,
/// or the conventional `<project>/blueprint/src/web.tex`.
fn massot_web_tex(args: &ExtractArgs) -> PathBuf {
    match &args.blueprint_src {
        Some(p) if p.is_file() => p.clone(),
        Some(p) => p.join("web.tex"),
        None => args.project.join("blueprint/src/web.tex"),
    }
}

fn build_massot_model(args: &ExtractArgs) -> Result<BlueprintModel> {
    let web_tex = massot_web_tex(args);
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

/// Serialize `value` into a temp file in `path`'s directory, fully written and
/// flushed but *not* yet moved into place. Pair with [`commit_staged`]: staging
/// both outputs before committing either means a serialization or disk-full
/// error can't leave one file updated and the other stale.
fn stage_json<T: serde::Serialize>(path: &Path, value: &T) -> Result<tempfile::NamedTempFile> {
    let dir = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => {
            std::fs::create_dir_all(p)
                .with_context(|| format!("failed to create {}", p.display()))?;
            p.to_path_buf()
        }
        _ => PathBuf::from("."),
    };
    let json = serde_json::to_string_pretty(value).context("failed to serialize output")?;
    use std::io::Write as _;
    let mut tmp = tempfile::NamedTempFile::new_in(&dir)
        .with_context(|| format!("failed to create temp file in {}", dir.display()))?;
    tmp.write_all(json.as_bytes())
        .with_context(|| format!("failed to write {}", path.display()))?;
    tmp.as_file()
        .sync_all()
        .with_context(|| format!("failed to flush {}", path.display()))?;
    Ok(tmp)
}

/// Move a staged temp file into its final path (an atomic rename on the same
/// filesystem).
fn commit_staged(tmp: tempfile::NamedTempFile, path: &Path) -> Result<()> {
    tmp.persist(path)
        .map_err(|e| e.error)
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

/// Best-effort absolute path for comparison. Output paths may not exist yet, so
/// fall back to canonicalizing the parent and re-appending the file name.
fn normalize_for_compare(p: &Path) -> PathBuf {
    if let Ok(c) = p.canonicalize() {
        return c;
    }
    match (p.parent(), p.file_name()) {
        (Some(parent), Some(name)) => parent
            .canonicalize()
            .map(|c| c.join(name))
            .unwrap_or_else(|_| p.to_path_buf()),
        _ => p.to_path_buf(),
    }
}

/// Reject output paths that collide with each other or with an input file, so
/// the summary can't silently overwrite the extract (or clobber an input).
fn validate_output_paths(
    extract: &Path,
    summary: &Path,
    inputs: &[Option<&PathBuf>],
) -> Result<()> {
    let extract_n = normalize_for_compare(extract);
    let summary_n = normalize_for_compare(summary);
    if extract_n == summary_n {
        anyhow::bail!(
            "--output and --summary-output must be different files (both resolve to {})",
            extract.display()
        );
    }
    for input in inputs.iter().flatten() {
        let input_n = normalize_for_compare(input);
        if input_n == extract_n || input_n == summary_n {
            anyhow::bail!(
                "refusing to overwrite input file {} with an output",
                input.display()
            );
        }
    }
    Ok(())
}

fn run_extract(args: ExtractArgs) -> Result<()> {
    let detected = detect_adapter(&args)?;

    let (mut atoms, source, lean_path) = load_atoms(&args)?;
    let (model, provenance) = build_model(&args, &detected)?;

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

    validate_output_paths(
        &extract_path,
        &summary_path,
        &[
            // The resolved atom base — an explicit `--lean`, or the file
            // `probe-lean` just generated under `.verilib/probes/`. Either way an
            // output must not clobber it (we already read it).
            Some(&lean_path),
            args.verso_manifest.as_ref(),
            args.blueprint_src.as_ref(),
        ],
    )?;

    let extract_env = emit::build_extract_envelope(atoms, source.clone());
    let summary_env = emit::build_summary_envelope(summary, source, provenance);

    // Stage both outputs (serialize + flush to temp files) before publishing
    // either, so a failure staging the summary can't leave a fresh extract
    // beside a stale/missing summary. The only residual window is a crash
    // between the two renames below.
    let extract_tmp = stage_json(&extract_path, &extract_env)?;
    let summary_tmp = stage_json(&summary_path, &summary_env)?;
    commit_staged(extract_tmp, &extract_path)?;
    commit_staged(summary_tmp, &summary_path)?;

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
    let h = &summary_env.data.headline;
    eprintln!(
        "Headline: {}/{} theorems machine-confirmed fully proved ({:.1}%)",
        h.theorems_fully_proved_machine_confirmed,
        h.theorems_total,
        h.fraction_machine_confirmed * 100.0
    );
    // Surface the blueprint's own claim only when it exceeds what the machine
    // backs, so a `declared` blueprint's over-claim is visible, not hidden.
    if h.theorems_fully_proved > h.theorems_fully_proved_machine_confirmed {
        eprintln!(
            "  (blueprint claims {}/{}; {} not backed by probe-lean's verification status)",
            h.theorems_fully_proved,
            h.theorems_total,
            h.theorems_fully_proved - h.theorems_fully_proved_machine_confirmed
        );
    }
    eprintln!("Wrote {}", extract_path.display());
    eprintln!("Wrote {}", summary_path.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal `ExtractArgs` for the Verso path, rooted at `project`.
    fn verso_args(project: PathBuf) -> ExtractArgs {
        ExtractArgs {
            project,
            lean: None,
            adapter: Adapter::Verso,
            verso_manifest: None,
            verso_render_cmd: None,
            no_render: false,
            blueprint_src: None,
            python: "python3".into(),
            emitter: None,
            output: None,
            summary_output: None,
            source_package: None,
            source_version: None,
        }
    }

    #[test]
    fn render_verso_docs_runs_command_in_project_dir() {
        let dir = tempfile::tempdir().unwrap();
        let mut args = verso_args(dir.path().to_path_buf());
        args.verso_render_cmd = Some("touch rendered.marker".to_string());
        render_verso_docs(&args, dir.path()).unwrap();
        assert!(
            dir.path().join("rendered.marker").exists(),
            "render command must run in the render root"
        );
    }

    #[test]
    fn render_verso_docs_surfaces_nonzero_exit() {
        let dir = tempfile::tempdir().unwrap();
        let mut args = verso_args(dir.path().to_path_buf());
        args.verso_render_cmd = Some("exit 7".to_string());
        let err = render_verso_docs(&args, dir.path()).unwrap_err();
        assert!(
            matches!(
                err.downcast_ref::<BlueprintError>(),
                Some(BlueprintError::VersoRenderFailed { .. })
            ),
            "non-zero render exit must map to VersoRenderFailed, got: {err}"
        );
    }

    #[test]
    fn build_verso_model_no_render_errors_without_manifest() {
        let dir = tempfile::tempdir().unwrap();
        let mut args = verso_args(dir.path().to_path_buf());
        args.no_render = true;
        let render_root = dir.path().to_path_buf();
        let err = build_verso_model(&args, &render_root).unwrap_err();
        assert!(
            matches!(
                err.downcast_ref::<BlueprintError>(),
                Some(BlueprintError::NoManifest(_))
            ),
            "--no-render must not attempt a render, got: {err}"
        );
    }

    #[test]
    fn build_verso_model_renders_then_loads_manifest() {
        // A stand-in render command drops a real manifest fixture into the
        // project; build_verso_model must render, then find and parse it.
        let dir = tempfile::tempdir().unwrap();
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/verso/erasure-codes-manifest.json");
        let mut args = verso_args(dir.path().to_path_buf());
        args.verso_render_cmd = Some(format!(
            "mkdir -p _out/site/html-multi/Erasure-Codes/-verso-data && \
             cp {} _out/site/html-multi/Erasure-Codes/-verso-data/blueprint-manifest.json",
            fixture.display()
        ));
        let render_root = dir.path().to_path_buf();
        let model = build_verso_model(&args, &render_root).unwrap();
        assert_eq!(model.nodes.len(), 4, "rendered manifest is loaded");
    }

    #[test]
    fn build_verso_model_ignores_stale_sibling_renders() {
        // A fresh render under _out/site plus a stale generation under
        // _out/site-v430. Discovery is scoped to _out/site, so the stale node
        // must not be merged in (which would inflate the count).
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/verso/erasure-codes-manifest.json");
        let site = root.join("_out/site/html-multi/Erasure-Codes/-verso-data");
        std::fs::create_dir_all(&site).unwrap();
        std::fs::copy(&fixture, site.join("blueprint-manifest.json")).unwrap();
        // Stale sibling with a unique extra node that must NOT appear.
        let stale = root.join("_out/site-v430/html-multi/Old/-verso-data");
        std::fs::create_dir_all(&stale).unwrap();
        std::fs::write(
            stale.join("blueprint-manifest.json"),
            r#"{"vbpInternalSchemaVersion":3,"graphs":[{"nodes":[
                {"label":"stale_only","kind":"theorem","statementStatus":"formalized",
                 "proofStatus":"formalizedWithAncestors"}]}],"previews":[]}"#,
        )
        .unwrap();

        let mut args = verso_args(root.to_path_buf());
        args.no_render = true; // a manifest already exists under _out/site
        let model = build_verso_model(&args, root).unwrap();
        assert_eq!(model.nodes.len(), 4, "only the _out/site render is read");
        assert!(
            !model.nodes.iter().any(|n| n.label == "stale_only"),
            "stale _out/site-v430 render must not be merged in"
        );
    }

    #[test]
    fn detect_adapter_finds_verso_in_docs_subproject() {
        // KVAC-style layout: the root lakefile has no verso signal; the
        // `versoBlueprint` dependency is declared in `docs/lakefile.toml`.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("lakefile.toml"), "name = \"kvac\"\n").unwrap();
        std::fs::create_dir(dir.path().join("docs")).unwrap();
        std::fs::write(
            dir.path().join("docs/lakefile.toml"),
            "[[require]]\nname = \"versoBlueprint\"\n",
        )
        .unwrap();
        let mut args = verso_args(dir.path().to_path_buf());
        args.adapter = Adapter::Auto;
        let detected = detect_adapter(&args).unwrap();
        assert_eq!(detected.adapter, ResolvedAdapter::Verso);
        assert_eq!(
            detected.verso_blueprint_root.as_deref(),
            Some(dir.path().join("docs").as_path()),
            "render root must be the docs/ subproject that declares versoBlueprint"
        );
    }

    #[test]
    fn detect_adapter_errors_without_any_signal() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("lakefile.toml"), "name = \"plain\"\n").unwrap();
        let mut args = verso_args(dir.path().to_path_buf());
        args.adapter = Adapter::Auto;
        let err = detect_adapter(&args).unwrap_err();
        assert!(
            matches!(
                err.downcast_ref::<BlueprintError>(),
                Some(BlueprintError::AdapterUndetected)
            ),
            "no signal must yield AdapterUndetected, got: {err}"
        );
    }

    #[test]
    fn default_output_sanitizes_path_traversal() {
        let source = Source {
            repo: String::new(),
            commit: String::new(),
            language: "lean".to_string(),
            package: "../../etc".to_string(),
            package_version: "a/b\\c".to_string(),
            extensions: Default::default(),
        };
        let out = default_output(Path::new("/proj"), &source, "");
        let name = out.file_name().unwrap().to_str().unwrap();
        assert!(!name.contains(".."), "{name} must not contain ..");
        assert!(!name.contains('/'), "{name} must not contain /");
        assert!(!name.contains('\\'), "{name} must not contain backslash");
        assert_eq!(out.parent().unwrap(), Path::new("/proj/.verilib/probes"));
    }

    #[test]
    fn validate_output_paths_rejects_collisions() {
        let dir = tempfile::tempdir().unwrap();
        let extract = dir.path().join("out.json");
        let summary = dir.path().join("out_summary.json");
        let input = dir.path().join("atoms.json");
        std::fs::write(&input, "{}").unwrap();

        // Distinct paths are fine.
        assert!(validate_output_paths(&extract, &summary, &[Some(&input)]).is_ok());

        // Extract == summary is rejected.
        assert!(validate_output_paths(&extract, &extract, &[]).is_err());

        // An output that would clobber an input is rejected.
        assert!(validate_output_paths(&input, &summary, &[Some(&input)]).is_err());
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
                    extensions: Default::default(),
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
                    extensions: Default::default(),
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
                    extensions: Default::default(),
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
                    extensions: Default::default(),
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
                    extensions: Default::default(),
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
                    extensions: Default::default(),
                },
            },
        ];
        assert!(select_source(&provenance, None, None).is_err());
        let source = select_source(&provenance, Some("chosen"), Some("9")).unwrap();
        assert_eq!(source.package, "chosen");
        assert_eq!(source.package_version, "9");
    }
}
