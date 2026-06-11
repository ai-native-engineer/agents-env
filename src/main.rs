//! agents-env — env var manager that lets AI agents use secrets without ever
//! seeing them.

mod aimode;
mod config;
mod guard;
mod mask;
mod store;

use clap::{Parser, Subcommand};
use std::path::{Path, PathBuf};
use store::{EnvFile, SelectError};

const AGENT_GUIDE: &str = "\
AGENT MODE (auto-detected via CLAUDECODE / CLAUDE_CODE_ENTRYPOINT / AI_AGENT / CODEX_SANDBOX):
  secret values are never printed — work with key names only.

  agents-env get TAVILY                       discover keys (values stay hidden)
  agents-env run KEY@tag -- cmd -H 'X: {{KEY}}'
                                              value goes only to the child process;
                                              child output is masked in real time
  agents-env copy KEY@tag --to .env.local     global store -> local file, value
                                              never passes through your context
  agents-env set PORT 3000 --to .env.local    non-secret literals only

  KEY@tag picks one of several accounts for the same key (tags come from the
  inline '# comment' in the env file). The global store is read-only by
  design: no flag of set/copy can reach it. Humans edit it with `agents-env edit`.
";

#[derive(Parser)]
#[command(
    name = "agents-env",
    version,
    about = "Env vars for AI agents — use secrets without ever seeing them",
    after_help = AGENT_GUIDE
)]
struct Cli {
    /// Read from the local scope (./.env) instead of the global store
    #[arg(short = 'l', long, global = true)]
    local: bool,

    /// Local env file name, e.g. .env.local (implies --local)
    #[arg(short = 'f', long, global = true, value_name = "NAME")]
    file: Option<String>,

    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Look up keys by pattern (case-insensitive substring on key names)
    Get {
        #[arg(value_name = "PATTERN")]
        pattern: String,
    },
    /// List key names and tags — never prints values, in any mode
    Ls {
        #[arg(value_name = "PATTERN")]
        pattern: Option<String>,
    },
    /// Run a command with secrets injected; child output is masked
    Run {
        /// KEY or KEY@tag selectors to inject
        #[arg(value_name = "KEY[@tag]")]
        selectors: Vec<String>,
        /// Inject every key in the selected scope
        #[arg(long)]
        all: bool,
        /// Disable output masking (refused in agent mode)
        #[arg(long)]
        no_mask: bool,
        /// Command to execute, after `--`. `{{KEY}}` in arguments is replaced
        /// with the injected value at exec time.
        #[arg(last = true, required = true, value_name = "COMMAND")]
        command: Vec<String>,
    },
    /// Write a non-secret literal into a local env file (backs up first)
    Set {
        key: String,
        value: String,
        /// Target file name in the current directory
        #[arg(long, default_value = ".env", value_name = "NAME")]
        to: String,
    },
    /// Copy secrets from the global store into a local env file — the value
    /// never appears in any output
    Copy {
        #[arg(value_name = "KEY[@tag]", required = true)]
        selectors: Vec<String>,
        /// Target file name in the current directory
        #[arg(long, default_value = ".env", value_name = "NAME")]
        to: String,
        /// Write under a different key name (single selector only)
        #[arg(long = "as", value_name = "NEWKEY")]
        rename: Option<String>,
    },
    /// Open the global store in $EDITOR (humans only — refused in agent mode)
    Edit,
    /// Audit protection coverage: permissions, gitignore, backups, dup keys
    Doctor,
}

fn main() {
    let cli = Cli::parse();
    let code = match &cli.cmd {
        Cmd::Get { pattern } => cmd_get(&cli, pattern),
        Cmd::Ls { pattern } => cmd_ls(&cli, pattern.as_deref()),
        Cmd::Run {
            selectors,
            all,
            no_mask,
            command,
        } => cmd_run(&cli, selectors, *all, *no_mask, command),
        Cmd::Set { key, value, to } => cmd_set(key, value, to),
        Cmd::Copy {
            selectors,
            to,
            rename,
        } => cmd_copy(selectors, to, rename.as_deref()),
        Cmd::Edit => cmd_edit(),
        Cmd::Doctor => cmd_doctor(),
    };
    std::process::exit(code);
}

// ---------------------------------------------------------------- scope

