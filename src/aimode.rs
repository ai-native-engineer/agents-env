//! Agent-mode detection.
//!
//! When any of these markers is present in the environment, agents-env assumes
//! the caller is an AI agent whose stdout/stderr ends up in a model context:
//! plaintext value output is disabled and output masking cannot be turned off.

/// Environment variable names that mark an AI-agent environment.
/// `AGENTS_ENV_AGENT_MODE` lets unknown harnesses opt in explicitly.
pub const MARKERS: &[&str] = &[
    "CLAUDECODE",
    "CLAUDE_CODE_ENTRYPOINT",
    "AI_AGENT",
    "AGENTS_ENV_AGENT_MODE",
];

pub fn agent_mode() -> bool {
    MARKERS
        .iter()
        .any(|m| std::env::var_os(m).is_some_and(|v| !v.is_empty()))
}
