//! Secret-store contracts shared by file and native backends.
//!
//! Metadata and values are separate types on purpose. Commands that discover
//! keys can never receive a value accidentally, while value-bearing commands
//! can still preserve the file backend's raw representation for `copy`.

use std::path::Path;

use crate::store::{EnvFile, SelectError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretMetadata {
    key: String,
    tag: Option<String>,
    value_len: Option<usize>,
}

impl SecretMetadata {
    pub fn new(key: String, tag: Option<String>, value_len: Option<usize>) -> Self {
        Self {
            key,
            tag,
            value_len,
        }
    }

    pub fn key(&self) -> &str {
        &self.key
    }

    pub fn tag(&self) -> Option<&str> {
        self.tag.as_deref()
    }

    pub fn value_len(&self) -> Option<usize> {
        self.value_len
    }
}

/// A resolved secret. This type intentionally does not implement `Debug` so a
/// generic diagnostic cannot accidentally print its value.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret {
    metadata: SecretMetadata,
    value: String,
    raw_value: Option<String>,
}

impl Secret {
    pub fn new(metadata: SecretMetadata, value: String, raw_value: Option<String>) -> Self {
        Self {
            metadata,
            value,
            raw_value,
        }
    }

    pub fn key(&self) -> &str {
        self.metadata.key()
    }

    pub fn tag(&self) -> Option<&str> {
        self.metadata.tag()
    }

    pub fn value(&self) -> &str {
        &self.value
    }

    /// Exact source representation when the backend has one, such as a
    /// quoted `.env` value. Native stores return `None` and callers must quote
    /// the decoded value before writing it to a local env file.
    pub fn raw_value(&self) -> Option<&str> {
        self.raw_value.as_deref()
    }

