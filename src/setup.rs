//! Resolve or install the `probe-lean` binary used by [`crate`]'s
//! zero-config `extract` path.
//!
//! Binary-only (not part of the `probe_leanblueprint` library): nothing
//! outside this crate's `main.rs` needs it, so it stays a private
//! implementation detail rather than public API surface.
//!
//! Ported from `probe-aeneas`'s `find_or_install_probe_lean`
//! (<https://github.com/Beneficial-AI-Foundation/probe-aeneas>), scoped down
//! to the single tool this crate depends on, and hardened:
//! - no unversioned-symlink management (`probe-leanblueprint` always invokes
//!   the resolved binary by its absolute path, so there is nothing to gain —
//!   and a user-managed `~/.local/bin/probe-lean` to lose — by touching it);
//! - a malformed/unreadable `lean-toolchain` is a hard error rather than
//!   silently falling back to an unversioned/incompatible binary;
//! - downloads/builds use `tempfile::TempDir` rather than fixed `/tmp` paths;
//! - the GitHub releases API response is parsed as JSON rather than
//!   line-grepped;
//! - binary (and lib dir) installs are staged then atomically published
//!   (temp file/dir + rename) rather than copied in place, so a concurrent
//!   `find_or_install_probe_lean` never observes a partially-written cache
//!   entry;
//! - a prebuilt-download failure other than "no matching release" (network
//!   error, bad archive, GitHub API hiccup) is propagated as-is under
//!   `PROBE_LEANBLUEPRINT_RELEASES_ONLY` instead of being reported as
//!   `ReleasesOnlyNoMatch`, which would otherwise misstate the cause;
//! - `PROBE_LEANBLUEPRINT_RELEASES_ONLY` disables the from-source fallback
//!   (which otherwise builds `probe-lean`'s floating `main` branch) so a
//!   production consumer can require tagged releases only;
//! - `PROBE_LEANBLUEPRINT_NO_AUTO_INSTALL` disables installation entirely,
//!   restoring today's "must already be present" behavior.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::Context;
use thiserror::Error;

const PROBE_LEAN_GIT: &str = "https://github.com/Beneficial-AI-Foundation/probe-lean.git";

/// A recoverable failure while resolving or installing `probe-lean`.
#[derive(Debug, Error)]
pub enum LeanInstallError {
    #[error("could not determine home directory")]
    HomeDirUnavailable,

    #[error("lean-toolchain is present but invalid ({0})")]
    LeanToolchainInvalid(String),

    #[error("no prebuilt probe-lean release found for Lean {lean_version} ({platform})")]
    NoPrebuiltAvailable {
        lean_version: String,
        platform: String,
    },

    #[error(
        "source build disabled (PROBE_LEANBLUEPRINT_RELEASES_ONLY is set) and no \
         prebuilt probe-lean release matches Lean {0}"
    )]
    ReleasesOnlyNoMatch(String),

    #[error("{command} exited with status {code}")]
    SubprocessFailed { command: String, code: i32 },

    #[error("{command} completed but {} was not created", path.display())]
    MissingOutput { command: String, path: PathBuf },

    #[error(
        "lake build failed (exit {code}).\n  Make sure elan/lean4 and lake are \
         installed: https://github.com/leanprover/elan\n  stderr: {stderr}"
    )]
    LakeBuildFailed { code: i32, stderr: String },

    #[error(
        "probe-lean not found and auto-install disabled \
         (PROBE_LEANBLUEPRINT_NO_AUTO_INSTALL is set)"
    )]
    AutoInstallDisabled,

    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

type Result<T> = std::result::Result<T, LeanInstallError>;

