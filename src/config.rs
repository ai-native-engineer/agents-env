//! User-owned, non-secret settings. The store path is resolved once per process
//! and cannot be overridden by a command flag or dedicated environment variable.

use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::OnceLock;

const KEYS: &[&str] = &["global_store", "markers", "editor", "agent_mode"];
static CONFIG: OnceLock<Config> = OnceLock::new();

#[derive(Clone, Debug)]
pub struct Config {
    pub home: PathBuf,
    pub dir: PathBuf,
    pub path: PathBuf,
    pub global_store: PathBuf,
    pub markers: Vec<String>,
    pub editor: Option<String>,
    pub agent_mode_always: bool,
}

impl Config {
    pub fn load() -> Result<Self, String> {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or_else(|| "HOME is not set; set it to an absolute home directory".to_string())?;
        Self::load_with(home, std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from))
    }

    fn load_with(home: PathBuf, xdg: Option<PathBuf>) -> Result<Self, String> {
        if !home.is_absolute() {
            return Err("HOME must be a non-empty absolute directory path".to_string());
        }
        let home = normalize(&home);
        let legacy = home.join(".config/agents-env");
        let dir = xdg
            .filter(|p| p.is_absolute())
            .map(|p| normalize(&p).join("agents-env"))
            .unwrap_or_else(|| legacy.clone());
        let path = dir.join("config");
        if let Some(text) = read_config(&path)? {
            return Self::parse(home, dir, &text);
        }
        // Keep legacy config and its relative paths when no XDG config exists.
        // Neither the configuration nor the secret store is migrated implicitly.
        if dir != legacy
            && let Some(text) = read_config(&legacy.join("config"))?
        {
            return Self::parse(home, legacy, &text);
        }
        Self::parse(home, dir, "")
    }

    fn parse(home: PathBuf, dir: PathBuf, text: &str) -> Result<Self, String> {
        let path = dir.join("config");
        let mut config = Self {
            home,
            global_store: dir.join("global.env"),
            dir,
            path,
            markers: Vec::new(),
            editor: None,
            agent_mode_always: false,
        };
        let mut seen = Vec::new();
        for (number, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let error = |reason: &str| {
                format!(
                    "invalid config {} at line {}: {reason}",
                    config.path.display(),
                    number + 1
                )
            };
            let Some((key, raw)) = line.split_once('=') else {
                return Err(error("expected a key=value setting"));
            };
            let key = key.trim();
            if !KEYS.contains(&key) {
                return Err(error(
                    "unknown setting; allowed keys: global_store, markers, editor, agent_mode",
                ));
            }
            if seen.contains(&key) {
                return Err(error("duplicate setting"));
            }
            seen.push(key);
            let value = value_and_comment(raw).0.trim();
            let decoded;
            let value = if key == "global_store" {
                decoded = decode_path(value).map_err(|reason| error(&reason))?;
                decoded.as_str()
            } else {
                value
            };
            if let Err(reason) = config.apply(key, value) {
                return Err(format!(
                    "invalid config {} at line {}: {reason}",
                    config.path.display(),
                    number + 1
                ));
            }
        }
        Ok(config)
    }

    fn apply(&mut self, key: &str, value: &str) -> Result<(), String> {
        if value.chars().any(char::is_control) {
            return Err("setting values must not contain control characters".to_string());
        }
        match key {
            "global_store" => {
                if value.is_empty() {
                    return Err("global_store must be a non-empty path".to_string());
                }
                let path = if let Some(rest) = value.strip_prefix("~/") {
                    self.home.join(rest)
                } else if value == "~" {
                    self.home.clone()
                } else if Path::new(value).is_absolute() {
                    PathBuf::from(value)
                } else {
                    self.dir.join(value)
                };
                self.global_store = normalize(&path);
            }
            "markers" => {
                let mut markers = Vec::new();
                for marker in value.split(',').map(str::trim).filter(|m| !m.is_empty()) {
                    let mut chars = marker.chars();
                    let first = chars.next().expect("non-empty marker");
                    if !(first.is_ascii_alphabetic() || first == '_')
                        || !chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
                    {
                        return Err(
                            "markers must contain valid environment variable names".to_string()
                        );
                    }
                    if !markers.iter().any(|m| m == marker) {
                        markers.push(marker.to_string());
                    }
                }
                self.markers = markers;
            }
            "editor" => {
                if value.is_empty() {
                    return Err("editor must be a non-empty command".to_string());
                }
                self.editor = Some(value.to_string());
            }
            "agent_mode" => match value {
                "auto" => self.agent_mode_always = false,
                "always" => self.agent_mode_always = true,
                _ => return Err("agent_mode must be auto or always".to_string()),
            },
            _ => {
                return Err(
                    "unknown setting; allowed keys: global_store, markers, editor, agent_mode"
                        .to_string(),
                );
            }
        }
        Ok(())
    }
}

