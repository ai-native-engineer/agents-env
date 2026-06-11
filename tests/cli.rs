//! End-to-end tests: every command runs the real binary in a sandbox HOME,
//! covering the guard bypass cases, masking, backups and mode switching.

use assert_cmd::Command;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

const AI_MARKERS: &[&str] = &[
    "CLAUDECODE",
    "CLAUDE_CODE_ENTRYPOINT",
    "AI_AGENT",
    "CODEX_SANDBOX",
    "AGENTS_ENV_AGENT_MODE",
];

const GLOBAL: &str = r#"# global store fixture
TAVILY_API_KEY="tvly-aaaa1111bbbb2222" # personal
GEMINI_API_KEY="gem-personal-XYZ12345" # personal
GEMINI_API_KEY="gem-work-ABC67890" # work account
PLAIN_PORT=8080
"#;

struct Sandbox {
    home: TempDir,
    cwd: TempDir,
}

impl Sandbox {
    fn new() -> Sandbox {
        let home = TempDir::new().unwrap();
        let cwd = TempDir::new().unwrap();
        let store_dir = home.path().join(".config/agents-env");
        fs::create_dir_all(&store_dir).unwrap();
        fs::write(store_dir.join("global.env"), GLOBAL).unwrap();
        Sandbox { home, cwd }
    }

    fn global_path(&self) -> PathBuf {
        self.home.path().join(".config/agents-env/global.env")
    }

    /// Build a command with a clean environment. `agent` toggles agent mode.
    fn cmd(&self, agent: bool) -> Command {
        let mut c = Command::cargo_bin("agents-env").unwrap();
        c.current_dir(self.cwd.path());
        for m in AI_MARKERS {
            c.env_remove(m);
        }
        c.env("HOME", self.home.path());
        c.env("PATH", std::env::var("PATH").unwrap());
        if agent {
            c.env("CLAUDECODE", "1");
        }
        c
    }

    fn local(&self, name: &str) -> String {
        fs::read_to_string(self.cwd.path().join(name)).unwrap()
    }
}

// ---------------------------------------------------------------- modes

#[test]
fn human_get_prints_value() {
    let sb = Sandbox::new();
    let out = sb.cmd(false).args(["get", "tavily"]).assert().success();
    let stdout = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    assert!(stdout.contains("tvly-aaaa1111bbbb2222"));
}

#[test]
fn agent_get_hides_value() {
    let sb = Sandbox::new();
    let out = sb.cmd(true).args(["get", "tavily"]).assert().success();
    let stdout = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    assert!(!stdout.contains("tvly-aaaa1111bbbb2222"));
    assert!(stdout.contains("TAVILY_API_KEY"));
    assert!(stdout.contains("[set,"));
}

#[test]
fn codex_sandbox_marker_hides_value() {
    let sb = Sandbox::new();
    let out = sb
        .cmd(false) // human base, then add Codex's sandbox marker
        .env("CODEX_SANDBOX", "seatbelt")
        .args(["get", "tavily"])
        .assert()
        .success();
    let stdout = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    assert!(!stdout.contains("tvly-aaaa1111bbbb2222"));
    assert!(stdout.contains("[set,"));
}

#[test]
fn config_extra_markers_trigger_agent_mode() {
    let sb = Sandbox::new();
    fs::write(
        sb.home.path().join(".config/agents-env/config"),
        "markers=MY_CUSTOM_AGENT\n",
    )
    .unwrap();
    let out = sb
        .cmd(false)
        .env("MY_CUSTOM_AGENT", "1")
        .args(["get", "tavily"])
        .assert()
        .success();
    let stdout = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    assert!(!stdout.contains("tvly-aaaa1111bbbb2222"));
    assert!(stdout.contains("[set,"));
}

#[test]
fn ls_never_prints_values() {
    let sb = Sandbox::new();
    for agent in [false, true] {
        let out = sb.cmd(agent).arg("ls").assert().success();
        let stdout = String::from_utf8(out.get_output().stdout.clone()).unwrap();
        assert!(!stdout.contains("tvly-"), "agent={agent}");
        assert!(stdout.contains("GEMINI_API_KEY"));
    }
}

// ---------------------------------------------------------------- run

