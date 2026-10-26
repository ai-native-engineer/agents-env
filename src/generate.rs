//! The only agent-accessible global writer: add a new internally generated key.
//! Existing bytes are preserved, ambiguous/malformed input is refused, and all
//! file operations stay relative to an opened directory across atomic replacement.

use crate::{config, guard, store};
use std::ffi::{CString, OsStr};
use std::fs::{self, File, Metadata};
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

type Failure = (i32, String);
type Result<T> = std::result::Result<T, Failure>;

fn refused(message: impl Into<String>) -> Failure {
    (2, message.into())
}

fn io_error(operation: &str, error: io::Error) -> Failure {
    (1, format!("{operation}: {error}"))
}

fn regular_private_inode(metadata: &Metadata) -> Result<()> {
    if !metadata.is_file() || metadata.nlink() != 1 || metadata.uid() != unsafe { libc::geteuid() }
    {
        return Err(refused(
            "store, backup and lock must be owned regular files with one hard link",
        ));
    }
    Ok(())
}

struct Directory {
    file: File,
    path: PathBuf,
}

impl Directory {
    fn open(path: &Path) -> Result<Self> {
        let path = path
            .canonicalize()
            .map_err(|e| io_error("cannot resolve store directory", e))?;
        let file = fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&path)
            .map_err(|e| io_error("cannot open store directory", e))?;
        Ok(Self { file, path })
    }

    fn open_file(&self, name: &str, flags: i32) -> io::Result<File> {
        let name = CString::new(name).map_err(|_| io::Error::other("invalid store file name"))?;
        let fd = unsafe {
            libc::openat(
                self.file.as_raw_fd(),
                name.as_ptr(),
                flags | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
                0o600,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let file = unsafe { File::from_raw_fd(fd) };
        if flags & libc::O_CREAT != 0 && flags & libc::O_EXCL != 0 {
            // A restrictive caller umask must not publish an unreadable store.
            file.set_permissions(fs::Permissions::from_mode(0o600))?;
        }
        Ok(file)
    }

    fn remove_file(&self, name: &str) -> io::Result<()> {
        let name = CString::new(name).map_err(|_| io::Error::other("invalid store file name"))?;
        if unsafe { libc::unlinkat(self.file.as_raw_fd(), name.as_ptr(), 0) } < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    fn persist(&self, temporary: &str, name: &str, replace: bool) -> io::Result<()> {
        let from =
            CString::new(temporary).map_err(|_| io::Error::other("invalid temporary file name"))?;
        let to = CString::new(name).map_err(|_| io::Error::other("invalid store file name"))?;
        let fd = self.file.as_raw_fd();
        let result = if replace {
            unsafe { libc::renameat(fd, from.as_ptr(), fd, to.as_ptr()) }
        } else {
            // Publishing a missing store must never overwrite one created in the meantime.
            unsafe { libc::linkat(fd, from.as_ptr(), fd, to.as_ptr(), 0) }
        };
        if result < 0 {
            return Err(io::Error::last_os_error());
        }
        if !replace {
            self.remove_file(temporary)?;
        }
        self.file.sync_all()
    }

    fn still_at(&self, parent: &Path) -> Result<()> {
        let current =
            fs::metadata(parent).map_err(|e| io_error("cannot recheck store directory", e))?;
        let opened = self
            .file
            .metadata()
            .map_err(|e| io_error("cannot inspect store directory", e))?;
        if current.dev() != opened.dev() || current.ino() != opened.ino() {
            return Err(refused("store directory changed; no key was added"));
        }
        Ok(())
    }
}

struct Snapshot {
    text: String,
    metadata: Metadata,
}

fn snapshot(directory: &Directory, name: &str) -> Result<Option<Snapshot>> {
    let mut file = match directory.open_file(name, libc::O_RDONLY) {
        Ok(file) => file,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(_) => {
            return Err(refused(
                "cannot safely open global store; check file type, links and permissions",
            ));
        }
    };
    let metadata = file
        .metadata()
        .map_err(|e| io_error("cannot inspect global store", e))?;
    regular_private_inode(&metadata)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|e| io_error("cannot read global store", e))?;
    let text = String::from_utf8(bytes)
        .map_err(|_| refused("global store is not valid UTF-8; no key was added"))?;
    Ok(Some(Snapshot { text, metadata }))
}

fn unchanged(directory: &Directory, name: &str, previous: Option<&Snapshot>) -> Result<()> {
    let current = snapshot(directory, name)?;
    let matches = match (previous, current.as_ref()) {
        (None, None) => true,
        (Some(old), Some(new)) => {
            old.text == new.text
                && old.metadata.dev() == new.metadata.dev()
                && old.metadata.ino() == new.metadata.ino()
                && old.metadata.mtime() == new.metadata.mtime()
                && old.metadata.mtime_nsec() == new.metadata.mtime_nsec()
                && old.metadata.ctime() == new.metadata.ctime()
                && old.metadata.ctime_nsec() == new.metadata.ctime_nsec()
        }
        _ => false,
    };
    if !matches {
        return Err(refused(
            "global store changed during generation; no key was added",
        ));
    }
    Ok(())
}

/// Held across both generator transactions and human editor sessions. The stable
/// sidecar inode is never unlinked, so two waiters cannot lock different files.
pub(crate) struct StoreLock {
    directory: Directory,
    _file: File,
    name: String,
    parent: PathBuf,
}

pub(crate) fn lock_store() -> Result<StoreLock> {
    config::check_config_target(config::current()).map_err(refused)?;
    let target = config::global_store();
    let parent = target
        .parent()
        .ok_or_else(|| refused("global store must have a parent directory"))?
        .to_path_buf();
    let name = target
        .file_name()
        .and_then(OsStr::to_str)
        .ok_or_else(|| refused("global store must have a UTF-8 file name"))?
        .to_string();
    if name.ends_with(".bak") || name.ends_with(".agents-env.lock") || name.ends_with(".tmp") {
        return Err(refused(
            "global store must not be a backup, lock or temporary file",
        ));
    }
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&parent)
        .map_err(|e| io_error("cannot create store directory", e))?;
    let directory = Directory::open(&parent)?;
    let lock_name = format!("{name}.agents-env.lock");
    let file = match directory.open_file(&lock_name, libc::O_RDWR | libc::O_CREAT | libc::O_EXCL) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => directory
            .open_file(&lock_name, libc::O_RDWR)
            .map_err(|_| refused("cannot safely open global store lock"))?,
        Err(_) => return Err(refused("cannot safely create global store lock")),
    };
    let metadata = file
        .metadata()
        .map_err(|e| io_error("cannot inspect global store lock", e))?;
    regular_private_inode(&metadata)?;
    if metadata.len() != 0 {
        return Err(refused("global store lock must be an empty file"));
    }
    if metadata.mode() & 0o777 != 0o600 {
        return Err(refused("global store lock must be private (0600)"));
    }
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } < 0 {
        return Err(refused(
            "global store is busy; retry after the other writer or editor finishes",
        ));
    }
    directory.still_at(&parent)?;
    Ok(StoreLock {
        directory,
        _file: file,
        name,
        parent,
    })
}

