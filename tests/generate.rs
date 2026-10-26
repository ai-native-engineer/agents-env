//! Generation uses fake, isolated stores only; the user's real store is never read.
use assert_cmd::Command;
use std::fs;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command as Process, Stdio};
use std::time::Duration;
use tempfile::TempDir;

struct Sandbox {
    root: TempDir,
    home: PathBuf,
    cwd: PathBuf,
    store: PathBuf,
}

impl Sandbox {
    fn new(contents: &[u8]) -> Self {
        let root = TempDir::new().unwrap();
        let home = root.path().join("home");
        let cwd = root.path().join("project");
        let store = home.join(".config/agents-env/global.env");
        fs::create_dir_all(store.parent().unwrap()).unwrap();
        fs::create_dir_all(&cwd).unwrap();
        fs::write(&store, contents).unwrap();
        Self {
            root,
            home,
            cwd,
            store,
        }
    }

    fn cmd(&self) -> Command {
        let mut command = Command::cargo_bin("agents-env").unwrap();
        command
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap())
            .env("HOME", &self.home)
            .env("AGENTS_ENV_AGENT_MODE", "1")
            .current_dir(&self.cwd)
            .timeout(Duration::from_secs(15));
        command
    }

    fn backup_files(&self) -> Vec<PathBuf> {
        fs::read_dir(self.store.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.extension().is_some_and(|extension| extension == "bak"))
            .collect()
    }

    fn generate(&self, key: &str) -> String {
        let result = self.cmd().args(["generate", key]).assert().success();
        let text = fs::read_to_string(&self.store).unwrap();
        let prefix = format!("{key}=\"");
        let value = text
            .lines()
            .find_map(|line| line.strip_prefix(&prefix))
            .unwrap()
            .split('"')
            .next()
            .unwrap()
            .to_string();
        assert_eq!(value.len(), 64);
        assert!(
            value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        );
        assert!(!String::from_utf8_lossy(&result.get_output().stdout).contains(&value));
        assert!(!String::from_utf8_lossy(&result.get_output().stderr).contains(&value));
        value
    }
}

fn git(cwd: &Path, arguments: &[&str]) {
    assert!(
        Process::new("git")
            .args(arguments)
            .current_dir(cwd)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap()
            .success()
    );
}

#[test]
fn generate_preserves_bytes_and_new_key_is_discoverable_and_masked() {
    for original in [
        "# accounts\nOLD='fixture-old-value' # keep\n",
        "# 마지막 주석 (개행 없음)",
        "OLD=fixture-old-value",
        "# header\r\nOLD=fixture-old-value\r\n",
        "\n\nOLD=fixture-old-value # personal",
    ] {
        let sb = Sandbox::new(original.as_bytes());
        let value = sb.generate("NEW_KEY");
        assert!(
            fs::read(&sb.store)
                .unwrap()
                .starts_with(original.as_bytes())
        );
        let found = sb.cmd().args(["get", "NEW_KEY"]).assert().success();
        let stdout = String::from_utf8_lossy(&found.get_output().stdout);
        assert!(stdout.contains("NEW_KEY  [set, 64 chars]"));
        assert!(!stdout.contains(&value));
        let used = sb
            .cmd()
            .args([
                "run",
                "NEW_KEY",
                "--",
                "sh",
                "-c",
                "printf '%s' \"$NEW_KEY\"; exit 17",
            ])
            .assert()
            .code(17);
        assert_eq!(used.get_output().stdout, b"[masked:NEW_KEY]");
        assert!(!String::from_utf8_lossy(&used.get_output().stderr).contains(&value));
        assert_eq!(sb.backup_files().len(), 1);
        assert_eq!(
            fs::read(&sb.backup_files()[0]).unwrap(),
            original.as_bytes()
        );
        assert_eq!(fs::metadata(&sb.store).unwrap().mode() & 0o777, 0o600);
        assert_eq!(
            fs::metadata(&sb.backup_files()[0]).unwrap().mode() & 0o777,
            0o600
        );
    }
}