/// Resolve the `probe-lean` binary to run against `project`, installing it
/// if necessary.
///
/// Resolution order: a cached binary version-matched to `project`'s
/// `lean-toolchain` (or, absent a toolchain file, an unversioned binary on
/// `PATH` / `~/.local/bin/probe-lean`); failing that, a prebuilt release
/// download; failing that, a source build (unless disabled — see the module
/// docs for the two `PROBE_LEANBLUEPRINT_*` environment overrides).
pub(crate) fn find_or_install_probe_lean(project: &Path) -> Result<PathBuf> {
    let lean_version = detect_lean_version(project)?;

    if let Some(ref version) = lean_version {
        let versioned_bin = home_dir()?.join(format!(".local/bin/probe-lean-{version}"));
        if versioned_bin.exists() {
            eprintln!("Using versioned probe-lean for Lean {version}");
            return Ok(versioned_bin);
        }
        // A specific Lean version is required; skip the unversioned PATH /
        // ~/.local/bin fallbacks below, since they may be built for a
        // different Lean version with an incompatible `.olean` format.
    } else if let Some(p) = find_on_path("probe-lean") {
        return Ok(p);
    } else {
        let local_bin = home_dir()?.join(".local/bin/probe-lean");
        if local_bin.exists() {
            return Ok(local_bin);
        }
    }

    if std::env::var_os("PROBE_LEANBLUEPRINT_NO_AUTO_INSTALL").is_some() {
        return Err(LeanInstallError::AutoInstallDisabled);
    }

    let releases_only = std::env::var_os("PROBE_LEANBLUEPRINT_RELEASES_ONLY").is_some();
    let version = lean_version.unwrap_or_else(|| "latest".to_string());
    eprintln!("probe-lean not found for Lean {version}, installing...");

    if version != "latest" {
        match try_prebuilt_download(&version) {
            Ok(bin) => return Ok(bin),
            Err(e) => {
                let no_matching_release = matches!(e, LeanInstallError::NoPrebuiltAvailable { .. });
                eprintln!("  prebuilt probe-lean unavailable for Lean {version}: {e}");
                // Under releases-only mode, only a genuine "no release
                // matches" is reported as ReleasesOnlyNoMatch below — any
                // other failure (network error, bad archive, GitHub API
                // hiccup) is propagated as-is so callers aren't told "no
                // release exists" when the real cause was e.g. a rate limit.
                if releases_only && !no_matching_release {
                    return Err(e);
                }
            }
        }
    }

    if releases_only {
        return Err(LeanInstallError::ReleasesOnlyNoMatch(version));
    }

    build_from_source(&version)
}

/// Read the Lean version from `project`'s `lean-toolchain` file.
///
/// A missing file is tolerated (`Ok(None)`, resolved by the caller against
/// an unversioned/latest binary); an unreadable, empty, or otherwise
/// malformed file is a hard error, since silently proceeding risks
/// installing or reusing a binary built for the wrong Lean version.
fn detect_lean_version(project: &Path) -> Result<Option<String>> {
    let toolchain_path = project.join("lean-toolchain");
    let content = match std::fs::read_to_string(&toolchain_path) {
        Ok(content) => content,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(LeanInstallError::Other(anyhow::Error::new(e).context(
                format!("read lean-toolchain at {}", toolchain_path.display()),
            )))
        }
    };

    let trimmed = content.trim();
    let version = match trimmed.split(':').nth(1) {
        Some(after_colon) => after_colon.trim().to_string(),
        None => trimmed.to_string(),
    };
    if version.is_empty() {
        return Err(LeanInstallError::LeanToolchainInvalid(format!(
            "{} is empty or blank",
            toolchain_path.display()
        )));
    }
    // `version` is embedded verbatim into cache paths like
    // `~/.local/bin/probe-lean-{version}`; a project could otherwise plant a
    // `lean-toolchain` containing `/` or `..` to make installs write outside
    // the intended cache directory. Require it to be exactly one normal path
    // component.
    let is_single_safe_component = matches!(
        Path::new(&version)
            .components()
            .collect::<Vec<_>>()
            .as_slice(),
        [std::path::Component::Normal(_)]
    );
    if !is_single_safe_component {
        return Err(LeanInstallError::LeanToolchainInvalid(format!(
            "{} has an unsafe version string {version:?} (expected a single path segment)",
            toolchain_path.display()
        )));
    }
    Ok(Some(version))
}

/// Detect platform as `{os}-{arch}` for pre-built binary downloads.
fn detect_platform() -> String {
    let os = if cfg!(target_os = "linux") {
        "linux"
    } else if cfg!(target_os = "macos") {
        "darwin"
    } else {
        "unknown"
    };
    let arch = if cfg!(target_arch = "x86_64") {
        "x86_64"
    } else if cfg!(target_arch = "aarch64") {
        "arm64"
    } else {
        "unknown"
    };
    format!("{os}-{arch}")
}

