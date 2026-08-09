//! Agent-mode detection.
//!
//! When any of these markers is present in the environment, agents-env assumes
//! the caller is an AI agent whose stdout/stderr ends up in a model context:
//! plaintext value output is disabled and output masking cannot be turned off.

/// Environment variable names that mark an AI-agent environment.
///
/// - Claude Code injects `CLAUDECODE` into spawned subprocesses; newer
///   versions also set the more specific `CLAUDE_CODE_CHILD_SESSION`.
///   `CLAUDE_CODE_ENTRYPOINT` and `AI_AGENT` are kept for existing harnesses.
/// - OpenAI Codex sets `CODEX_SANDBOX` (e.g. `seatbelt`) on commands run inside
///   its sandbox.
/// - OpenCode sets `OPENCODE=1`; Antigravity supplies a conversation ID to
///   terminal commands.
/// - `AGENTS_ENV_AGENT_MODE` lets any other harness opt in explicitly.
///
/// Do not add guessed tool-specific markers here. If a coding assistant has no
/// verified stable child-process marker, document `AGENTS_ENV_AGENT_MODE=1` or
/// `markers=` instead.
///
/// Extra markers can be added without recompiling via a `markers=A,B,C` line in
/// `~/.config/agents-env/config`.
pub const MARKERS: &[&str] = &[
    "CLAUDECODE",
    "CLAUDE_CODE_CHILD_SESSION",
    "CLAUDE_CODE_ENTRYPOINT",
    "AI_AGENT",
    "CODEX_SANDBOX",
    "OPENCODE",
    "ANTIGRAVITY_CONVERSATION_ID",
    "AGENTS_ENV_AGENT_MODE",
];

const AGENT_CLI_NAMES: &[&str] = &["grok", "codex", "opencode", "claude", "agy"];

pub fn agent_mode() -> bool {
    let builtin = MARKERS
        .iter()
        .any(|m| std::env::var_os(m).is_some_and(|v| !v.is_empty()));
    builtin
        || extra_markers()
            .iter()
            .any(|m| std::env::var_os(m).is_some_and(|v| !v.is_empty()))
        || agent_cli_ancestor()
}

#[cfg(unix)]
fn agent_cli_ancestor() -> bool {
    let mut pid = std::process::id();
    // ponytail: three levels cover CLI -> shell -> command; use an env marker
    // for renamed, detached, or more deeply wrapped launchers.
    for _ in 0..3 {
        let Ok(output) = std::process::Command::new("ps")
            .args(["-o", "ppid=,args=", "-p"])
            .arg(pid.to_string())
            .output()
        else {
            return false;
        };
        if !output.status.success() {
            return false;
        }
        let Ok(process) = std::str::from_utf8(&output.stdout) else {
            return false;
        };
        let mut fields = process.split_whitespace();
        let Some(parent) = fields.next().and_then(|s| s.parse::<u32>().ok()) else {
            return false;
        };
        let Some(argv0) = fields.next() else {
            return false;
        };
        let name = std::path::Path::new(argv0)
            .file_name()
            .and_then(|s| s.to_str());
        if name.is_some_and(|name| AGENT_CLI_NAMES.contains(&name)) {
            return true;
        }
        pid = parent;
    }
    false
}

#[cfg(not(unix))]
fn agent_cli_ancestor() -> bool {
    false
}

/// User-configured extra markers from `markers=` in the config file.
fn extra_markers() -> Vec<String> {
    let path = crate::config::config_path();
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    for line in text.lines() {
        if let Some(rest) = line.trim().strip_prefix("markers=") {
            return rest
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
        }
    }
    Vec::new()
}
