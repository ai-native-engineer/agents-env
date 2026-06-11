//! Agent-mode detection.
//!
//! When any of these markers is present in the environment, agents-env assumes
//! the caller is an AI agent whose stdout/stderr ends up in a model context:
//! plaintext value output is disabled and output masking cannot be turned off.

/// Environment variable names that mark an AI-agent environment.
///
/// - Claude Code injects `CLAUDECODE` / `CLAUDE_CODE_ENTRYPOINT` / `AI_AGENT`
///   into every shell it spawns.
/// - OpenAI Codex sets `CODEX_SANDBOX` (e.g. `seatbelt`) on commands run inside
///   its sandbox. Caveat: with the sandbox bypassed it may be absent — set
///   `AGENTS_ENV_AGENT_MODE=1` in that config to stay safe.
/// - `AGENTS_ENV_AGENT_MODE` lets any other harness opt in explicitly.
///
/// Extra markers can be added without recompiling via a `markers=A,B,C` line in
/// `~/.config/agents-env/config`.
pub const MARKERS: &[&str] = &[
    "CLAUDECODE",
    "CLAUDE_CODE_ENTRYPOINT",
    "AI_AGENT",
    "CODEX_SANDBOX",
    "AGENTS_ENV_AGENT_MODE",
];

pub fn agent_mode() -> bool {
    let builtin = MARKERS
        .iter()
        .any(|m| std::env::var_os(m).is_some_and(|v| !v.is_empty()));
    builtin || extra_markers().iter().any(|m| std::env::var_os(m).is_some_and(|v| !v.is_empty()))
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