#[test]
fn generate_rejects_existing_empty_tagged_and_case_variant_keys_without_changes() {
    for original in [
        "KEY=fixture-original\n",
        "KEY=\n",
        "export KEY='fixture-original' # work\n",
        "key=fixture-original\n",
        "KEY=fixture-personal # personal\nKEY=fixture-work # work\n",
    ] {
        let sb = Sandbox::new(original.as_bytes());
        sb.cmd().args(["generate", "KEY"]).assert().code(2);
        assert_eq!(fs::read(&sb.store).unwrap(), original.as_bytes());
        assert!(sb.backup_files().is_empty());
    }
    let sb = Sandbox::new(b"KEEP=fixture-existing\n");
    sb.generate("NEW_KEY");
    let first = fs::read(&sb.store).unwrap();
    sb.cmd().args(["generate", "NEW_KEY"]).assert().code(2);
    assert_eq!(fs::read(&sb.store).unwrap(), first);
}

#[test]
fn unrelated_tagged_accounts_survive_and_select_as_before() {
    let original = "API_KEY=fixture-personal # personal\nAPI_KEY=fixture-work # work\n";
    let sb = Sandbox::new(original.as_bytes());
    sb.generate("LOCAL_SERVICE_KEY");
    sb.cmd()
        .args(["run", "API_KEY", "--", "true"])
        .assert()
        .code(2);
    let output = sb
        .cmd()
        .args([
            "run",
            "API_KEY@work",
            "--",
            "sh",
            "-c",
            "test \"$API_KEY\" = fixture-work && printf '%s' \"$API_KEY\"",
        ])
        .assert()
        .success();
    assert_eq!(output.get_output().stdout, b"[masked:API_KEY]");
    assert!(fs::read_to_string(&sb.store).unwrap().starts_with(original));
}

#[test]
fn numeric_existing_keys_are_preserved_discoverable_and_masked() {
    let original = "# fixture accounts\r\n1SERVICE_KEY='fixture-numeric-value' # old\r\n";
    let sb = Sandbox::new(original.as_bytes());
    sb.generate("NEW_KEY");
    assert!(
        fs::read(&sb.store)
            .unwrap()
            .starts_with(original.as_bytes())
    );
    let found = sb.cmd().args(["get", "1SERVICE_KEY"]).assert().success();
    let stdout = String::from_utf8_lossy(&found.get_output().stdout);
    assert!(stdout.contains("1SERVICE_KEY"));
    assert!(!stdout.contains("fixture-numeric-value"));
    let result = sb
        .cmd()
        .args(["run", "1SERVICE_KEY@old", "--", "env"])
        .assert()
        .success();
    assert!(
        String::from_utf8_lossy(&result.get_output().stdout)
            .contains("1SERVICE_KEY=[masked:1SERVICE_KEY]")
    );
    assert!(
        !String::from_utf8_lossy(&result.get_output().stdout).contains("fixture-numeric-value")
    );
    let before = fs::read(&sb.store).unwrap();
    sb.cmd().args(["generate", "1SERVICE_KEY"]).assert().code(3);
    assert_eq!(fs::read(&sb.store).unwrap(), before);
    assert_eq!(
        fs::read(&sb.backup_files()[0]).unwrap(),
        original.as_bytes()
    );
}

#[test]
fn malformed_input_cannot_hide_a_key_or_leak_its_contents() {
    for original in [
        b"KEY=fixture-old\nBAD=\"fixture-unclosed".as_slice(),
        b"KEY =fixture-spaced\n",
        b"export KEY =fixture-spaced\n",
        b"KEY='fixture-value' trailing-secret\n",
        b"\xef\xbb\xbfKEY=fixture-bom\n",
        b"KEY=fixture\0value\n",
        b"KEY=\xff\n",
        b"KEY=fixture-original # trailing-comment\r",
    ] {
        let sb = Sandbox::new(original);
        let output = sb.cmd().args(["generate", "NEW_KEY"]).assert().code(2);
        let message = format!(
            "{}{}",
            String::from_utf8_lossy(&output.get_output().stdout),
            String::from_utf8_lossy(&output.get_output().stderr)
        );
        assert!(!message.contains("fixture"));
        assert!(!message.contains("trailing-secret"));
        assert_eq!(fs::read(&sb.store).unwrap(), original);
        assert!(sb.backup_files().is_empty());
    }
}