/// Resolve the read scope: global store by default, `./<file>` with -l/-f.
fn scope_path(cli: &Cli) -> Result<(PathBuf, bool), String> {
    if cli.local || cli.file.is_some() {
        let name = cli.file.clone().unwrap_or_else(|| ".env".to_string());
        guard::validate_name(&name)?;
        let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
        Ok((cwd.join(name), true))
    } else {
        Ok((config::global_store(), false))
    }
}

fn load_scope(cli: &Cli) -> Result<(EnvFile, bool), (i32, String)> {
    let (path, is_local) = scope_path(cli).map_err(|m| (3, m))?;
    match EnvFile::load(&path) {
        Ok(f) => Ok((f, is_local)),
        Err(e) => Err((
            1,
            format!(
                "cannot read {}: {e}{}",
                path.display(),
                if !is_local {
                    "\n  (global store missing? humans can create it with `agents-env edit`)"
                } else {
                    ""
                }
            ),
        )),
    }
}

fn fail(code: i32, msg: &str) -> i32 {
    eprintln!("agents-env: {msg}");
    code
}

// ---------------------------------------------------------------- get / ls

const BLUE: &str = "\x1b[0;34m";
const GREEN: &str = "\x1b[0;32m";
const YELLOW: &str = "\x1b[1;33m";
const RESET: &str = "\x1b[0m";

fn cmd_get(cli: &Cli, pattern: &str) -> i32 {
    let (f, _) = match load_scope(cli) {
        Ok(v) => v,
        Err((c, m)) => return fail(c, &m),
    };
    let matches = f.search(pattern);
    if matches.is_empty() {
        return fail(
            1,
            &format!("no key matching '{pattern}' in {}", f.path.display()),
        );
    }
    if aimode::agent_mode() {
        for (_, e) in &matches {
            let tag = e.comment.as_deref().unwrap_or("");
            println!(
                "{}  [set, {} chars]  {}",
                e.key,
                e.value.chars().count(),
                tag
            );
        }
        let example = selector_example(&f, &matches);
        println!("--");
        println!("agent mode: values are hidden. use them without seeing them:");
        println!(
            "  agents-env run {example} -- <command using {{{{{}}}}}>",
            matches[0].1.key
        );
        println!("  agents-env copy {example} --to .env.local");
    } else {
        let tty = unsafe { libc::isatty(1) == 1 };
        for (i, e) in &matches {
            let value_raw = f.value_raw(*i);
            let tag = e
                .comment
                .as_deref()
                .map(|c| format!(" {c}"))
                .unwrap_or_default();
            if tty {
                println!("{BLUE}{}{RESET}={GREEN}{}{}{RESET}", e.key, value_raw, tag);
            } else {
                println!("{}={}{}", e.key, value_raw, tag);
            }
        }
    }
    0
}

/// A concrete selector for the first matched key: `KEY@tag` when the key is
/// duplicated and has a tag, plain `KEY` otherwise.
fn selector_example(f: &EnvFile, matches: &[(usize, &store::Entry)]) -> String {
    let e = matches[0].1;
    let dups = f.occurrences(&e.key).len();
    if dups > 1
        && let Some(c) = &e.comment {
            let tag = c.trim_start_matches('#').trim();
            // Leading run of identifier-ish chars: "senugw0u@gmail.com" -> "senugw0u",
            // "jax contact" -> "jax". Enough to disambiguate without an ugly @-in-@.
            let token: String = tag
                .chars()
                .take_while(|c| c.is_alphanumeric() || matches!(c, '.' | '_' | '-'))
                .collect();
            if !token.is_empty() {
                return format!("{}@{}", e.key, token);
            }
        }
    e.key.clone()
}

fn cmd_ls(cli: &Cli, pattern: Option<&str>) -> i32 {
    let (f, _) = match load_scope(cli) {
        Ok(v) => v,
        Err((c, m)) => return fail(c, &m),
    };
    let matches = f.search(pattern.unwrap_or(""));
    if matches.is_empty() {
        return fail(1, &format!("no keys in {}", f.path.display()));
    }
    for (_, e) in matches {
        let tag = e.comment.as_deref().unwrap_or("");
        println!("{}  {}", e.key, tag);
    }
    0
}

