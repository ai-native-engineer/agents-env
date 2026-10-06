//! agents-env — env var manager that lets AI agents use secrets without ever
//! seeing them.

mod aimode;
#[allow(dead_code)] // backend contract is introduced before CLI wiring in the next goal rows.
mod backend;
mod config;
mod guard;
mod mask;
mod store;

use backend::{FileStore, SecretMetadata, SecretStore, StoreError};
use clap::{Parser, Subcommand};
use std::path::{Path, PathBuf};
use store::{EnvFile, SelectError};

const AGENT_GUIDE: &str = "\
AGENT MODE (auto on Unix: Grok, Codex, OpenCode, Claude Code, AGY; opt-in: AGENTS_ENV_AGENT_MODE / markers=):
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

  The global backend is file by default. On macOS, `backend=keychain` opts into
  metadata-only discovery and in-memory Keychain reads; `copy` still exports a
  plaintext local file. Use `migrate --to-keychain --dry-run` before migration.

  Other assistants and renamed/deeply wrapped CLIs are supported by explicit
  opt-in: launch them with AGENTS_ENV_AGENT_MODE=1, or add a marker you set
  via `markers=MY_AGENT_MODE` in ~/.config/agents-env/config.
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
    /// Import the file-backed global store into macOS Keychain explicitly
    Migrate {
        /// Migrate the global file into the macOS Keychain backend
        #[arg(long)]
        to_keychain: bool,
        /// Show the metadata-only migration plan without writing Keychain items
        #[arg(long)]
        dry_run: bool,
    },
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
        Cmd::Migrate {
            to_keychain,
            dry_run,
        } => cmd_migrate(&cli, *to_keychain, *dry_run),
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