#[test]
fn generate_never_prints_value_in_human_mode_and_uses_no_caller_value() {
    let sb = Sandbox::new(b"");
    let output = sb
        .cmd()
        .env_remove("AGENTS_ENV_AGENT_MODE")
        .args(["generate", "NEW_KEY"])
        .assert()
        .success();
    let text = fs::read_to_string(&sb.store).unwrap();
    let value = text.split('"').nth(1).unwrap();
    assert!(!String::from_utf8_lossy(&output.get_output().stdout).contains(value));
    assert!(!String::from_utf8_lossy(&output.get_output().stderr).contains(value));
    for arguments in [
        vec!["generate", "KEY", "caller-secret"],
        vec!["generate", "KEY@work"],
        vec!["generate", "BAD=KEY"],
        vec!["generate", "KEY", "--force"],
        vec!["-l", "generate", "KEY"],
        vec!["-f", ".env.local", "generate", "KEY"],
    ] {
        sb.cmd().args(arguments).assert().failure();
        assert_eq!(fs::read_to_string(&sb.store).unwrap(), text);
        assert!(!sb.cwd.join(".env.local").exists());
    }
}

#[test]
fn generate_honors_configured_store_and_creates_private_missing_store() {
    let sb = Sandbox::new(b"DEFAULT=fixture-untouched\n");
    let destination = sb.root.path().join("new-private/store/custom.env");
    fs::write(
        sb.home.join(".config/agents-env/config"),
        format!("global_store={}\n", destination.display()),
    )
    .unwrap();
    sb.cmd()
        .args(["generate", "SERVICE_KEY"])
        .assert()
        .success();
    assert_eq!(
        fs::read_to_string(&sb.store).unwrap(),
        "DEFAULT=fixture-untouched\n"
    );
    assert_eq!(fs::metadata(&destination).unwrap().mode() & 0o777, 0o600);
    assert_eq!(
        fs::metadata(destination.parent().unwrap()).unwrap().mode() & 0o777,
        0o700
    );
    sb.cmd().args(["get", "SERVICE_KEY"]).assert().success();
    let empty = Sandbox::new(b"");
    fs::remove_dir_all(&empty.home).unwrap();
    empty
        .cmd()
        .args(["generate", "FIRST_KEY"])
        .assert()
        .success();
    assert_eq!(fs::metadata(&empty.store).unwrap().mode() & 0o777, 0o600);
}

#[test]
fn generate_refuses_linked_or_nonregular_store_and_config_alias() {
    for kind in ["symlink", "hardlink", "directory"] {
        let sb = Sandbox::new(b"KEEP=fixture-original\n");
        let outside = sb.root.path().join("original");
        fs::rename(&sb.store, &outside).unwrap();
        match kind {
            "symlink" => std::os::unix::fs::symlink(&outside, &sb.store).unwrap(),
            "hardlink" => fs::hard_link(&outside, &sb.store).unwrap(),
            _ => fs::create_dir(&sb.store).unwrap(),
        }
        sb.cmd().args(["generate", "NEW_KEY"]).assert().code(2);
        assert_eq!(fs::read(&outside).unwrap(), b"KEEP=fixture-original\n");
    }
    let sb = Sandbox::new(b"KEEP=fixture-original\n");
    let config = sb.home.join(".config/agents-env/config");
    fs::write(&config, "global_store=config\n").unwrap();
    sb.cmd().args(["generate", "NEW_KEY"]).assert().code(2);
    assert_eq!(
        fs::read_to_string(&config).unwrap(),
        "global_store=config\n"
    );
}

#[test]
fn lock_and_backup_symlinks_cannot_receive_existing_secrets() {
    for suffix in ["agents-env.lock".to_string(), format!("{}.bak", date())] {
        let sb = Sandbox::new(b"KEEP=fixture-original\n");
        let outside = sb.root.path().join("outside");
        let target = sb.store.with_file_name(format!("global.env.{suffix}"));
        std::os::unix::fs::symlink(&outside, &target).unwrap();
        sb.cmd().args(["generate", "NEW_KEY"]).assert().code(2);
        assert!(!outside.exists());
        assert_eq!(fs::read(&sb.store).unwrap(), b"KEEP=fixture-original\n");
    }
}