fn candidate(text: &str, path: &Path, key: &str, value: &str) -> Result<String> {
    if text.contains('\0') {
        return Err(refused(
            "global store contains a NUL byte; no key was added",
        ));
    }
    let original = store::EnvFile::parse(path, text);
    if let Some(line) = original.unparseable_lines().first() {
        return Err(refused(format!(
            "global store line {} is not parseable; no key was added",
            line + 1
        )));
    }
    if original
        .entries()
        .any(|(_, entry)| entry.key.eq_ignore_ascii_case(key))
    {
        return Err(refused(format!(
            "{key} already exists (including tagged or case-variant entries); no key was added"
        )));
    }
    let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let mut output = text.to_string();
    if !output.is_empty() && !output.ends_with('\n') {
        output.push_str(if output.ends_with('\r') {
            "\n"
        } else {
            newline
        });
    }
    output.push_str(&format!(
        "{key}=\"{value}\" # generated by agents-env{newline}"
    ));
    let parsed = store::EnvFile::parse(path, &output);
    let old_count = original.entries().count();
    let preserved = original
        .entries()
        .zip(parsed.entries())
        .all(|((_, old), (_, new))| {
            old.key == new.key && old.value == new.value && old.comment == new.comment
        });
    if !parsed.unparseable_lines().is_empty()
        || !preserved
        || parsed.entries().count() != old_count + 1
        || !parsed
            .select(key)
            .is_ok_and(|(_, entry)| entry.key == key && entry.value == value)
    {
        return Err(refused(
            "generated entry failed lookup verification; no key was added",
        ));
    }
    Ok(output)
}

fn backup(directory: &Directory, name: &str, previous: &Snapshot) -> Result<()> {
    let name = format!("{name}.{}.bak", guard::yymmdd());
    match directory.open_file(&name, libc::O_RDONLY) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return publish_backup(directory, &name, previous);
        }
        Err(_) => return Err(refused("existing backup is not safe")),
    }
    .metadata()
    .map_err(|e| io_error("cannot inspect backup", e))
    .and_then(|metadata| {
        regular_private_inode(&metadata)?;
        if metadata.mode() & 0o777 != 0o600 {
            return Err(refused("existing backup is not private; no key was added"));
        }
        Ok(())
    })
}

