//! Write-side guards.
//!
//! `set`/`copy` can only ever write bare `.env*` files in the current
//! directory. The global store is unreachable by construction (no flag points
//! at it, no path separators are accepted) and the remaining bypass routes —
//! symlinks, hard links, cwd being the store's own directory — are each
//! rejected explicitly. Writes are atomic (temp file + rename) and preceded by
//! a once-per-day backup.

use std::fs;
use std::io;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::config;

/// A local env file name must be `.env` or `.env.<something>`, with no path
/// separators, and never a backup file.
pub fn validate_name(name: &str) -> Result<(), String> {
    if name.contains('/') || name.contains('\\') || name.contains('\0') {
        return Err(format!(
            "'{name}': file name must be a bare name in the current directory (no path separators)"
        ));
    }
    if !(name == ".env" || name.starts_with(".env.")) {
        return Err(format!(
            "'{name}': file name must be .env or .env.<suffix> (e.g. .env.local)"
        ));
    }
    if name.ends_with(".bak") {
        return Err(format!("'{name}': refusing to write to a backup file"));
    }
    Ok(())
}

/// Validate the write target and return its absolute path.
pub fn check_write_allowed(cwd: &Path, name: &str) -> Result<PathBuf, String> {
    validate_name(name)?;
    let target = cwd.join(name);
    let global = config::global_store();

    if let Ok(md) = fs::symlink_metadata(&target) {
        if md.file_type().is_symlink() {
            return Err(format!(
                "{name} is a symlink — refusing to write through it"
            ));
        }
        use std::os::unix::fs::MetadataExt;
        if md.nlink() > 1 {
            return Err(format!(
                "{name} has {} hard links — refusing to write (it may alias another file)",
                md.nlink()
            ));
        }
        if same_file::is_same_file(&target, &global).unwrap_or(false) {
            return Err(
                "target is the global store — it is read-only for this tool; humans edit it with `agents-env edit`"
                    .to_string(),
            );
        }
    }

    let cwd_canon = cwd
        .canonicalize()
        .map_err(|e| format!("cannot resolve current directory: {e}"))?;
    if let Some(gdir) = global.parent()
        && let Ok(gdir_canon) = gdir.canonicalize()
            && cwd_canon == gdir_canon {
                return Err(
                    "refusing to write env files inside the global store's directory".to_string(),
                );
            }
    if let Ok(cfg_canon) = config::config_dir().canonicalize()
        && cwd_canon == cfg_canon {
            return Err(
                "refusing to write env files inside the agents-env config directory".to_string(),
            );
        }
    Ok(target)
}

/// Copy `target` to `<name>.YYMMDD.bak` (0600) before the first modification
/// of the day. Later writes on the same day keep that first backup — the
/// state at the start of the day is the recovery point that matters.
pub fn backup(target: &Path) -> io::Result<Option<PathBuf>> {
    // Use symlink_metadata, not exists(): exists() follows symlinks, and an
    // attacker may have replaced the (previously absent) target with a symlink
    // to the global store between check_write_allowed and now (TOCTOU). Never
    // copy through a symlink — that would leak the link's target into a .bak.
    match fs::symlink_metadata(target) {
        Ok(md) if md.file_type().is_symlink() => {
            return Err(io::Error::other(
                "target became a symlink before backup — refusing to follow it",
            ));
        }
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    }
    let name = target.file_name().unwrap().to_string_lossy();
    let bak = target.with_file_name(format!("{}.{}.bak", name, yymmdd()));
    if bak.exists() {
        return Ok(Some(bak)); // first-wins
    }
    fs::copy(target, &bak)?;
    set_mode_0600(&bak)?;
    Ok(Some(bak))
}

/// Atomic write: O_EXCL+O_NOFOLLOW temp file (0600) in the same directory,
/// then rename over the target.
pub fn atomic_write(target: &Path, contents: &str) -> io::Result<()> {
    let dir = target.parent().expect("target has a parent");
    let fname = target.file_name().unwrap().to_string_lossy();
    let tmp = dir.join(format!(".{}.agents-env.{}.tmp", fname, std::process::id()));
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut fh = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&tmp)?;
        fh.write_all(contents.as_bytes())?;
        fh.sync_all()?;
    }
    match fs::rename(&tmp, target) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = fs::remove_file(&tmp);
            Err(e)
        }
    }
}

/// Secrets may only be written to files git will never pick up: inside a
/// repo the target must be untracked AND ignored. No override flag — fixing
/// .gitignore is the correct resolution.
pub fn git_secret_check(cwd: &Path, name: &str) -> Result<(), String> {
    let git = |args: &[&str]| {
        Command::new("git")
            .args(args)
            .current_dir(cwd)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    };
    if !git(&["rev-parse", "--is-inside-work-tree"]) {
        return Ok(());
    }
    if git(&["ls-files", "--error-unmatch", name]) {
        return Err(format!(
            "{name} is tracked by git — refusing to write secrets into it.\n  fix: git rm --cached {name} && echo '.env*' >> .gitignore"
        ));
    }
    if !git(&["check-ignore", "-q", name]) {
        return Err(format!(
            "{name} is not gitignored — refusing to write secrets into it.\n  fix: echo '.env*' >> .gitignore  (this also covers the .bak backups)"
        ));
    }
    Ok(())
}

pub fn set_mode_0600(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = fs::metadata(path)?.permissions();
    perms.set_mode(0o600);
    fs::set_permissions(path, perms)
}

/// Local date as YYMMDD (e.g. 260610).
pub fn yymmdd() -> String {
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&t, &mut tm) };
    format!(
        "{:02}{:02}{:02}",
        (tm.tm_year + 1900) % 100,
        tm.tm_mon + 1,
        tm.tm_mday
    )
}

/// Heuristic: does a literal value look like a credential? Used as a tripwire
/// warning on `set` — an agent typing a real secret literal means the value
/// is already in its context, which `copy` exists to avoid.
pub fn looks_like_secret(v: &str) -> bool {
    const PREFIXES: &[&str] = &[
        "sk-", "sk_live", "pk_live", "ghp_", "gho_", "github_pat_", "xoxb-", "xoxp-", "AIza",
        "AKIA", "tvly-", "whsec_", "glpat-", "ntn_", "secret_",
    ];
    if PREFIXES.iter().any(|p| v.starts_with(p)) {
        return true;
    }
    if v.len() >= 32 && !v.contains(' ') {
        let has_upper = v.chars().any(|c| c.is_ascii_uppercase());
        let has_lower = v.chars().any(|c| c.is_ascii_lowercase());
        let has_digit = v.chars().any(|c| c.is_ascii_digit());
        return has_upper && has_lower && has_digit;
    }
    false
}