fn read_config(path: &Path) -> Result<Option<String>, String> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("cannot read config {}: {e}", path.display())),
    }
}

/// Initialize before invoking any command, including config mutation commands.
pub fn init() -> Result<(), String> {
    if CONFIG.get().is_none() {
        let config = Config::load()?;
        let _ = CONFIG.set(config);
    }
    Ok(())
}

pub fn current() -> &'static Config {
    CONFIG
        .get()
        .expect("config::init must succeed before command dispatch")
}

pub fn home() -> PathBuf {
    current().home.clone()
}

pub fn config_dir() -> PathBuf {
    current().dir.clone()
}

pub fn config_path() -> PathBuf {
    current().path.clone()
}

pub fn global_store() -> PathBuf {
    current().global_store.clone()
}

/// Display parsed settings only. Never read or display the secret store.
pub fn show() -> String {
    let config = current();
    format!(
        "config: {}\nglobal_store={}\nmarkers={}\neditor={}\nagent_mode={}\n",
        config.path.display(),
        config.global_store.display(),
        config.markers.join(","),
        config
            .editor
            .as_deref()
            .unwrap_or("(VISUAL, EDITOR, or vi)"),
        if config.agent_mode_always {
            "always"
        } else {
            "auto"
        },
    )
}

/// Change a known setting, or remove it to restore the default. The caller
/// enforces human-only access using the configuration loaded before this write.
pub fn update(key: &str, value: Option<&str>) -> Result<(), String> {
    update_config(current(), key, value)
}

fn update_config(config: &Config, key: &str, value: Option<&str>) -> Result<(), String> {
    if !KEYS.contains(&key) {
        return Err(
            "unknown setting; allowed keys: global_store, markers, editor, agent_mode".to_string(),
        );
    }
    check_config_target(config)?;
    let text = read_config(&config.path)?.unwrap_or_default();
    // Revalidate an external edit before updating. Do not discard invalid or
    // unrecognized settings, and do not print their contents in the error.
    let existing = Config::parse(config.home.clone(), config.dir.clone(), &text)?;
    check_config_target(&existing)?;
    let value = if let Some(value) = value {
        if value.chars().any(char::is_control) {
            return Err("setting values must not contain control characters".to_string());
        }
        let value = value.trim();
        let mut next = existing.clone();
        next.apply(key, value)?;
        check_config_target(&next)?;
        let value = if key == "global_store" {
            encode_path(
                next.global_store
                    .to_str()
                    .ok_or("global_store must be UTF-8")?,
            )
        } else {
            value.to_string()
        };
        if value_and_comment(&value).0.trim() != value {
            return Err("setting value must not contain an unquoted inline comment".to_string());
        }
        Some(value)
    } else {
        None
    };
    let output = replace_setting(&text, key, value.as_deref());
    let next = Config::parse(config.home.clone(), config.dir.clone(), &output)?;
    check_config_target(&next)?;
    if output == text {
        return Ok(());
    }
    use std::os::unix::fs::DirBuilderExt;
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&config.dir)
        .map_err(|e| {
            format!(
                "cannot create config directory {}: {e}",
                config.dir.display()
            )
        })?;
    crate::guard::backup(&config.path).map_err(|e| format!("config backup failed: {e}"))?;
    crate::guard::atomic_write(&config.path, &output)
        .map_err(|e| format!("config write failed: {e}"))
}