    pub fn metadata(&self) -> &SecretMetadata {
        &self.metadata
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreError {
    NotFound(String),
    Ambiguous { key: String, tags: Vec<String> },
    Unavailable(String),
    Invalid(String),
}

impl From<SelectError> for StoreError {
    fn from(error: SelectError) -> Self {
        match error {
            SelectError::NotFound(selector) => Self::NotFound(selector),
            SelectError::Ambiguous { key, tags } => Self::Ambiguous { key, tags },
        }
    }
}

pub trait SecretStore {
    /// Return metadata only. Implementations must not load secret bytes for
    /// discovery unless the backend cannot enumerate attributes otherwise.
    fn metadata(&self, pattern: Option<&str>) -> Result<Vec<SecretMetadata>, StoreError>;

    /// Resolve one selector and return its value in memory.
    fn resolve(&self, selector: &str) -> Result<Secret, StoreError>;

    /// Resolve every entry for ambient masking or an explicit export.
    fn resolve_all(&self) -> Result<Vec<Secret>, StoreError>;
}

pub struct FileStore {
    file: EnvFile,
}

impl FileStore {
    pub fn load(path: &Path) -> std::io::Result<Self> {
        Ok(Self {
            file: EnvFile::load(path)?,
        })
    }

    pub fn from_env_file(file: EnvFile) -> Self {
        Self { file }
    }

    pub fn file(&self) -> &EnvFile {
        &self.file
    }

    fn metadata_for(entry: &crate::store::Entry) -> SecretMetadata {
        SecretMetadata::new(
            entry.key.clone(),
            entry.comment.clone(),
            Some(entry.value.chars().count()),
        )
    }

    fn secret_for(&self, index: usize, entry: &crate::store::Entry) -> Secret {
        let metadata = Self::metadata_for(entry);
        Secret::new(
            metadata,
            entry.value.clone(),
            Some(self.file.value_raw(index).to_string()),
        )
    }
}

impl SecretStore for FileStore {
    fn metadata(&self, pattern: Option<&str>) -> Result<Vec<SecretMetadata>, StoreError> {
        let entries = self.file.search(pattern.unwrap_or(""));
        Ok(entries
            .into_iter()
            .map(|(_, entry)| Self::metadata_for(entry))
            .collect())
    }

    fn resolve(&self, selector: &str) -> Result<Secret, StoreError> {
        let (index, entry) = self.file.select(selector).map_err(StoreError::from)?;
        Ok(self.secret_for(index, entry))
    }

    fn resolve_all(&self) -> Result<Vec<Secret>, StoreError> {
        Ok(self
            .file
            .entries()
            .map(|(index, entry)| self.secret_for(index, entry))
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct MemoryStore {
        entries: Vec<(String, Option<String>, String)>,
    }

    impl MemoryStore {
        fn fixture() -> Self {
            Self {
                entries: vec![
                    (
                        "API_KEY".into(),
                        Some("# personal".into()),
                        "personal-secret".into(),
                    ),
                    (
                        "API_KEY".into(),
                        Some("# work".into()),
                        "work-secret".into(),
                    ),
                    ("PLAIN".into(), None, "plain-secret".into()),
                ],
            }
        }
    }

    impl SecretStore for MemoryStore {
        fn metadata(&self, pattern: Option<&str>) -> Result<Vec<SecretMetadata>, StoreError> {
            let pattern = pattern.unwrap_or("").to_ascii_lowercase();
            Ok(self
                .entries
                .iter()
                .filter(|(key, _, _)| key.to_ascii_lowercase().contains(&pattern))
                .map(|(key, tag, value)| {
                    SecretMetadata::new(key.clone(), tag.clone(), Some(value.chars().count()))
                })
                .collect())
        }

        fn resolve(&self, selector: &str) -> Result<Secret, StoreError> {
            let (key, tag) = selector
                .split_once('@')
                .map_or((selector, None), |(key, tag)| (key, Some(tag)));
            let matches: Vec<_> = self
                .entries
                .iter()
                .filter(|(candidate, candidate_tag, _)| {
                    candidate.eq_ignore_ascii_case(key)
                        && tag.is_none_or(|wanted| {
                            candidate_tag
                                .as_deref()
                                .unwrap_or("")
                                .to_ascii_lowercase()
                                .contains(&wanted.to_ascii_lowercase())
                        })
                })
                .collect();
            match matches.as_slice() {
                [] => Err(StoreError::NotFound(selector.to_string())),
                [(key, tag, value)] => Ok(Secret::new(
                    SecretMetadata::new(key.clone(), tag.clone(), Some(value.chars().count())),
                    value.clone(),
                    None,
                )),
                _ => Err(StoreError::Ambiguous {
                    key: key.to_string(),
                    tags: matches
                        .iter()
                        .map(|(_, tag, _)| tag.clone().unwrap_or_else(|| "(no tag)".into()))
                        .collect(),
                }),
            }
        }

        fn resolve_all(&self) -> Result<Vec<Secret>, StoreError> {
            self.entries
                .iter()
                .map(|(key, tag, value)| {
                    Ok(Secret::new(
                        SecretMetadata::new(key.clone(), tag.clone(), Some(value.chars().count())),
                        value.clone(),
                        None,
                    ))
                })
                .collect()
        }
    }

    fn assert_contract(store: &impl SecretStore) {
        let metadata = store.metadata(Some("api")).unwrap();
        assert_eq!(metadata.len(), 2);
        assert_eq!(metadata[0].key(), "API_KEY");
        assert_eq!(metadata[1].tag(), Some("# work"));
        assert_eq!(
            metadata[0].value_len(),
            Some("personal-secret".chars().count())
        );

        let resolved = store.resolve("API_KEY@work").unwrap();
        assert_eq!(resolved.key(), "API_KEY");
        assert_eq!(resolved.tag(), Some("# work"));
        assert_eq!(resolved.value(), "work-secret");
        assert!(resolved.raw_value().is_none());

        assert!(matches!(
            store.resolve("API_KEY"),
            Err(StoreError::Ambiguous { .. })
        ));
        assert!(matches!(
            store.resolve("MISSING"),
            Err(StoreError::NotFound(_))
        ));
    }

    #[test]
    fn fake_provider_contract_separates_metadata_and_values() {
        assert_contract(&MemoryStore::fixture());
    }

    #[test]
    fn file_store_preserves_raw_value_for_copy() {
        let file = EnvFile::parse(
            Path::new("fixture.env"),
            "API_KEY=\"work-secret\" # work\nPLAIN=plain-secret\n",
        );
        let store = FileStore::from_env_file(file);
        let resolved = store.resolve("API_KEY@work").unwrap();
        assert_eq!(resolved.value(), "work-secret");
        assert_eq!(resolved.raw_value(), Some("\"work-secret\""));
    }

    #[test]
    fn metadata_debug_does_not_contain_values() {
        let store = MemoryStore::fixture();
        let debug = format!("{:?}", store.metadata(None).unwrap());
        assert!(!debug.contains("personal-secret"));
        assert!(!debug.contains("work-secret"));
    }

    #[test]
    fn backend_failure_is_value_free() {
        let error = StoreError::Unavailable("keychain is unavailable".into());
        let debug = format!("{error:?}");
        assert!(debug.contains("unavailable"));
        assert!(!debug.contains("secret"));
    }
}