// ---------------------------------------------------------------- run

fn cmd_run(cli: &Cli, selectors: &[String], all: bool, no_mask: bool, command: &[String]) -> i32 {
    let agent = aimode::agent_mode();
    if no_mask && agent {
        return fail(2, "--no-mask is not allowed in agent mode");
    }
    if command.is_empty() {
        return fail(
            3,
            "no command given — usage: agents-env run KEY[@tag]... -- <command>",
        );
    }
    if all && !selectors.is_empty() {
        return fail(3, "use either selectors or --all, not both");
    }
    if !all && selectors.is_empty() {
        return fail(3, "specify KEY[@tag] selectors or --all");
    }

    let (f, is_local) = match load_scope(cli) {
        Ok(v) => v,
        Err((c, m)) => return fail(c, &m),
    };

    let mut inject: Vec<(String, String)> = Vec::new();
    if all {
        // last-wins for duplicate keys, with a warning naming the skipped entry
        for (_, e) in f.entries() {
            if let Some(pos) = inject.iter().position(|(k, _)| k == &e.key) {
                eprintln!(
                    "agents-env: warning: duplicate key {} — last occurrence wins ({})",
                    e.key,
                    e.comment.as_deref().unwrap_or("(no tag)")
                );
                inject[pos] = (e.key.clone(), e.value.clone());
            } else {
                inject.push((e.key.clone(), e.value.clone()));
            }
        }
        if inject.is_empty() {
            return fail(1, &format!("no keys in {}", f.path.display()));
        }
    } else {
        for sel in selectors {
            match f.select(sel) {
                Ok((_, e)) => inject.push((e.key.clone(), e.value.clone())),
                Err(err) => return fail(2, &select_error_message(&err)),
            }
        }
    }

    // Mask set: injected values (ALWAYS, any length — a leak here is the value
    // the agent asked to use) ∪ ambient values from the global store and local
    // scope (length-floored to avoid over-masking common short strings).
    let mut mask_values: Vec<(String, String)> = inject.clone();
    let mut ambient: Vec<(String, String)> = Vec::new();
    if let Ok(g) = EnvFile::load(&config::global_store()) {
        for (_, e) in g.entries() {
            ambient.push((e.key.clone(), e.value.clone()));
        }
    }
    if is_local {
        for (_, e) in f.entries() {
            ambient.push((e.key.clone(), e.value.clone()));
        }
    }
    ambient.retain(|(_, v)| v.len() >= 6);
    mask_values.extend(ambient);
    // Order-preserving dedup by value: injected entries come first, so an
    // injected short secret is never the one dropped.
    {
        let mut seen = std::collections::HashSet::new();
        mask_values.retain(|(_, v)| seen.insert(v.clone()));
    }

    mask::run(&inject, command, &mask_values, !no_mask)
}

fn select_error_message(err: &SelectError) -> String {
    match err {
        SelectError::NotFound(sel) => format!("no entry matches selector '{sel}'"),
        SelectError::Ambiguous { key, tags } => {
            let opts = tags
                .iter()
                .map(|t| format!("  {key}@{}", t.trim_start_matches('#').trim()))
                .collect::<Vec<_>>()
                .join("\n");
            format!("'{key}' has multiple entries — pick one with KEY@tag:\n{opts}")
        }
    }
}

// ---------------------------------------------------------------- set / copy

fn cmd_set(key: &str, value: &str, to: &str) -> i32 {
    if guard::looks_like_secret(value) {
        eprintln!(
            "agents-env: warning: this value looks like a credential. If an agent typed it, \
             the secret is already in its context — use `agents-env copy {key}@<tag> --to {to}` instead."
        );
    }
    let cwd = match std::env::current_dir() {
        Ok(d) => d,
        Err(e) => return fail(1, &e.to_string()),
    };
    let target = match guard::check_write_allowed(&cwd, to) {
        Ok(t) => t,
        Err(m) => return fail(2, &m),
    };
    let mut f = match EnvFile::load_or_empty(&target) {
        Ok(f) => f,
        Err(e) => return fail(1, &format!("cannot read {to}: {e}")),
    };
    let action = match upsert(&mut f, key, &store::quote_value(value), None) {
        Ok(a) => a,
        Err(m) => return fail(2, &m),
    };
    if let Err(m) = write_back(&target, &f) {
        return fail(1, &m);
    }
    println!("set {key} -> {to} [{action}]");
    0
}

