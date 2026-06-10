//! Global store location.
//!
//! The global store is a human-owned master .env file. Its path comes from
//! `~/.config/agents-env/config` (a `global_store=<path>` line) and is NOT
//! overridable per invocation (no flag, no environment variable) — that would
//! be a bypass surface for the write guard.

use std::fs;
use std::path::PathBuf;

pub fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").expect("HOME is not set"))
}

pub fn config_dir() -> PathBuf {
    home().join(".config").join("agents-env")
}

pub fn config_path() -> PathBuf {
    config_dir().join("config")
}

/// Path of the global store. Defaults to `~/.config/agents-env/global.env`.
pub fn global_store() -> PathBuf {
    if let Ok(text) = fs::read_to_string(config_path()) {
        for line in text.lines() {
            let line = line.trim();
            if let Some(rest) = line.strip_prefix("global_store=") {
                let p = rest.trim();
                if !p.is_empty() {
                    return expand_tilde(p);
                }
            }
        }
    }
    config_dir().join("global.env")
}

fn expand_tilde(p: &str) -> PathBuf {
    if let Some(rest) = p.strip_prefix("~/") {
        home().join(rest)
    } else {
        PathBuf::from(p)
    }
}
