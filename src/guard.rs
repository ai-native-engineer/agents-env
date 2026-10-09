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
use std::process::{Command, Output};

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
        && cwd_canon.starts_with(&gdir_canon)
    {
        return Err("refusing to write env files inside the global store's directory".to_string());
    }
    if let Ok(cfg_canon) = config::config_dir().canonicalize()
        && cwd_canon.starts_with(&cfg_canon)
    {
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
    use std::os::unix::fs::OpenOptionsExt;

    // Validate the opened inode, rather than a pathname that can be swapped
    // before fs::copy. Nonblocking avoids hanging if the target became a FIFO.
    let mut source = match fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(target)
    {
        Ok(source) => source,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    regular_single_link(&source.metadata()?, "backup source")?;
    let bak = backup_path(target)?;
    match fs::symlink_metadata(&bak) {
        Ok(md) => {
            regular_single_link(&md, "existing backup")?;
            return Ok(Some(bak)); // first-wins, without following any links
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }

    let mut destination = match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&bak)
    {
        Ok(destination) => destination,
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
            regular_single_link(&fs::symlink_metadata(&bak)?, "existing backup")?;
            return Ok(Some(bak));
        }
        Err(e) => return Err(e),
    };
    let result = io::copy(&mut source, &mut destination).and_then(|_| destination.sync_all());
    drop(destination);
    if let Err(e) = result {
        // An incomplete backup must not become tomorrow's recovery point or
        // prevent a retry today under the first-wins rule.
        let _ = fs::remove_file(&bak);
        return Err(e);
    }
    Ok(Some(bak))
}

fn backup_path(target: &Path) -> io::Result<PathBuf> {
    let name = target
        .file_name()
        .ok_or_else(|| io::Error::other("backup target has no file name"))?
        .to_string_lossy();
    Ok(target.with_file_name(format!("{}.{}.bak", name, yymmdd())))
}

fn regular_single_link(md: &fs::Metadata, description: &str) -> io::Result<()> {
    use std::os::unix::fs::MetadataExt;
    if !md.is_file() || md.nlink() != 1 {
        return Err(io::Error::other(format!(
            "{description} must be a regular file with exactly one hard link"
        )));
    }
    Ok(())
}

/// Check the git protection of the backup before a secret-bearing write.
/// A new target has no previous contents to back up.
pub fn git_backup_check(target: &Path) -> Result<(), String> {
    match fs::symlink_metadata(target) {
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(format!("cannot inspect backup source: {e}")),
    }
    let bak = backup_path(target).map_err(|e| e.to_string())?;
    let cwd = target
        .parent()
        .ok_or_else(|| "backup target has no parent directory".to_string())?;
    let name = bak
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "backup file name is not valid UTF-8".to_string())?;
    git_secret_check(cwd, name)
}

/// Atomic write: O_EXCL+O_NOFOLLOW temp file (0600) in the same directory,
/// then rename over the target.
pub fn atomic_write(target: &Path, contents: &str) -> io::Result<()> {
    let dir = target.parent().expect("target has a parent");
    let fname = target.file_name().unwrap().to_string_lossy();
    let tmp = dir.join(format!(".{}.agents-env.{}.tmp", fname, std::process::id()));
    use std::os::unix::fs::OpenOptionsExt;
    let mut fh = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&tmp)?;
    let result = fh
        .write_all(contents.as_bytes())
        .and_then(|_| fh.sync_all());
    drop(fh);
    match result.and_then(|_| fs::rename(&tmp, target)) {
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
    let git = |args: &[&str]| -> Result<Output, String> {
        Command::new("git")
            .args(args)
            .current_dir(cwd)
            .env("LC_ALL", "C")
            .output()
            .map_err(|e| format!("cannot verify git protection: {e}"))
    };
    let repository = git(&["rev-parse", "--is-inside-work-tree"])?;
    if !repository.status.success() {
        // Exit 128 also covers inaccessible or corrupt repositories. Only
        // git's explicit no-repository result, with no .git ancestor or
        // explicit repository override, means this is an ordinary directory.
        let stderr = String::from_utf8_lossy(&repository.stderr);
        let no_repository = repository.status.code() == Some(128)
            && (stderr.starts_with(
                "fatal: not a git repository (or any of the parent directories): .git\n",
            ) || (stderr
                .starts_with("fatal: not a git repository (or any parent up to mount point ")
                && stderr.contains(
                    "\nStopping at filesystem boundary (GIT_DISCOVERY_ACROSS_FILESYSTEM not set).",
                )));
        if no_repository
            && std::env::var_os("GIT_DIR").is_none()
            && std::env::var_os("GIT_WORK_TREE").is_none()
            && !has_git_ancestor(cwd)?
        {
            return Ok(());
        }
        return Err(format!(
            "cannot verify git repository status ({}) — refusing to write secrets",
            repository.status
        ));
    }
    if repository.stdout != b"true\n" {
        return Err("git did not confirm a working tree — refusing to write secrets".to_string());
    }
    let tracked = git(&["--literal-pathspecs", "ls-files", "--cached", "--", name])?;
    if !tracked.status.success() {
        return Err(format!(
            "cannot verify git tracked files ({}) — refusing to write secrets",
            tracked.status
        ));
    }
    if !tracked.stdout.is_empty() {
        return Err(format!(
            "{name} is tracked by git — refusing to write secrets into it.\n  fix: git rm --cached {name} && echo '.env*' >> .gitignore"
        ));
    }
    let ignored = git(&["check-ignore", "-q", "--", name])?;
    if ignored.status.code() == Some(1) {
        return Err(format!(
            "{name} is not gitignored — refusing to write secrets into it.\n  fix: echo '.env*' >> .gitignore  (this also covers the .bak backups)"
        ));
    }
    if !ignored.status.success() {
        return Err(format!(
            "cannot verify git ignore rules ({}) — refusing to write secrets",
            ignored.status
        ));
    }
    Ok(())
}

fn has_git_ancestor(cwd: &Path) -> Result<bool, String> {
    let cwd = cwd
        .canonicalize()
        .map_err(|e| format!("cannot resolve current directory for git protection: {e}"))?;
    for ancestor in cwd.ancestors() {
        match fs::symlink_metadata(ancestor.join(".git")) {
            Ok(_) => return Ok(true),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("cannot inspect git repository ancestor: {e}")),
        }
    }
    Ok(false)
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
        "sk-",
        "sk_live",
        "pk_live",
        "ghp_",
        "gho_",
        "github_pat_",
        "xoxb-",
        "xoxp-",
        "AIza",
        "AKIA",
        "tvly-",
        "whsec_",
        "glpat-",
        "ntn_",
        "secret_",
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