#[test]
fn run_injects_env_and_masks_stdout() {
    let sb = Sandbox::new();
    let out = sb
        .cmd(true)
        .args([
            "run",
            "TAVILY_API_KEY",
            "--",
            "sh",
            "-c",
            "echo token=$TAVILY_API_KEY",
        ])
        .assert()
        .success();
    let stdout = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    assert!(stdout.contains("token=[masked:TAVILY_API_KEY]"));
    assert!(!stdout.contains("tvly-aaaa1111bbbb2222"));
}

#[test]
fn run_masks_stderr_and_other_global_values() {
    let sb = Sandbox::new();
    // Child echoes a DIFFERENT secret (work gemini key) to stderr: still masked,
    // because the whole global store is in the mask set.
    let out = sb
        .cmd(true)
        .args([
            "run",
            "TAVILY_API_KEY",
            "--",
            "sh",
            "-c",
            "echo leak=gem-work-ABC67890 1>&2",
        ])
        .assert()
        .success();
    let stderr = String::from_utf8(out.get_output().stderr.clone()).unwrap();
    assert!(!stderr.contains("gem-work-ABC67890"));
    assert!(stderr.contains("[masked:GEMINI_API_KEY]"));
}

#[test]
fn run_substitutes_argv_placeholders() {
    let sb = Sandbox::new();
    let out = sb
        .cmd(true)
        .args([
            "run",
            "TAVILY_API_KEY",
            "--",
            "sh",
            "-c",
            r#"[ "$1" = "tvly-aaaa1111bbbb2222" ] && echo MATCH"#,
            "sh",
            "{{TAVILY_API_KEY}}",
        ])
        .assert()
        .success();
    let stdout = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    assert!(stdout.contains("MATCH"));
}

#[test]
fn run_propagates_exit_code() {
    let sb = Sandbox::new();
    sb.cmd(true)
        .args(["run", "TAVILY_API_KEY", "--", "sh", "-c", "exit 7"])
        .assert()
        .code(7);
}

#[test]
fn run_ambiguous_selector_lists_tags_without_values() {
    let sb = Sandbox::new();
    let out = sb
        .cmd(true)
        .args(["run", "GEMINI_API_KEY", "--", "true"])
        .assert()
        .code(2);
    let stderr = String::from_utf8(out.get_output().stderr.clone()).unwrap();
    assert!(stderr.contains("GEMINI_API_KEY@"));
    assert!(!stderr.contains("gem-personal-XYZ12345"));
    assert!(!stderr.contains("gem-work-ABC67890"));
}

#[test]
fn run_selector_with_tag_picks_account() {
    let sb = Sandbox::new();
    let out = sb
        .cmd(true)
        .args([
            "run",
            "GEMINI_API_KEY@work",
            "--",
            "sh",
            "-c",
            r#"[ "$GEMINI_API_KEY" = "gem-work-ABC67890" ] && echo PICKED"#,
        ])
        .assert()
        .success();
    let stdout = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    assert!(stdout.contains("PICKED"));
}

#[test]
fn run_masks_short_secret() {
    // Regression: values < 6 bytes used to be dropped from the mask set, so a
    // short injected secret leaked. Injected values must always be masked.
    let sb = Sandbox::new();
    fs::write(
        sb.home.path().join(".config/agents-env/global.env"),
        "PIN=\"12345\" # personal\n",
    )
    .unwrap();
    let out = sb
        .cmd(true)
        .args(["run", "PIN", "--", "sh", "-c", "echo got=$PIN"])
        .assert()
        .success();
    let stdout = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    assert!(stdout.contains("got=[masked:PIN]"), "stdout was: {stdout}");
    assert!(!stdout.contains("12345"));
}

#[test]
fn run_masks_overlapping_secret_no_suffix_leak() {
    // A is a prefix of B; printing B must mask the whole thing, not leak B's tail.
    let sb = Sandbox::new();
    fs::write(
        sb.home.path().join(".config/agents-env/global.env"),
        "A=\"abcdefghij\" # one\nB=\"abcdefghijKLMNOP\" # two\n",
    )
    .unwrap();
    let out = sb
        .cmd(true)
        .args(["run", "B", "--", "sh", "-c", "echo v=$B"])
        .assert()
        .success();
    let stdout = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    assert!(!stdout.contains("KLMNOP"), "suffix leaked: {stdout}");
    assert!(!stdout.contains("abcdefghij"));
}

