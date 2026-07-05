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
///   its sandbox. Caveat: with the sandbox bypassed it may be absent — set
///   `AGENTS_ENV_AGENT_MODE=1` in that config to stay safe.
/// - Hermes Agent sets `HERMES_SESSION_ID`/`HERMES_SESSION_KEY` for gateway and
///   tool-run child commands.
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
    "HERMES_SESSION_ID",
    "HERMES_SESSION_KEY",
    "AGENTS_ENV_AGENT_MODE",
];

/// Agent markers whose values may carry session metadata and should be masked
/// if a wrapped child prints its inherited environment.
pub const MASKED_MARKERS: &[&str] = &["HERMES_SESSION_ID", "HERMES_SESSION_KEY"];

pub fn agent_mode() -> bool {
    let builtin = MARKERS
        .iter()
        .any(|m| std::env::var_os(m).is_some_and(|v| !v.is_empty()));
    builtin
        || extra_markers()
            .iter()
            .any(|m| std::env::var_os(m).is_some_and(|v| !v.is_empty()))
}

pub fn masked_marker_values() -> Vec<(String, String)> {
    MASKED_MARKERS
        .iter()
        .filter_map(|m| {
            std::env::var(m)
                .ok()
                .filter(|v| v.len() >= 6)
                .map(|v| ((*m).to_string(), v))
        })
        .collect()
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