/// Try downloading a pre-built `probe-lean` binary from GitHub Releases.
fn try_prebuilt_download(lean_version: &str) -> Result<PathBuf> {
    let platform = detect_platform();
    if platform.contains("unknown") {
        // No prebuilt release could possibly match; skip the curl/tar
        // requirement below entirely rather than demanding tools that
        // wouldn't be used anyway.
        return Err(LeanInstallError::NoPrebuiltAvailable {
            lean_version: lean_version.to_string(),
            platform,
        });
    }

    for tool in ["curl", "tar"] {
        if find_on_path(tool).is_none() {
            return Err(anyhow::anyhow!(
                "`{tool}` not found on PATH; required to download a prebuilt probe-lean release"
            )
            .into());
        }
    }

    let artifact = format!("probe-lean-{lean_version}-{platform}.tar.gz");
    eprintln!("  Checking for pre-built binary: {artifact}...");

    // GitHub's API rejects unauthenticated requests with no User-Agent
    // header; per_page=100 avoids missing older releases once probe-lean
    // has more than a page's worth (GitHub's default page size is 30).
    let output = Command::new("curl")
        .args([
            "-fsSL",
            "-H",
            "User-Agent: probe-leanblueprint",
            "https://api.github.com/repos/Beneficial-AI-Foundation/probe-lean/releases?per_page=100",
        ])
        .output()
        .context("spawn curl to query GitHub releases")?;
    if !output.status.success() {
        return Err(LeanInstallError::SubprocessFailed {
            command: "curl (list probe-lean releases)".to_string(),
            code: output.status.code().unwrap_or(-1),
        });
    }

    let releases: serde_json::Value =
        serde_json::from_slice(&output.stdout).context("parse GitHub releases API response")?;
    let download_url = releases
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|release| release.get("assets")?.as_array())
        .flatten()
        .find_map(|asset| {
            let name = asset.get("name")?.as_str()?;
            if name != artifact {
                return None;
            }
            asset
                .get("browser_download_url")?
                .as_str()
                .map(str::to_string)
        })
        .ok_or_else(|| LeanInstallError::NoPrebuiltAvailable {
            lean_version: lean_version.to_string(),
            platform: platform.clone(),
        })?;

    eprintln!("  Downloading pre-built binary...");
    let tmpdir = tempfile::tempdir().context("create temp dir for probe-lean download")?;
    let archive_path = tmpdir.path().join(&artifact);

    let status = Command::new("curl")
        .args(["-fsSL", "-H", "User-Agent: probe-leanblueprint", "-o"])
        .arg(&archive_path)
        .arg(&download_url)
        .status()
        .context("spawn curl to download probe-lean archive")?;
    if !status.success() || !archive_path.exists() {
        return Err(LeanInstallError::SubprocessFailed {
            command: format!("curl -o {artifact}"),
            code: status.code().unwrap_or(-1),
        });
    }

    // Path-traversal guard: list entries before extracting anything.
    let listing = Command::new("tar")
        .arg("-tzf")
        .arg(&archive_path)
        .output()
        .context("spawn tar to list probe-lean archive contents")?;
    if !listing.status.success() {
        return Err(LeanInstallError::SubprocessFailed {
            command: "tar -tzf".to_string(),
            code: listing.status.code().unwrap_or(-1),
        });
    }
    let entries = String::from_utf8_lossy(&listing.stdout);
    for entry in entries.lines() {
        if entry.starts_with('/') || entry.split('/').any(|part| part == "..") {
            return Err(anyhow::anyhow!(
                "refusing to extract probe-lean archive with unsafe entry path: {entry}"
            )
            .into());
        }
    }

    // The name-based guard above can't catch a *symlink* entry (e.g. `lib ->
    // /etc`, followed by `lib/passwd`) — the entry names look safe, but `tar`
    // would be extracting through a symlinked directory component. GNU tar
    // and modern libarchive both already refuse this during extraction, but
    // that's an implementation detail of the system `tar`, not something
    // this code enforces itself; reject any non-regular-file/non-directory
    // entry (symlink, hardlink, device, fifo) outright before extracting, so
    // the guard doesn't depend on which `tar` happens to be on PATH.
    let verbose_listing = Command::new("tar")
        .arg("-tvzf")
        .arg(&archive_path)
        .output()
        .context("spawn tar to list probe-lean archive contents (verbose)")?;
    if !verbose_listing.status.success() {
        return Err(LeanInstallError::SubprocessFailed {
            command: "tar -tvzf".to_string(),
            code: verbose_listing.status.code().unwrap_or(-1),
        });
    }
    let verbose_entries = String::from_utf8_lossy(&verbose_listing.stdout);
    for line in verbose_entries.lines() {
        match line.chars().next() {
            Some('-') | Some('d') => {}
            _ => {
                return Err(anyhow::anyhow!(
                    "refusing to extract probe-lean archive: unsupported entry type in {line:?}"
                )
                .into());
            }
        }
    }

    let extract_dir = tmpdir.path().join("extracted");
    std::fs::create_dir_all(&extract_dir).context("create extraction dir")?;
    let status = Command::new("tar")
        .arg("-xzf")
        .arg(&archive_path)
        .arg("-C")
        .arg(&extract_dir)
        .status()
        .context("spawn tar to extract probe-lean archive")?;
    if !status.success() {
        return Err(LeanInstallError::SubprocessFailed {
            command: "tar -xzf".to_string(),
            code: status.code().unwrap_or(-1),
        });
    }

    let downloaded_bin = extract_dir.join("bin/probe-lean");
    match std::fs::symlink_metadata(&downloaded_bin) {
        Ok(meta) if meta.file_type().is_symlink() => {
            return Err(anyhow::anyhow!(
                "refusing to install {}: archive's bin/probe-lean is a symlink",
                downloaded_bin.display()
            )
            .into());
        }
        Ok(_) => {}
        Err(_) => {
            return Err(LeanInstallError::MissingOutput {
                command: format!("extract {artifact}"),
                path: downloaded_bin,
            });
        }
    }

    // Publish the lib dir before the binary: a concurrent reader treats the
    // binary's existence as "ready to run," so anything it depends on must
    // already be in place first.
    let downloaded_lib = extract_dir.join("lib");
    if downloaded_lib.exists() {
        let versioned_lib = home_dir()?.join(format!(".local/lib/probe-lean-{lean_version}"));
        atomic_install_dir(&downloaded_lib, &versioned_lib)
            .with_context(|| format!("install lib dir for probe-lean-{lean_version}"))?;
    }

    let versioned_bin = home_dir()?
        .join(".local/bin")
        .join(format!("probe-lean-{lean_version}"));
    atomic_install(&downloaded_bin, &versioned_bin)
        .with_context(|| format!("install probe-lean-{lean_version} binary"))?;

    eprintln!("  Installed pre-built probe-lean-{lean_version}");
    Ok(versioned_bin)
}