#[test]
fn run_no_mask_refused_in_agent_mode() {
    let sb = Sandbox::new();
    sb.cmd(true)
        .args(["run", "--no-mask", "TAVILY_API_KEY", "--", "true"])
        .assert()
        .code(2);
}

#[test]
fn run_no_mask_allowed_for_humans() {
    let sb = Sandbox::new();
    let out = sb
        .cmd(false)
        .args([
            "run",
            "--no-mask",
            "TAVILY_API_KEY",
            "--",
            "sh",
            "-c",
            "echo $TAVILY_API_KEY",
        ])
        .assert()
        .success();
    let stdout = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    assert!(stdout.contains("tvly-aaaa1111bbbb2222"));
}

// ---------------------------------------------------------------- copy / set

#[test]
fn copy_writes_value_without_printing_it() {
    let sb = Sandbox::new();
    let out = sb
        .cmd(true)
        .args(["copy", "GEMINI_API_KEY@work", "--to", ".env.local"])
        .assert()
        .success();
    let stdout = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    let stderr = String::from_utf8(out.get_output().stderr.clone()).unwrap();
    assert!(!stdout.contains("gem-work-ABC67890"));
    assert!(!stderr.contains("gem-work-ABC67890"));
    assert!(stdout.contains("copied GEMINI_API_KEY"));
    let content = sb.local(".env.local");
    assert!(content.contains("GEMINI_API_KEY=\"gem-work-ABC67890\""));
    assert!(content.contains("# work account"), "tag comment travels along");
}

#[test]
fn copy_as_renames_key() {
    let sb = Sandbox::new();
    sb.cmd(true)
        .args([
            "copy",
            "GEMINI_API_KEY@work",
            "--to",
            ".env.local",
            "--as",
            "GOOGLE_KEY",
        ])
        .assert()
        .success();
    assert!(sb.local(".env.local").contains("GOOGLE_KEY=\"gem-work-ABC67890\""));
}

#[test]
fn set_updates_in_place_preserving_other_lines() {
    let sb = Sandbox::new();
    fs::write(
        sb.cwd.path().join(".env"),
        "# header\nA=\"old\" # tag\nB=keep\n",
    )
    .unwrap();
    sb.cmd(true).args(["set", "A", "new"]).assert().success();
    assert_eq!(sb.local(".env"), "# header\nA=\"new\" # tag\nB=keep\n");
}

#[test]
fn set_refuses_duplicate_key_in_target() {
    let sb = Sandbox::new();
    fs::write(sb.cwd.path().join(".env"), "A=1\nA=2\n").unwrap();
    sb.cmd(true).args(["set", "A", "3"]).assert().code(2);
    assert_eq!(sb.local(".env"), "A=1\nA=2\n");
}

#[test]
fn set_warns_on_credential_looking_value() {
    let sb = Sandbox::new();
    let out = sb
        .cmd(true)
        .args(["set", "K", "sk-abcdef123456"])
        .assert()
        .success();
    let stderr = String::from_utf8(out.get_output().stderr.clone()).unwrap();
    assert!(stderr.contains("looks like a credential"));
}

// ---------------------------------------------------------------- backups

#[test]
fn backup_is_first_wins_per_day() {
    let sb = Sandbox::new();
    sb.cmd(true).args(["set", "K", "v1"]).assert().success(); // create — no backup
    sb.cmd(true).args(["set", "K", "v2"]).assert().success(); // backup of v1
    sb.cmd(true).args(["set", "K", "v3"]).assert().success(); // backup stays v1
    let baks: Vec<_> = fs::read_dir(sb.cwd.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.ends_with(".bak"))
        .collect();
    assert_eq!(baks.len(), 1, "exactly one backup: {baks:?}");
    assert!(baks[0].starts_with(".env."), "covered by .env* patterns: {}", baks[0]);
    let bak_content = sb.local(&baks[0]);
    assert!(bak_content.contains("v1"), "first-wins keeps day-start state");
    assert!(!bak_content.contains("v2"));
}

// ---------------------------------------------------------------- write guards

#[test]
fn guard_rejects_path_separators_and_non_env_names() {
    let sb = Sandbox::new();
    sb.cmd(true)
        .args(["set", "K", "v", "--to", "../evil.env"])
        .assert()
        .code(2);
    sb.cmd(true)
        .args(["set", "K", "v", "--to", "/tmp/.env"])
        .assert()
        .code(2);
    sb.cmd(true)
        .args(["set", "K", "v", "--to", "notenv"])
        .assert()
        .code(2);
    sb.cmd(true)
        .args(["set", "K", "v", "--to", ".env.260101.bak"])
        .assert()
        .code(2);
}