fn load_global_backend() -> Result<Box<dyn SecretStore>, (i32, String)> {
    match config::backend().map_err(|m| (3, m))? {
        config::Backend::File => {
            let path = config::global_store();
            let file = EnvFile::load(&path).map_err(|e| {
                (
                    1,
                    format!(
                        "cannot read {}: {e}\n  (global store missing? humans can create it with `agents-env edit`)",
                        path.display()
                    ),
                )
            })?;
            Ok(Box::new(FileStore::from_env_file(file)))
        }
        config::Backend::Keychain => {
            #[cfg(target_os = "macos")]
            {
                backend::NativeKeychainStore::new()
                    .map(|store| Box::new(store) as Box<dyn SecretStore>)
                    .map_err(|error| (2, store_error_message(&error)))
            }
            #[cfg(not(target_os = "macos"))]
            {
                Err((2, "keychain backend requires macOS".to_string()))
            }
        }
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
    if !cli.local && cli.file.is_none() {
        match config::backend() {
            Ok(config::Backend::Keychain) => return cmd_get_keychain(pattern),
            Ok(config::Backend::File) => {}
            Err(m) => return fail(3, &m),
        }
    }
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

fn cmd_get_keychain(pattern: &str) -> i32 {
    let store = match load_global_backend() {
        Ok(store) => store,
        Err((code, message)) => return fail(code, &message),
    };
    let metadata = match store.metadata(Some(pattern)) {
        Ok(metadata) => metadata,
        Err(error) => return fail(2, &store_error_message(&error)),
    };
    if metadata.is_empty() {
        return fail(1, &format!("no key matching '{pattern}' in Keychain"));
    }
    print_metadata(&metadata);
    println!("--");
    println!("Keychain backend: values are hidden. use them without seeing them:");
    let example = metadata_selector_example(&metadata[0], &metadata);
    println!(
        "  agents-env run {example} -- <command using {{{{{}}}}}>",
        metadata[0].key()
    );
    println!("  agents-env copy {example} --to .env.local");
    0
}

fn print_metadata(metadata: &[SecretMetadata]) {
    for item in metadata {
        let length = item
            .value_len()
            .map(|length| format!(", {length} chars"))
            .unwrap_or_default();
        println!(
            "{}  [set{}]  {}",
            item.key(),
            length,
            item.tag().unwrap_or("")
        );
    }
}

fn metadata_selector_example(item: &SecretMetadata, all: &[SecretMetadata]) -> String {
    let duplicate = all
        .iter()
        .filter(|candidate| candidate.key() == item.key())
        .count()
        > 1;
    if duplicate && let Some(tag) = item.tag() {
        let token: String = tag
            .trim_start_matches('#')
            .trim()
            .chars()
            .take_while(|c| c.is_alphanumeric() || matches!(c, '.' | '_' | '-'))
            .collect();
        if !token.is_empty() {
            return format!("{}@{}", item.key(), token);
        }
    }
    item.key().to_string()
}

/// A concrete selector for the first matched key: `KEY@tag` when the key is
/// duplicated and has a tag, plain `KEY` otherwise.
fn selector_example(f: &EnvFile, matches: &[(usize, &store::Entry)]) -> String {
    let e = matches[0].1;
    let dups = f.occurrences(&e.key).len();
    if dups > 1
        && let Some(c) = &e.comment
    {
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
    if !cli.local && cli.file.is_none() {
        match config::backend() {
            Ok(config::Backend::Keychain) => return cmd_ls_keychain(pattern),
            Ok(config::Backend::File) => {}
            Err(m) => return fail(3, &m),
        }
    }
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

fn cmd_ls_keychain(pattern: Option<&str>) -> i32 {
    let store = match load_global_backend() {
        Ok(store) => store,
        Err((code, message)) => return fail(code, &message),
    };
    let metadata = match store.metadata(pattern) {
        Ok(metadata) => metadata,
        Err(error) => return fail(2, &store_error_message(&error)),
    };
    if metadata.is_empty() {
        return fail(1, "no keys in Keychain");
    }
    for item in metadata {
        println!("{}  {}", item.key(), item.tag().unwrap_or(""));
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

    if !cli.local && cli.file.is_none() {
        match config::backend() {
            Ok(config::Backend::Keychain) => {
                return cmd_run_keychain(selectors, all, no_mask, command);
            }
            Ok(config::Backend::File) => {}
            Err(m) => return fail(3, &m),
        }
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
    if is_local {
        match ambient_global_values() {
            Ok(values) => ambient.extend(values),
            Err(message) => return fail(2, &message),
        }
        for (_, e) in f.entries() {
            ambient.push((e.key.clone(), e.value.clone()));
        }
    } else {
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

fn ambient_global_values() -> Result<Vec<(String, String)>, String> {
    match config::backend()? {
        config::Backend::File => Ok(EnvFile::load(&config::global_store())
            .ok()
            .map(|file| {
                file.entries()
                    .map(|(_, entry)| (entry.key.clone(), entry.value.clone()))
                    .collect()
            })
            .unwrap_or_default()),
        config::Backend::Keychain => load_global_backend()
            .map_err(|(_, message)| message)
            .and_then(|store| {
                store
                    .resolve_all()
                    .map(|secrets| {
                        secrets
                            .into_iter()
                            .map(|secret| (secret.key().to_string(), secret.value().to_string()))
                            .collect()
                    })
                    .map_err(|error| store_error_message(&error))
            }),
    }
}

fn cmd_run_keychain(selectors: &[String], all: bool, no_mask: bool, command: &[String]) -> i32 {
    let store = match load_global_backend() {
        Ok(store) => store,
        Err((code, message)) => return fail(code, &message),
    };
    let resolved = if all {
        match store.resolve_all() {
            Ok(values) => values,
            Err(error) => return fail(2, &store_error_message(&error)),
        }
    } else {
        let mut values = Vec::with_capacity(selectors.len());
        for selector in selectors {
            match store.resolve(selector) {
                Ok(value) => values.push(value),
                Err(error) => return fail(2, &store_error_message(&error)),
            }
        }
        values
    };
    if resolved.is_empty() {
        return fail(1, "no keys in Keychain");
    }

    let mut inject: Vec<(String, String)> = Vec::new();
    for secret in resolved {
        if let Some(position) = inject.iter().position(|(key, _)| key == secret.key()) {
            eprintln!(
                "agents-env: warning: duplicate key {} — last occurrence wins",
                secret.key()
            );
            inject[position] = (secret.key().to_string(), secret.value().to_string());
        } else {
            inject.push((secret.key().to_string(), secret.value().to_string()));
        }
    }

    let mut mask_values = inject.clone();
    let ambient = match store.resolve_all() {
        Ok(values) => values,
        Err(error) => return fail(2, &store_error_message(&error)),
    };
    mask_values.extend(
        ambient
            .into_iter()
            .map(|secret| (secret.key().to_string(), secret.value().to_string())),
    );
    let mut seen = std::collections::HashSet::new();
    mask_values.retain(|(_, value)| seen.insert(value.clone()));
    mask::run(&inject, command, &mask_values, !no_mask)
}

fn select_error_message(err: &SelectError) -> String {
    match err {
        SelectError::NotFound(sel) => {
            format!(
                "no entry matches selector '{sel}' — run `agents-env ls {sel}` to discover key names and tags"
            )
        }
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

fn store_error_message(err: &StoreError) -> String {
    match err {
        StoreError::NotFound(selector) => {
            format!(
                "no entry matches selector '{selector}' — run `agents-env ls {selector}` to discover key names and tags"
            )
        }
        StoreError::Ambiguous { key, tags } => {
            let opts = tags
                .iter()
                .map(|tag| format!("  {key}@{}", tag.trim_start_matches('#').trim()))
                .collect::<Vec<_>>()
                .join("\n");
            format!("'{key}' has multiple entries — pick one with KEY@tag:\n{opts}")
        }
        StoreError::Unavailable(message) | StoreError::Invalid(message) => message.clone(),
    }
}

// ---------------------------------------------------------------- set / copy

fn cmd_set(key: &str, value: &str, to: &str) -> i32 {
    if let Err(m) = store::validate_key(key) {
        return fail(3, &m);
    }
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
    if let Some(rename) = rename
        && let Err(m) = store::validate_key(rename)
    {
        return fail(3, &format!("invalid --as key: {m}"));
    }
    match config::backend() {
        Ok(config::Backend::Keychain) => return cmd_copy_keychain(selectors, to, rename),
        Ok(config::Backend::File) => {}
        Err(m) => return fail(3, &m),
    }
    let gpath = config::global_store();
    let g = match EnvFile::load(&gpath) {
        Ok(f) => f,
        Err(e) => {
            return fail(
                1,
                &format!("cannot read global store {}: {e}", gpath.display()),
            );
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

fn cmd_copy_keychain(selectors: &[String], to: &str, rename: Option<&str>) -> i32 {
    let store = match load_global_backend() {
        Ok(store) => store,
        Err((code, message)) => return fail(code, &message),
    };
    let mut resolved: Vec<(String, String, Option<String>)> = Vec::new();
    for selector in selectors {
        let secret = match store.resolve(selector) {
            Ok(secret) => secret,
            Err(error) => return fail(2, &store_error_message(&error)),
        };
        let key = rename.unwrap_or(secret.key()).to_string();
        let raw = secret
            .raw_value()
            .map(str::to_string)
            .unwrap_or_else(|| store::quote_value(secret.value()));
        resolved.push((key, raw, secret.tag().map(str::to_string)));
    }

    let cwd = match std::env::current_dir() {
        Ok(directory) => directory,
        Err(error) => return fail(1, &error.to_string()),
    };
    let target = match guard::check_write_allowed(&cwd, to) {
        Ok(target) => target,
        Err(message) => return fail(2, &message),
    };
    if let Err(message) = guard::git_secret_check(&cwd, to) {
        return fail(2, &message);
    }
    let mut file = match EnvFile::load_or_empty(&target) {
        Ok(file) => file,
        Err(error) => return fail(1, &format!("cannot read {to}: {error}")),
    };
    let mut report = Vec::new();
    for (key, raw, tag) in &resolved {
        match upsert(&mut file, key, raw, tag.as_deref()) {
            Ok(action) => report.push(format!(
                "copied {key} ({}) -> {to} [{action}]",
                tag.as_deref()
                    .map(|tag| tag.trim_start_matches('#').trim())
                    .unwrap_or("untagged")
            )),
            Err(message) => return fail(2, &message),
        }
    }
    if let Err(message) = write_back(&target, &file) {
        return fail(1, &message);
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
    match config::backend() {
        Ok(config::Backend::Keychain) => {
            return fail(
                2,
                "edit is only available for the file backend; Keychain entries use an explicit setup or migration command",
            );
        }
        Ok(config::Backend::File) => {}
        Err(m) => return fail(3, &m),
    }
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
        && let Err(e) = std::fs::create_dir_all(dir)
    {
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

fn cmd_migrate(cli: &Cli, to_keychain: bool, dry_run: bool) -> i32 {
    if cli.local || cli.file.is_some() {
        return fail(
            3,
            "migrate uses the global store; remove -l/--local and -f/--file",
        );
    }
    if !to_keychain {
        return fail(3, "specify --to-keychain to choose the migration target");
    }
    let source_path = config::global_store();
    let source = match FileStore::load(&source_path) {
        Ok(source) => source,
        Err(error) => {
            return fail(
                1,
                &format!(
                    "cannot read migration source {}: {error}",
                    source_path.display()
                ),
            );
        }
    };
    let secrets = match source.resolve_all() {
        Ok(secrets) => secrets,
        Err(error) => return fail(2, &store_error_message(&error)),
    };
    if secrets.is_empty() {
        return fail(1, "migration source has no entries");
    }

    let mut identities = std::collections::HashSet::new();
    for secret in &secrets {
        let identity = (
            secret.key().to_ascii_lowercase(),
            secret.tag().unwrap_or("").to_ascii_lowercase(),
        );
        if !identities.insert(identity) {
            return fail(
                2,
                &format!(
                    "migration source contains a duplicate key/tag identity for {}",
                    secret.key()
                ),
            );
        }
    }

    println!(
        "migration plan: {} entries from {} -> macOS Keychain",
        secrets.len(),
        source_path.display()
    );
    for secret in &secrets {
        println!(
            "  {} ({})",
            secret.key(),
            secret.tag().unwrap_or("untagged")
        );
    }
    if dry_run {
        println!("dry run: no Keychain writes; source retained");
        return 0;
    }

    #[cfg(target_os = "macos")]
    {
        let target = match backend::NativeKeychainStore::new() {
            Ok(target) => target,
            Err(error) => return fail(2, &store_error_message(&error)),
        };
        let existing = match target.metadata(None) {
            Ok(existing) => existing,
            Err(error) => return fail(2, &store_error_message(&error)),
        };
        for secret in &secrets {
            if existing.iter().any(|item| {
                item.key().eq_ignore_ascii_case(secret.key())
                    && item
                        .tag()
                        .unwrap_or("")
                        .eq_ignore_ascii_case(secret.tag().unwrap_or(""))
            }) {
                return fail(
                    2,
                    &format!(
                        "Keychain already contains key/tag identity for {}; source retained",
                        secret.key()
                    ),
                );
            }
        }
        for (index, secret) in secrets.iter().enumerate() {
            if let Err(error) = target.set(secret.key(), secret.tag(), secret.value()) {
                return fail(
                    1,
                    &format!(
                        "migration stopped after {} entries: {}; source retained; remove partial Keychain entries before retry",
                        index,
                        store_error_message(&error)
                    ),
                );
            }
        }
        println!(
            "migration complete: {} entries written; source retained",
            secrets.len()
        );
        0
    }

    #[cfg(not(target_os = "macos"))]
    {
        fail(2, "Keychain migration requires macOS; source retained")
    }
}

// ---------------------------------------------------------------- doctor

fn cmd_doctor() -> i32 {
    use std::os::unix::fs::PermissionsExt;
    let mut warnings = 0;

    let selected_backend = match config::backend() {
        Ok(backend) => backend,
        Err(message) => {
            warnings += 1;
            println!("global backend: invalid\n  warn: {message}");
            config::Backend::File
        }
    };
    println!(
        "global backend: {}",
        match selected_backend {
            config::Backend::File => "file",
            config::Backend::Keychain => "keychain",
        }
    );
    if selected_backend == config::Backend::Keychain {
        #[cfg(target_os = "macos")]
        match backend::NativeKeychainStore::new() {
            Ok(_) => println!("  keychain: provider available (metadata not read)"),
            Err(error) => {
                warnings += 1;
                println!("  warn: {}", store_error_message(&error));
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            warnings += 1;
            println!("  warn: keychain backend requires macOS");
        }
    }

    let gpath = config::global_store();
    if selected_backend == config::Backend::File {
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
                    && age.as_secs() > 30 * 24 * 3600
                {
                    warnings += 1;
                    println!(
                        "local: {name}\n  warn: backup older than 30 days — consider deleting"
                    );
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