fn publish_backup(directory: &Directory, name: &str, previous: &Snapshot) -> Result<()> {
    let temporary = format!("{name}.agents-env.{}.tmp", std::process::id());
    guard::git_secret_check(&directory.path, &temporary).map_err(refused)?;
    let mut file = directory
        .open_file(&temporary, libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL)
        .map_err(|_| refused("cannot safely create private backup staging file"))?;
    let result = file
        .write_all(previous.text.as_bytes())
        .and_then(|_| file.sync_all())
        .and_then(|_| directory.persist(&temporary, name, false));
    drop(file);
    let _ = directory.remove_file(&temporary);
    result.map_err(|e| io_error("cannot publish global store backup", e))
}

pub fn create(key: &str) -> Result<()> {
    store::validate_key(key).map_err(|message| (3, message))?;
    let locked = lock_store()?;
    let directory = &locked.directory;
    let path = directory.path.join(&locked.name);
    guard::git_secret_check(&directory.path, &locked.name).map_err(refused)?;
    let previous = snapshot(directory, &locked.name)?;
    let text = previous
        .as_ref()
        .map_or("", |snapshot| snapshot.text.as_str());
    // Validate input and collisions before requesting randomness or creating a backup.
    candidate(text, &path, key, "validation-only")?;
    guard::git_backup_check(&path).map_err(refused)?;
    let temporary = format!("{}.agents-env.{}.tmp", locked.name, std::process::id());
    guard::git_secret_check(&directory.path, &temporary).map_err(refused)?;
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|_| {
        (
            1,
            "operating system randomness failed; no key was added".to_string(),
        )
    })?;
    let mut value = String::with_capacity(64);
    const HEX: &[u8] = b"0123456789abcdef";
    for byte in bytes {
        value.push(HEX[(byte >> 4) as usize] as char);
        value.push(HEX[(byte & 15) as usize] as char);
    }
    let output = candidate(text, &path, key, &value)?;
    let mut file = directory
        .open_file(&temporary, libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL)
        .map_err(|_| refused("cannot safely create private staging file"))?;
    let transaction = (|| {
        file.write_all(output.as_bytes())
            .and_then(|_| file.sync_all())
            .map_err(|e| io_error("cannot stage generated key", e))?;
        directory.still_at(&locked.parent)?;
        unchanged(directory, &locked.name, previous.as_ref())?;
        if let Some(previous) = previous.as_ref() {
            backup(directory, &locked.name, previous)?;
        }
        unchanged(directory, &locked.name, previous.as_ref())?;
        directory
            .persist(&temporary, &locked.name, previous.is_some())
            .map_err(|e| io_error("cannot publish generated key", e))?;
        let saved = snapshot(directory, &locked.name)?
            .ok_or_else(|| refused("cannot verify persisted generated key"))?;
        if saved.text != output || saved.metadata.mode() & 0o777 != 0o600 {
            return Err(refused(
                "persisted key verification failed; inspect the store before retrying",
            ));
        }
        Ok(())
    })();
    drop(file);
    let _ = directory.remove_file(&temporary);
    transaction
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changed_or_replaced_store_is_detected_before_publishing() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("global.env");
        fs::write(&path, "KEEP=fixture-original\n").unwrap();
        let directory = Directory::open(root.path()).unwrap();
        let before = snapshot(&directory, "global.env").unwrap().unwrap();
        fs::write(&path, "KEEP=fixture-edited\n").unwrap();
        assert_eq!(
            unchanged(&directory, "global.env", Some(&before))
                .unwrap_err()
                .0,
            2
        );
        let replacement = root.path().join("replacement");
        fs::write(&replacement, &before.text).unwrap();
        fs::rename(replacement, &path).unwrap();
        assert_eq!(
            unchanged(&directory, "global.env", Some(&before))
                .unwrap_err()
                .0,
            2
        );
        assert_eq!(fs::read_to_string(path).unwrap(), before.text);
    }

    #[test]
    fn replacing_parent_directory_is_detected() {
        let root = tempfile::tempdir().unwrap();
        let parent = root.path().join("store");
        fs::create_dir(&parent).unwrap();
        let directory = Directory::open(&parent).unwrap();
        fs::rename(&parent, root.path().join("moved")).unwrap();
        fs::create_dir(&parent).unwrap();
        assert_eq!(directory.still_at(&parent).unwrap_err().0, 2);
    }
}