#[test]
fn guard_rejects_symlink_target() {
    let sb = Sandbox::new();
    let other = sb.cwd.path().join("other.txt");
    fs::write(&other, "x").unwrap();
    std::os::unix::fs::symlink(&other, sb.cwd.path().join(".env")).unwrap();
    sb.cmd(true).args(["set", "K", "v"]).assert().code(2);
    assert_eq!(fs::read_to_string(&other).unwrap(), "x");
}

#[test]
fn guard_rejects_hardlinked_target() {
    let sb = Sandbox::new();
    let env = sb.cwd.path().join(".env");
    fs::write(&env, "A=1\n").unwrap();
    fs::hard_link(&env, sb.cwd.path().join(".env.alias")).unwrap();
    sb.cmd(true).args(["set", "K", "v"]).assert().code(2);
}

#[test]
fn guard_rejects_writes_in_global_store_directory() {
    let sb = Sandbox::new();
    let store_dir = sb.home.path().join(".config/agents-env");
    let mut c = sb.cmd(true);
    c.current_dir(&store_dir);
    c.args(["set", "K", "v", "--to", ".env.local"]).assert().code(2);
}

#[test]
fn guard_global_store_is_unreachable_even_via_symlink_name() {
    let sb = Sandbox::new();
    // a local symlink pointing at the global store
    std::os::unix::fs::symlink(sb.global_path(), sb.cwd.path().join(".env.g")).unwrap();
    sb.cmd(true)
        .args(["set", "K", "v", "--to", ".env.g"])
        .assert()
        .code(2);
    assert_eq!(fs::read_to_string(sb.global_path()).unwrap(), GLOBAL);
}

// ---------------------------------------------------------------- git gate

fn git(cwd: &Path, args: &[&str]) {
    let st = std::process::Command::new("git")
        .args(args)
        .current_dir(cwd)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap();
    assert!(st.success(), "git {args:?} failed");
}

#[test]
fn copy_refuses_non_gitignored_target_then_accepts_after_fix() {
    let sb = Sandbox::new();
    git(sb.cwd.path(), &["init", "-q"]);
    sb.cmd(true)
        .args(["copy", "TAVILY_API_KEY", "--to", ".env"])
        .assert()
        .code(2);
    assert!(!sb.cwd.path().join(".env").exists());

    fs::write(sb.cwd.path().join(".gitignore"), ".env*\n").unwrap();
    sb.cmd(true)
        .args(["copy", "TAVILY_API_KEY", "--to", ".env"])
        .assert()
        .success();
    assert!(sb.local(".env").contains("tvly-aaaa1111bbbb2222"));
}

#[test]
fn copy_refuses_git_tracked_target() {
    let sb = Sandbox::new();
    git(sb.cwd.path(), &["init", "-q"]);
    fs::write(sb.cwd.path().join(".env"), "A=1\n").unwrap();
    git(sb.cwd.path(), &["add", "-f", ".env"]);
    sb.cmd(true)
        .args(["copy", "TAVILY_API_KEY", "--to", ".env"])
        .assert()
        .code(2);
    assert_eq!(sb.local(".env"), "A=1\n");
}

// ---------------------------------------------------------------- edit

#[test]
fn edit_refused_in_agent_mode_and_non_tty() {
    let sb = Sandbox::new();
    sb.cmd(true).arg("edit").assert().code(2);
    // human mode but stdin/stdout are pipes -> still refused
    sb.cmd(false).env("EDITOR", "cat").arg("edit").assert().code(2);
    assert_eq!(fs::read_to_string(sb.global_path()).unwrap(), GLOBAL);
}

// ---------------------------------------------------------------- local scope

#[test]
fn local_scope_reads_named_file() {
    let sb = Sandbox::new();
    fs::write(sb.cwd.path().join(".env.production"), "DB_URL=\"postgres://x:hunter2secret@h/db\"\n").unwrap();
    let out = sb
        .cmd(true)
        .args(["-f", ".env.production", "get", "DB"])
        .assert()
        .success();
    let stdout = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    assert!(stdout.contains("DB_URL"));
    assert!(!stdout.contains("hunter2secret"));
}