fn check_config_target(config: &Config) -> Result<(), String> {
    use std::os::unix::fs::MetadataExt;
    // Config backups and staging files belong to the settings writer. A store
    // must never occupy one, including via a symlinked parent directory.
    let store_resolved = resolve_existing_parents(&config.global_store)?;
    let config_resolved = resolve_existing_parents(&config.path)?;
    if store_resolved.parent() == config_resolved.parent()
        && let Some(name) = store_resolved.file_name().and_then(|name| name.to_str())
        && ((name.starts_with("config.") && name.ends_with(".bak"))
            || (name.starts_with(".config.agents-env.") && name.ends_with(".tmp")))
    {
        return Err("global_store must not occupy a config backup or temporary file".to_string());
    }
    match fs::symlink_metadata(&config.path) {
        Ok(md) if md.file_type().is_symlink() || !md.is_file() || md.nlink() > 1 => {
            return Err("config must be a regular file without symlinks or hard links".to_string());
        }
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => {
            return Err(format!(
                "cannot inspect config {}: {e}",
                config.path.display()
            ));
        }
    }
    if resolve_existing_parents(&config.path)? == resolve_existing_parents(&config.global_store)?
        || same_file::is_same_file(&config.path, &config.global_store).unwrap_or(false)
    {
        return Err("config must not alias the global secret store".to_string());
    }
    Ok(())
}

// Resolve symlinked parents even before either file exists. Comparing only the
// complete files with same_file would miss an alias created by our own write.
fn resolve_existing_parents(path: &Path) -> Result<PathBuf, String> {
    let mut ancestor = path;
    let mut suffix = Vec::new();
    loop {
        match ancestor.canonicalize() {
            Ok(mut resolved) => {
                for name in suffix.into_iter().rev() {
                    resolved.push(name);
                }
                return Ok(resolved);
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                if let Some(name) = ancestor.file_name() {
                    suffix.push(name.to_os_string());
                }
                ancestor = ancestor
                    .parent()
                    .ok_or("cannot resolve config/store paths")?;
            }
            Err(e) => return Err(format!("cannot inspect config/store path: {e}")),
        }
    }
}

fn replace_setting(text: &str, key: &str, value: Option<&str>) -> String {
    let mut output = String::new();
    let mut found = false;
    for line in text.split_inclusive('\n') {
        let raw = line.trim_end_matches(['\r', '\n']);
        let is_key = raw.split_once('=').is_some_and(|(k, _)| k.trim() == key);
        if !is_key {
            output.push_str(line);
            continue;
        }
        found = true;
        let (prefix, old) = raw.split_once('=').expect("matched setting");
        let comment = value_and_comment(old).1;
        if let Some(value) = value {
            output.push_str(prefix);
            output.push('=');
            output.push_str(value);
            output.push_str(comment);
            output.push_str(if line.ends_with("\r\n") { "\r\n" } else { "\n" });
        } else if !comment.is_empty() {
            output.push_str(comment.trim_start());
            output.push('\n');
        }
    }
    if !found && let Some(value) = value {
        if !output.is_empty() && !output.ends_with('\n') {
            output.push('\n');
        }
        output.push_str(key);
        output.push('=');
        output.push_str(value);
        output.push('\n');
    }
    output
}

// A whitespace-prefixed # outside quotes begins an inline comment. Preserve
// its leading whitespace when replacing a setting.
fn value_and_comment(value: &str) -> (&str, &str) {
    let mut quote = None;
    let mut escaped = false;
    for (index, ch) in value.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' && quote != Some('\'') {
            escaped = true;
        } else if Some(ch) == quote {
            quote = None;
        } else if quote.is_none() && (ch == '\'' || ch == '"') {
            quote = Some(ch);
        } else if quote.is_none()
            && ch == '#'
            && (index == 0
                || value[..index]
                    .chars()
                    .next_back()
                    .is_some_and(char::is_whitespace))
        {
            let start = value[..index].trim_end().len();
            return (&value[..start], &value[start..]);
        }
    }
    (value, "")
}