fn date() -> String {
    let output = Process::new("date").arg("+%y%m%d").output().unwrap();
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

#[test]
fn git_protects_store_backup_and_private_staging_before_writing() {
    for rule in ["", "global.env\n", "global.env\nglobal.env.*.bak\n"] {
        let sb = Sandbox::new(b"KEEP=fixture-original\n");
        let directory = sb.store.parent().unwrap();
        git(directory, &["init", "-q"]);
        fs::write(directory.join(".gitignore"), rule).unwrap();
        sb.cmd().args(["generate", "NEW_KEY"]).assert().code(2);
        assert_eq!(fs::read(&sb.store).unwrap(), b"KEEP=fixture-original\n");
        assert!(sb.backup_files().is_empty());
    }
    let sb = Sandbox::new(b"KEEP=fixture-original\n");
    let directory = sb.store.parent().unwrap();
    git(directory, &["init", "-q"]);
    fs::write(directory.join(".gitignore"), "global.env*\n").unwrap();
    git(directory, &["add", "-f", "global.env"]);
    sb.cmd().args(["generate", "NEW_KEY"]).assert().code(2);
    assert_eq!(fs::read(&sb.store).unwrap(), b"KEEP=fixture-original\n");
    let good = Sandbox::new(b"KEEP=fixture-original\n");
    let directory = good.store.parent().unwrap();
    git(directory, &["init", "-q"]);
    fs::write(directory.join(".gitignore"), "global.env*\n").unwrap();
    good.generate("NEW_KEY");
}

#[test]
fn concurrent_generators_preserve_every_successful_key_and_refuse_replacement() {
    let sb = Sandbox::new(b"KEEP=fixture-original\n");
    let children: Vec<_> = (0..12)
        .map(|number| {
            let mut command = Process::new(env!("CARGO_BIN_EXE_agents-env"));
            command
                .env_clear()
                .env("PATH", std::env::var_os("PATH").unwrap())
                .env("HOME", &sb.home)
                .env("AGENTS_ENV_AGENT_MODE", "1")
                .current_dir(&sb.cwd)
                .args(["generate", &format!("KEY_{number}")])
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap()
        })
        .collect();
    let results: Vec<_> = children
        .into_iter()
        .map(|child| child.wait_with_output().unwrap())
        .collect();
    for (number, output) in results.into_iter().enumerate() {
        if !output.status.success() {
            assert_eq!(output.status.code(), Some(2));
            sb.generate(&format!("KEY_{number}"));
        }
    }
    let original = fs::read_to_string(&sb.store).unwrap();
    assert!(original.starts_with("KEEP=fixture-original\n"));
    for number in 0..12 {
        let key = format!("KEY_{number}");
        assert_eq!(
            original
                .lines()
                .filter(|line| line.starts_with(&format!("{key}=")))
                .count(),
            1
        );
        sb.cmd().args(["get", &key]).assert().success();
        sb.cmd().args(["generate", &key]).assert().code(2);
    }
    assert_eq!(fs::read_to_string(&sb.store).unwrap(), original);
    assert_eq!(
        fs::read(&sb.backup_files()[0]).unwrap(),
        b"KEEP=fixture-original\n"
    );
}

#[test]
fn locked_store_is_refused_without_writing_and_can_retry_after_unlock() {
    let sb = Sandbox::new(b"KEEP=fixture-original\n");
    let path = sb.store.with_file_name("global.env.agents-env.lock");
    let lock = fs::File::create(&path).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(
        unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        0
    );
    sb.cmd().args(["generate", "NEW_KEY"]).assert().code(2);
    assert_eq!(fs::read(&sb.store).unwrap(), b"KEEP=fixture-original\n");
    assert!(sb.backup_files().is_empty());
    // Explicitly release the lock before retrying. A concurrent test can be
    // between fork and exec and briefly retain a duplicate of this descriptor.
    assert_eq!(unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_UN) }, 0);
    drop(lock);
    sb.generate("NEW_KEY");
}