/// Build `probe-lean` from source for a specific Lean version.
///
/// Clones `probe-lean`'s default branch — there is no tagged release
/// matching `lean_version` at this point (checked by the caller) — so the
/// resulting binary is not reproducible across time. Callers that need a
/// releases-only guarantee (see `PROBE_LEANBLUEPRINT_RELEASES_ONLY`) must
/// not reach this function; see also the upstream tracking issue,
/// <https://github.com/Beneficial-AI-Foundation/probe-aeneas/issues/46>.
fn build_from_source(lean_version: &str) -> Result<PathBuf> {
    eprintln!("Building probe-lean from source for Lean {lean_version}...");

    let build_tmp = tempfile::tempdir().context("create temp dir for probe-lean build")?;
    let build_dir = build_tmp.path().join("probe-lean");

    let status = Command::new("git")
        .args(["clone", "--depth", "1", PROBE_LEAN_GIT])
        .arg(&build_dir)
        .status()
        .context("spawn git clone for probe-lean")?;
    if !status.success() {
        return Err(LeanInstallError::SubprocessFailed {
            command: "git clone probe-lean".to_string(),
            code: status.code().unwrap_or(-1),
        });
    }

    if lean_version != "latest" {
        let toolchain_content = format!("leanprover/lean4:{lean_version}\n");
        std::fs::write(build_dir.join("lean-toolchain"), toolchain_content)
            .context("write lean-toolchain pin")?;
        // Force lake to re-resolve dependencies against the pinned version
        // rather than reusing a manifest/lockfile built for a different one.
        // A stale manifest/`.lake` left behind by a failed removal (anything
        // but "didn't exist") could otherwise make `lake build` silently
        // reuse artifacts for the wrong Lean version.
        if let Err(e) = std::fs::remove_file(build_dir.join("lake-manifest.json")) {
            if e.kind() != std::io::ErrorKind::NotFound {
                return Err(anyhow::Error::new(e)
                    .context("remove stale lake-manifest.json")
                    .into());
            }
        }
        if let Err(e) = std::fs::remove_dir_all(build_dir.join(".lake")) {
            if e.kind() != std::io::ErrorKind::NotFound {
                return Err(anyhow::Error::new(e)
                    .context("remove stale .lake dir")
                    .into());
            }
        }
    }

    let output = Command::new("lake")
        .arg("build")
        .current_dir(&build_dir)
        .output()
        .context("spawn lake build")?;
    if !output.status.success() {
        return Err(LeanInstallError::LakeBuildFailed {
            code: output.status.code().unwrap_or(-1),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }

    let built_bin = build_dir.join(".lake/build/bin/probe-lean");
    if !built_bin.exists() {
        return Err(LeanInstallError::MissingOutput {
            command: "lake build".to_string(),
            path: built_bin,
        });
    }

    let dest_dir = home_dir()?.join(".local/bin");
    let (dest_bin, label) = if lean_version != "latest" {
        let versioned = dest_dir.join(format!("probe-lean-{lean_version}"));
        (versioned, format!("probe-lean-{lean_version}"))
    } else {
        (dest_dir.join("probe-lean"), "probe-lean".to_string())
    };
    atomic_install(&built_bin, &dest_bin).with_context(|| format!("install {label}"))?;

    eprintln!("  Installed {label} to {}", dest_bin.display());
    Ok(dest_bin)
}

/// Recursively copy directory contents from `src` to `dst`.
///
/// Rejects symlinks outright rather than following them: `Path::is_dir()`
/// and `fs::copy()` both follow symlinks, so a malicious archive could
/// otherwise smuggle a symlink pointing outside the extraction dir past the
/// string-based `..`/absolute-path guard already applied to archive entry
/// names (which only inspects names, not link targets).
fn copy_dir_contents(src: &Path, dst: &Path) -> Result<()> {
    let entries = std::fs::read_dir(src).with_context(|| format!("read dir {}", src.display()))?;
    for entry in entries {
        let entry = entry.with_context(|| format!("read entry in {}", src.display()))?;
        let file_type = entry
            .file_type()
            .with_context(|| format!("stat {}", entry.path().display()))?;
        let src_path = entry.path();
        if file_type.is_symlink() {
            return Err(anyhow::anyhow!(
                "refusing to install {}: archive contains a symlink",
                src_path.display()
            )
            .into());
        }
        let dst_path = dst.join(entry.file_name());
        if file_type.is_dir() {
            std::fs::create_dir_all(&dst_path)
                .with_context(|| format!("create dir {}", dst_path.display()))?;
            copy_dir_contents(&src_path, &dst_path)?;
        } else {
            std::fs::copy(&src_path, &dst_path).with_context(|| {
                format!("copy {} to {}", src_path.display(), dst_path.display())
            })?;
        }
    }
    Ok(())
}

/// Install `src` as an executable at `dest`, publishing it atomically: the
/// file is written to a temp path in `dest`'s directory and only renamed
/// into place once complete, so a concurrent `find_or_install_probe_lean`
/// (checking `dest.exists()`) never observes a partially-written binary.
fn atomic_install(src: &Path, dest: &Path) -> Result<()> {
    let dest_dir = dest
        .parent()
        .with_context(|| format!("{} has no parent directory", dest.display()))?;
    std::fs::create_dir_all(dest_dir).with_context(|| format!("create {}", dest_dir.display()))?;

    let mut tmp = tempfile::Builder::new()
        .prefix(".probe-lean-install-")
        .tempfile_in(dest_dir)
        .with_context(|| format!("create temp file in {}", dest_dir.display()))?;
    let mut src_file =
        std::fs::File::open(src).with_context(|| format!("open {}", src.display()))?;
    std::io::copy(&mut src_file, tmp.as_file_mut())
        .with_context(|| format!("copy {} to {}", src.display(), dest.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tmp.as_file()
            .set_permissions(std::fs::Permissions::from_mode(0o755))
            .with_context(|| format!("set +x on {}", dest.display()))?;
    }

    tmp.persist(dest)
        .map_err(|e| anyhow::Error::new(e.error).context(format!("install {}", dest.display())))?;
    Ok(())
}

/// Install the directory `src` at `dest`, publishing it atomically: contents
/// are copied into a temp directory alongside `dest` and only renamed into
/// place once complete, so a concurrent reader never observes a partially
/// populated lib dir.
///
/// Never removes an existing `dest`: because `dest` is only ever reached via
/// this atomic rename, an existing `dest` is always a complete, previously
/// published install (by this process or a concurrent one) rather than
/// partial leftovers — and a concurrent installer for the same version may
/// already be relying on it via the (also atomically published) binary that
/// depends on it. Deleting it out from under that installer, even briefly,
/// would be a race of its own.
fn atomic_install_dir(src: &Path, dest: &Path) -> Result<()> {
    if dest.exists() {
        return Ok(());
    }

    let dest_parent = dest
        .parent()
        .with_context(|| format!("{} has no parent directory", dest.display()))?;
    std::fs::create_dir_all(dest_parent)
        .with_context(|| format!("create {}", dest_parent.display()))?;

    let tmp_dir = tempfile::Builder::new()
        .prefix(".probe-lean-lib-install-")
        .tempdir_in(dest_parent)
        .with_context(|| format!("create temp dir in {}", dest_parent.display()))?;
    copy_dir_contents(src, tmp_dir.path())?;

    match std::fs::rename(tmp_dir.path(), dest) {
        Ok(()) => {
            // `tmp_dir` no longer exists at its original path — renamed
            // away — so let it go without trying (and failing) to remove it
            // on drop.
            std::mem::forget(tmp_dir);
        }
        // Lost a race with a concurrent installer that published `dest`
        // first — that's fine, it's published either way.
        Err(_) if dest.exists() => {}
        Err(e) => {
            return Err(anyhow::Error::new(e)
                .context(format!("publish {}", dest.display()))
                .into())
        }
    }
    Ok(())
}

fn find_on_path(name: &str) -> Option<PathBuf> {
    which::which(name).ok()
}

fn home_dir() -> Result<PathBuf> {
    dirs::home_dir().ok_or(LeanInstallError::HomeDirUnavailable)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_lean_version_reads_leanprover_format() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("lean-toolchain"),
            "leanprover/lean4:v4.9.0\n",
        )
        .unwrap();
        assert_eq!(
            detect_lean_version(dir.path()).unwrap(),
            Some("v4.9.0".to_string())
        );
    }

    #[test]
    fn detect_lean_version_falls_back_to_whole_line_without_colon() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("lean-toolchain"), "v4.9.0\n").unwrap();
        assert_eq!(
            detect_lean_version(dir.path()).unwrap(),
            Some("v4.9.0".to_string())
        );
    }

    #[test]
    fn detect_lean_version_tolerates_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(detect_lean_version(dir.path()).unwrap(), None);
    }

    #[test]
    fn detect_lean_version_rejects_empty_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("lean-toolchain"), "  \n").unwrap();
        assert!(matches!(
            detect_lean_version(dir.path()),
            Err(LeanInstallError::LeanToolchainInvalid(_))
        ));
    }

    #[test]
    fn detect_lean_version_rejects_path_traversal() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("lean-toolchain"),
            "leanprover/lean4:../../etc/evil\n",
        )
        .unwrap();
        assert!(matches!(
            detect_lean_version(dir.path()),
            Err(LeanInstallError::LeanToolchainInvalid(_))
        ));
    }

    #[test]
    fn detect_lean_version_rejects_embedded_path_separator() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("lean-toolchain"), "v4.9.0/evil\n").unwrap();
        assert!(matches!(
            detect_lean_version(dir.path()),
            Err(LeanInstallError::LeanToolchainInvalid(_))
        ));
    }

    #[test]
    fn detect_lean_version_rejects_bare_dotdot() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("lean-toolchain"), "..\n").unwrap();
        assert!(matches!(
            detect_lean_version(dir.path()),
            Err(LeanInstallError::LeanToolchainInvalid(_))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn detect_lean_version_rejects_unreadable_file() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("lean-toolchain");
        std::fs::write(&path, "leanprover/lean4:v4.9.0\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();

        // Running as root (some CI/container setups) ignores file
        // permissions entirely, which would make the assertion below
        // meaningless rather than wrong — probe first to detect that case.
        let permissions_enforced = std::fs::read_to_string(&path).is_err();
        let result = detect_lean_version(dir.path());

        // Restore permissions so the tempdir can be cleaned up.
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

        if permissions_enforced {
            assert!(matches!(result, Err(LeanInstallError::Other(_))));
        }
    }
}