fn cmd_copy(selectors: &[String], to: &str, rename: Option<&str>) -> i32 {
    if rename.is_some() && selectors.len() != 1 {
        return fail(3, "--as works with exactly one selector");
    }
    let gpath = config::global_store();
    let g = match EnvFile::load(&gpath) {
        Ok(f) => f,
        Err(e) => {
            return fail(
                1,
                &format!("cannot read global store {}: {e}", gpath.display()),
            )
        }
    };

    // Resolve everything first so a failure changes nothing.
    let mut resolved: Vec<(String, String, Option<String>)> = Vec::new(); // (write_key, value_raw, comment)
    for sel in selectors {
        match g.select(sel) {
            Ok((idx, e)) => {
                let write_key = rename.unwrap_or(&e.key).to_string();
                resolved.push((write_key, g.value_raw(idx).to_string(), e.comment.clone()));
            }
            Err(err) => return fail(2, &select_error_message(&err)),
        }
    }

    let cwd = match std::env::current_dir() {
        Ok(d) => d,
        Err(e) => return fail(1, &e.to_string()),
    };
    let target = match guard::check_write_allowed(&cwd, to) {
        Ok(t) => t,
        Err(m) => return fail(2, &m),
    };
    if let Err(m) = guard::git_secret_check(&cwd, to) {
        return fail(2, &m);
    }
    let mut f = match EnvFile::load_or_empty(&target) {
        Ok(f) => f,
        Err(e) => return fail(1, &format!("cannot read {to}: {e}")),
    };
    let mut report = Vec::new();
    for (key, value_raw, comment) in &resolved {
        match upsert(&mut f, key, value_raw, comment.as_deref()) {
            Ok(action) => report.push(format!(
                "copied {key} ({}) -> {to} [{action}]",
                comment
                    .as_deref()
                    .map(|c| c.trim_start_matches('#').trim())
                    .unwrap_or("untagged")
            )),
            Err(m) => return fail(2, &m),
        }
    }
    if let Err(m) = write_back(&target, &f) {
        return fail(1, &m);
    }
    for line in report {
        println!("{line}");
    }
    0
}

/// Update the single occurrence of `key` (preserving the line), append if new,
/// refuse if the target itself has duplicate occurrences.
fn upsert(
    f: &mut EnvFile,
    key: &str,
    value_raw: &str,
    comment: Option<&str>,
) -> Result<&'static str, String> {
    let occ = f.occurrences(key);
    match occ.len() {
        0 => {
            f.append(key, value_raw, comment);
            Ok("added")
        }
        1 => {
            f.replace_value(occ[0], value_raw);
            Ok("updated")
        }
        n => Err(format!(
            "{key} appears {n} times in the target file — resolve the duplicates manually first"
        )),
    }
}

fn write_back(target: &Path, f: &EnvFile) -> Result<(), String> {
    match guard::backup(target) {
        Ok(Some(bak)) => eprintln!(
            "agents-env: backup: {}",
            bak.file_name().unwrap().to_string_lossy()
        ),
        Ok(None) => {}
        Err(e) => return Err(format!("backup failed, aborting write: {e}")),
    }
    guard::atomic_write(target, &f.serialize()).map_err(|e| format!("write failed: {e}"))
}

// ---------------------------------------------------------------- edit