#[test]
fn concurrent_same_key_has_at_most_one_success_and_never_rotates() {
    let sb = Sandbox::new(b"KEEP=fixture-original\n");
    let children: Vec<_> = (0..8)
        .map(|_| {
            Process::new(env!("CARGO_BIN_EXE_agents-env"))
                .env_clear()
                .env("PATH", std::env::var_os("PATH").unwrap())
                .env("HOME", &sb.home)
                .env("AGENTS_ENV_AGENT_MODE", "1")
                .current_dir(&sb.cwd)
                .args(["generate", "ONE_KEY"])
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap()
        })
        .collect();
    let results: Vec<_> = children
        .into_iter()
        .map(|child| child.wait_with_output().unwrap())
        .collect();
    assert_eq!(
        results
            .iter()
            .filter(|output| output.status.success())
            .count(),
        1
    );
    assert!(
        results
            .iter()
            .all(|output| output.status.success() || output.status.code() == Some(2))
    );
    let saved = fs::read_to_string(&sb.store).unwrap();
    assert_eq!(
        saved
            .lines()
            .filter(|line| line.starts_with("ONE_KEY="))
            .count(),
        1
    );
    sb.cmd().args(["generate", "ONE_KEY"]).assert().code(2);
    assert_eq!(fs::read_to_string(&sb.store).unwrap(), saved);
}

#[test]
fn unsafe_fifo_lock_and_backup_are_refused_without_hanging_or_changing_keys() {
    for kind in [
        "store-fifo",
        "lock-fifo",
        "backup-fifo",
        "lock-hardlink",
        "backup-hardlink",
        "backup-public",
    ] {
        let sb = Sandbox::new(b"KEEP=fixture-original\n");
        let outside = sb.root.path().join("outside");
        let target = if kind.starts_with("lock") {
            sb.store.with_file_name("global.env.agents-env.lock")
        } else if kind.starts_with("backup") {
            sb.store
                .with_file_name(format!("global.env.{}.bak", date()))
        } else {
            fs::rename(&sb.store, &outside).unwrap();
            sb.store.clone()
        };
        if kind.ends_with("fifo") {
            use std::os::unix::ffi::OsStrExt;
            let name = std::ffi::CString::new(target.as_os_str().as_bytes()).unwrap();
            assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        } else if kind.ends_with("hardlink") {
            fs::write(&outside, b"fixture-outside\n").unwrap();
            fs::hard_link(&outside, &target).unwrap();
        } else {
            fs::write(&target, b"fixture-public-backup\n").unwrap();
            fs::set_permissions(&target, fs::Permissions::from_mode(0o644)).unwrap();
        }
        sb.cmd().args(["generate", "NEW_KEY"]).assert().code(2);
        if kind == "store-fifo" {
            assert_eq!(fs::read(&outside).unwrap(), b"KEEP=fixture-original\n");
        } else {
            assert_eq!(fs::read(&sb.store).unwrap(), b"KEEP=fixture-original\n");
        }
    }
}

#[test]
fn commented_examples_do_not_become_active_duplicates() {
    let original = "# NEW_KEY=example-not-an-active-secret\nKEEP=fixture-old\n";
    let sb = Sandbox::new(original.as_bytes());
    sb.generate("NEW_KEY");
    assert!(fs::read_to_string(&sb.store).unwrap().starts_with(original));
    sb.cmd().args(["get", "NEW_KEY"]).assert().success();
}

#[test]
fn restrictive_umask_does_not_publish_unreadable_keys() {
    let sb = Sandbox::new(b"KEEP=fixture-original\n");
    let output = Process::new("sh")
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap())
        .env("HOME", &sb.home)
        .env("AGENTS_ENV_AGENT_MODE", "1")
        .current_dir(&sb.cwd)
        .args([
            "-c",
            "umask 0777; exec \"$1\" generate NEW_KEY",
            "sh",
            env!("CARGO_BIN_EXE_agents-env"),
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(fs::metadata(&sb.store).unwrap().mode() & 0o777, 0o600);
    assert_eq!(
        fs::metadata(&sb.backup_files()[0]).unwrap().mode() & 0o777,
        0o600
    );
    sb.cmd().args(["get", "NEW_KEY"]).assert().success();
}

#[test]
fn missing_git_cannot_bypass_global_write_protection() {
    let sb = Sandbox::new(b"KEEP=fixture-original\n");
    sb.cmd()
        .env("PATH", sb.root.path().join("no-tools"))
        .args(["generate", "NEW_KEY"])
        .assert()
        .code(2);
    assert_eq!(fs::read(&sb.store).unwrap(), b"KEEP=fixture-original\n");
    assert!(sb.backup_files().is_empty());
}