fn normalize(path: &Path) -> PathBuf {
    let mut output = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                output.pop();
            }
            _ => output.push(component.as_os_str()),
        }
    }
    output
}

// Only paths use scalar quoting. Editor values remain command strings whose
// quoted arguments are parsed by the editor launcher, never by a shell.
fn encode_path(value: &str) -> String {
    if value.contains(['\'', '"', '\\']) || value_and_comment(value).0 != value {
        format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        value.to_string()
    }
}

fn decode_path(value: &str) -> Result<String, String> {
    if let Some(inner) = value.strip_prefix('\'') {
        let inner = inner
            .strip_suffix('\'')
            .ok_or("global_store has an unterminated quoted path")?;
        if inner.contains('\'') {
            return Err("global_store has an invalid quoted path".to_string());
        }
        return Ok(inner.to_string());
    }
    if let Some(inner) = value.strip_prefix('"') {
        let inner = inner
            .strip_suffix('"')
            .ok_or("global_store has an unterminated quoted path")?;
        let mut output = String::new();
        let mut chars = inner.chars();
        while let Some(ch) = chars.next() {
            if ch == '\\' {
                let escaped = chars
                    .next()
                    .ok_or("global_store has an invalid path escape")?;
                if escaped != '\\' && escaped != '"' {
                    return Err(
                        "global_store path escapes must be a backslash or quote".to_string()
                    );
                }
                output.push(escaped);
            } else if ch == '"' {
                return Err("global_store has an invalid quoted path".to_string());
            } else {
                output.push(ch);
            }
        }
        Ok(output)
    } else {
        Ok(value.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn parse(text: &str) -> Result<Config, String> {
        Config::parse(
            PathBuf::from("/home/user"),
            PathBuf::from("/home/user/.config/agents-env"),
            text,
        )
    }

    #[test]
    fn defaults_and_paths_are_absolute_and_cwd_independent() {
        let config = parse("").unwrap();
        assert_eq!(
            config.global_store,
            Path::new("/home/user/.config/agents-env/global.env")
        );
        assert!(config.markers.is_empty());
        assert!(config.editor.is_none());
        assert!(!config.agent_mode_always);
        assert_eq!(
            parse("global_store=~/secrets/.env\n").unwrap().global_store,
            Path::new("/home/user/secrets/.env")
        );
        assert_eq!(
            parse("global_store=../secrets/.env\n")
                .unwrap()
                .global_store,
            Path::new("/home/user/.config/secrets/.env")
        );
    }

    #[test]
    fn known_settings_and_inline_comments() {
        let config = parse(
            "# settings\nmarkers= A,_B,A # extra signals\neditor=code --wait\nagent_mode=always\n",
        )
        .unwrap();
        assert_eq!(config.markers, ["A", "_B"]);
        assert_eq!(config.editor.as_deref(), Some("code --wait"));
        assert!(config.agent_mode_always);
        assert_eq!(
            value_and_comment("vi '+set #local' # note"),
            ("vi '+set #local'", " # note")
        );
        assert_eq!(
            parse("global_store=\"~/secrets # one/.env\" # note\n")
                .unwrap()
                .global_store,
            Path::new("/home/user/secrets # one/.env")
        );
        let path = "/home/user/a # path/with \\\"quote";
        assert_eq!(decode_path(&encode_path(path)).unwrap(), path);
    }

    #[test]
    fn invalid_config_errors_never_echo_values() {
        for text in [
            "password=do-not-echo",
            "editor=\n",
            "global_store=\n",
            "markers=9BAD\n",
            "agent_mode=never\n",
            "markers=A\nmarkers=B\n",
            "invalid-do-not-echo",
        ] {
            let error = parse(text).unwrap_err();
            assert!(error.contains("line "));
            assert!(!error.contains("do-not-echo"));
        }
        assert!(parse("editor=vi\u{0}bad\n").is_err());
    }

    #[test]
    fn xdg_selection_preserves_existing_legacy_config() {
        let home = TempDir::new().unwrap();
        let xdg = TempDir::new().unwrap();
        let legacy = home.path().join(".config/agents-env");
        fs::create_dir_all(&legacy).unwrap();
        fs::write(legacy.join("config"), "global_store=legacy.env\n").unwrap();
        let config = Config::load_with(home.path().into(), Some(xdg.path().into())).unwrap();
        assert_eq!(config.dir, legacy);
        assert_eq!(config.global_store, legacy.join("legacy.env"));
        let new_dir = xdg.path().join("agents-env");
        fs::create_dir_all(&new_dir).unwrap();
        fs::write(new_dir.join("config"), "agent_mode=always\n").unwrap();
        let config = Config::load_with(home.path().into(), Some(xdg.path().into())).unwrap();
        assert_eq!(config.dir, new_dir);
        assert!(config.agent_mode_always);
    }

    #[test]
    fn xdg_defaults_and_relative_xdg_are_deterministic() {
        let home = TempDir::new().unwrap();
        let xdg = TempDir::new().unwrap();
        let config = Config::load_with(home.path().into(), Some(xdg.path().into())).unwrap();
        assert_eq!(
            config.global_store,
            xdg.path().join("agents-env/global.env")
        );
        let config = Config::load_with(home.path().into(), Some("relative".into())).unwrap();
        assert_eq!(config.dir, home.path().join(".config/agents-env"));
        for home in [PathBuf::new(), PathBuf::from("relative")] {
            assert!(Config::load_with(home, None).unwrap_err().contains("HOME"));
        }
    }

    #[test]
    fn invalid_xdg_config_does_not_fall_back() {
        let home = TempDir::new().unwrap();
        let xdg = TempDir::new().unwrap();
        fs::create_dir_all(xdg.path().join("agents-env")).unwrap();
        fs::write(xdg.path().join("agents-env/config"), [0xff]).unwrap();
        assert!(Config::load_with(home.path().into(), Some(xdg.path().into())).is_err());
    }

    #[test]
    fn setting_replacement_preserves_comments_and_other_lines() {
        let text = "# header\nmarkers = OLD # user note\neditor=code --wait\n";
        assert_eq!(
            replace_setting(text, "markers", Some("NEW")),
            "# header\nmarkers =NEW # user note\neditor=code --wait\n"
        );
        assert_eq!(
            replace_setting(text, "markers", None),
            "# header\n# user note\neditor=code --wait\n"
        );
        assert_eq!(
            replace_setting("# header", "agent_mode", Some("always")),
            "# header\nagent_mode=always\n"
        );
    }

    #[test]
    fn update_only_changes_config_and_protects_aliases() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let home = TempDir::new().unwrap();
        let config = Config::load_with(home.path().into(), None).unwrap();
        update_config(&config, "agent_mode", Some("always")).unwrap();
        assert!(!config.global_store.exists());
        assert_eq!(
            fs::metadata(&config.dir).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(&config.path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let original = fs::read_to_string(&config.path).unwrap();
        assert!(update_config(&config, "global_store", Some("./config")).is_err());
        assert!(update_config(&config, "editor", Some("vi\nagent_mode=auto")).is_err());
        assert_eq!(fs::read_to_string(&config.path).unwrap(), original);
        let store_link = config.dir.join("store-link");
        symlink(&config.path, &store_link).unwrap();
        assert!(
            update_config(&config, "global_store", Some(store_link.to_str().unwrap())).is_err()
        );
        let linked = config.dir.join("config-link");
        fs::hard_link(&config.path, &linked).unwrap();
        assert!(update_config(&config, "markers", Some("TEST_AGENT")).is_err());
    }

    #[test]
    fn missing_config_cannot_become_store_via_symlinked_parent() {
        use std::os::unix::fs::symlink;
        let home = TempDir::new().unwrap();
        let config = Config::load_with(home.path().into(), None).unwrap();
        fs::create_dir_all(&config.dir).unwrap();
        let alias = home.path().join("alias");
        symlink(&config.dir, &alias).unwrap();
        assert!(
            update_config(
                &config,
                "global_store",
                Some(alias.join("config").to_str().unwrap())
            )
            .is_err()
        );
        assert!(!config.path.exists());
    }
}