fn cmd_edit() -> i32 {
    if aimode::agent_mode() {
        return fail(
            2,
            "edit is human-only. Agents: use `copy`/`set` on local files; the global store is read-only for you.",
        );
    }
    let tty = unsafe { libc::isatty(0) == 1 && libc::isatty(1) == 1 };
    if !tty {
        return fail(2, "edit requires an interactive terminal");
    }
    let path = config::global_store();
    if let Some(dir) = path.parent()
        && let Err(e) = std::fs::create_dir_all(dir) {
            return fail(1, &format!("cannot create {}: {e}", dir.display()));
        }
    let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".to_string());
    let status = std::process::Command::new(&editor).arg(&path).status();
    match status {
        Ok(s) if s.success() => {}
        Ok(s) => return s.code().unwrap_or(1),
        Err(e) => return fail(1, &format!("cannot launch editor '{editor}': {e}")),
    }
    let _ = guard::set_mode_0600(&path);
    // Post-edit lint: keep the KEY@tag selector scheme intact. Names and line
    // numbers only — never values.
    if let Ok(f) = EnvFile::load(&path) {
        for idx in f.unparseable_lines() {
            println!("{YELLOW}lint:{RESET} line {} is not parseable", idx + 1);
        }
        let mut seen: Vec<String> = Vec::new();
        for (i, e) in f.entries() {
            if f.occurrences(&e.key).len() > 1 && e.comment.is_none() && !seen.contains(&e.key) {
                seen.push(e.key.clone());
                println!(
                    "{YELLOW}lint:{RESET} duplicate key {} (line {}) has an entry without a '# tag' comment — KEY@tag selection needs tags",
                    e.key,
                    i + 1
                );
            }
        }
    }
    0
}

// ---------------------------------------------------------------- doctor

fn cmd_doctor() -> i32 {
    use std::os::unix::fs::PermissionsExt;
    let mut warnings = 0;

    let gpath = config::global_store();
    println!("global store: {}", gpath.display());
    match std::fs::metadata(&gpath) {
        Ok(md) => {
            let mode = md.permissions().mode() & 0o777;
            if mode & 0o077 != 0 {
                warnings += 1;
                println!("  warn: permissions {mode:o} — consider chmod 600");
            }
            if let Ok(f) = EnvFile::load(&gpath) {
                let mut seen: Vec<String> = Vec::new();
                for (_, e) in f.entries() {
                    if f.occurrences(&e.key).len() > 1
                        && e.comment.is_none()
                        && !seen.contains(&e.key)
                    {
                        seen.push(e.key.clone());
                        warnings += 1;
                        println!(
                            "  warn: duplicate key {} has untagged entries (KEY@tag selection)",
                            e.key
                        );
                    }
                }
            }
        }
        Err(_) => {
            warnings += 1;
            println!(
                "  warn: does not exist — create it with `agents-env edit` or set global_store= in {}",
                config::config_path().display()
            );
        }
    }

    let cwd = std::env::current_dir().unwrap();
    let now = std::time::SystemTime::now();
    let mut found_local = false;
    if let Ok(rd) = std::fs::read_dir(&cwd) {
        for entry in rd.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if !(name == ".env" || name.starts_with(".env.")) {
                continue;
            }
            found_local = true;
            if name.ends_with(".bak") {
                if let Ok(md) = entry.metadata()
                    && let Ok(modified) = md.modified()
                        && let Ok(age) = now.duration_since(modified)
                            && age.as_secs() > 30 * 24 * 3600 {
                                warnings += 1;
                                println!("local: {name}\n  warn: backup older than 30 days — consider deleting");
                            }
                continue;
            }
            println!("local: {name}");
            if let Ok(md) = std::fs::symlink_metadata(entry.path()) {
                if md.file_type().is_symlink() {
                    warnings += 1;
                    println!("  warn: is a symlink — writes are refused");
                } else {
                    let mode = md.permissions().mode() & 0o777;
                    if mode & 0o077 != 0 {
                        warnings += 1;
                        println!("  warn: permissions {mode:o} — consider chmod 600");
                    }
                }
            }
            if let Err(m) = guard::git_secret_check(&cwd, &name) {
                warnings += 1;
                println!("  warn: {}", m.lines().next().unwrap_or(""));
            }
        }
    }
    if !found_local {
        println!("local: no env files in {}", cwd.display());
    }

    let settings = config::home().join(".claude").join("settings.json");
    if let Ok(text) = std::fs::read_to_string(&settings) {
        if text.contains("Read(**/.env") {
            println!("claude code: deny rules for env files present");
        } else {
            warnings += 1;
            println!(
                "claude code: no broad env deny rules in {} — agents can still Read env files directly.\n  suggested deny: \"Read(**/.env)\", \"Read(**/.env.*)\"",
                settings.display()
            );
        }
    }

    if warnings == 0 {
        println!("ok: no warnings");
        0
    } else {
        println!("{warnings} warning(s)");
        1
    }
}
