//! Line-preserving .env file model.
//!
//! Files are parsed line by line and every byte that is not a value being
//! replaced survives a round trip: comments, blank lines, ordering, inline
//! `# tag` comments and quoting style of untouched lines all stay intact.

use std::fs;
use std::io;
use std::ops::Range;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Entry {
    pub key: String,
    /// Decoded value (quotes removed, escapes resolved).
    pub value: String,
    /// Byte range of the value text (including quotes) within the raw line.
    pub value_span: Range<usize>,
    /// Inline comment including the leading `#`, used as the account tag.
    pub comment: Option<String>,
}

#[derive(Debug)]
pub struct Line {
    pub raw: String,
    pub entry: Option<Entry>,
}

#[derive(Debug)]
pub struct EnvFile {
    pub path: PathBuf,
    pub lines: Vec<Line>,
}

#[derive(Debug)]
pub enum SelectError {
    NotFound(String),
    Ambiguous { key: String, tags: Vec<String> },
}

impl EnvFile {
    pub fn load(path: &Path) -> io::Result<EnvFile> {
        let text = fs::read_to_string(path)?;
        Ok(Self::parse(path, &text))
    }

    pub fn load_or_empty(path: &Path) -> io::Result<EnvFile> {
        match fs::read_to_string(path) {
            Ok(text) => Ok(Self::parse(path, &text)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(EnvFile {
                path: path.to_path_buf(),
                lines: Vec::new(),
            }),
            Err(e) => Err(e),
        }
    }

    pub fn parse(path: &Path, text: &str) -> EnvFile {
        let lines = text
            .lines()
            .map(|l| Line {
                raw: l.to_string(),
                entry: parse_entry(l),
            })
            .collect();
        EnvFile {
            path: path.to_path_buf(),
            lines,
        }
    }

    pub fn entries(&self) -> impl Iterator<Item = (usize, &Entry)> {
        self.lines
            .iter()
            .enumerate()
            .filter_map(|(i, l)| l.entry.as_ref().map(|e| (i, e)))
    }

    /// Case-insensitive substring match on key names.
    pub fn search(&self, pattern: &str) -> Vec<(usize, &Entry)> {
        let p = pattern.to_ascii_lowercase();
        self.entries()
            .filter(|(_, e)| e.key.to_ascii_lowercase().contains(&p))
            .collect()
    }

    /// Resolve a `KEY` / `KEY@tag` selector to exactly one entry.
    /// The key half is matched exactly (case-insensitive); the tag half is a
    /// case-insensitive substring match against the inline comment.
    pub fn select(&self, selector: &str) -> Result<(usize, &Entry), SelectError> {
        let (key, tag) = match selector.split_once('@') {
            Some((k, t)) => (k, Some(t)),
            None => (selector, None),
        };
        let mut candidates: Vec<(usize, &Entry)> = self
            .entries()
            .filter(|(_, e)| e.key.eq_ignore_ascii_case(key))
            .collect();
        if let Some(tag) = tag {
            let t = tag.to_ascii_lowercase();
            candidates.retain(|(_, e)| {
                e.comment
                    .as_deref()
                    .unwrap_or("")
                    .to_ascii_lowercase()
                    .contains(&t)
            });
        }
        match candidates.len() {
            0 => Err(SelectError::NotFound(selector.to_string())),
            1 => Ok(candidates[0]),
            _ => Err(SelectError::Ambiguous {
                key: key.to_string(),
                tags: candidates
                    .iter()
                    .map(|(_, e)| e.comment.clone().unwrap_or_else(|| "(no tag)".into()))
                    .collect(),
            }),
        }
    }

    /// Exact value text (including quotes) of the entry at `idx`.
    pub fn value_raw(&self, idx: usize) -> &str {
        let line = &self.lines[idx];
        let e = line.entry.as_ref().expect("line has no entry");
        &line.raw[e.value_span.clone()]
    }

    /// Replace only the value of the entry at `idx`; every other byte of the
    /// line (key, spacing, inline comment) is preserved.
    pub fn replace_value(&mut self, idx: usize, new_value_raw: &str) {
        let line = &mut self.lines[idx];
        let span = line
            .entry
            .as_ref()
            .expect("line has no entry")
            .value_span
            .clone();
        let mut raw = line.raw.clone();
        raw.replace_range(span, new_value_raw);
        line.entry = parse_entry(&raw);
        line.raw = raw;
    }

    pub fn append(&mut self, key: &str, value_raw: &str, comment: Option<&str>) {
        let raw = match comment {
            Some(c) => format!("{key}={value_raw} {c}"),
            None => format!("{key}={value_raw}"),
        };
        self.lines.push(Line {
            entry: parse_entry(&raw),
            raw,
        });
    }

    /// Line indices where `key` appears (exact, case-sensitive — write side).
    pub fn occurrences(&self, key: &str) -> Vec<usize> {
        self.entries()
            .filter(|(_, e)| e.key == key)
            .map(|(i, _)| i)
            .collect()
    }

    /// Lines that are neither blank, full-line comments, nor parseable entries.
    pub fn unparseable_lines(&self) -> Vec<usize> {
        self.lines
            .iter()
            .enumerate()
            .filter(|(_, l)| {
                l.entry.is_none()
                    && !l.raw.trim().is_empty()
                    && !l.raw.trim_start().starts_with('#')
            })
            .map(|(i, _)| i)
            .collect()
    }

    pub fn serialize(&self) -> String {
        let mut out = String::new();
        for l in &self.lines {
            out.push_str(&l.raw);
            out.push('\n');
        }
        out
    }
}

/// Quote a literal value for writing: always double quotes, with `\`, `"`,
/// newline, carriage return and tab escaped so a value can never break the
/// one-key-per-line file structure on the next parse.
pub fn quote_value(v: &str) -> String {
    let escaped = v
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t");
    format!("\"{escaped}\"")
}

fn parse_entry(raw: &str) -> Option<Entry> {
    let bytes = raw.as_bytes();
    let mut i = 0;
    while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t') {
        i += 1;
    }
    if i >= bytes.len() || bytes[i] == b'#' {
        return None;
    }
    if raw[i..].starts_with("export ") || raw[i..].starts_with("export\t") {
        i += 7;
        while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t') {
            i += 1;
        }
    }
    let key_start = i;
    if i >= bytes.len() || !(bytes[i].is_ascii_alphabetic() || bytes[i] == b'_') {
        return None;
    }
    while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
        i += 1;
    }
    let key = raw[key_start..i].to_string();
    if i >= bytes.len() || bytes[i] != b'=' {
        return None;
    }
    i += 1;
    let vstart = i;

    let (value, vend) = if i < bytes.len() && bytes[i] == b'"' {
        let mut j = i + 1;
        let mut val = String::new();
        let mut closed = false;
        while j < bytes.len() {
            match bytes[j] {
                b'\\' if j + 1 < bytes.len() => {
                    match bytes[j + 1] {
                        b'"' => {
                            val.push('"');
                            j += 2;
                        }
                        b'\\' => {
                            val.push('\\');
                            j += 2;
                        }
                        b'n' => {
                            val.push('\n');
                            j += 2;
                        }
                        b'r' => {
                            val.push('\r');
                            j += 2;
                        }
                        b't' => {
                            val.push('\t');
                            j += 2;
                        }
                        _ => {
                            // Unknown escape: keep the backslash, then copy the
                            // next *Unicode scalar* (not one byte — bytes[j+1]
                            // may be the lead byte of a multibyte char, and
                            // advancing by 1 would land mid-codepoint and panic
                            // the `chars().next()` in the default arm).
                            val.push('\\');
                            let ch = raw[j + 1..].chars().next().unwrap();
                            val.push(ch);
                            j += 1 + ch.len_utf8();
                        }
                    }
                }
                b'"' => {
                    closed = true;
                    j += 1;
                    break;
                }
                _ => {
                    let ch = raw[j..].chars().next().unwrap();
                    val.push(ch);
                    j += ch.len_utf8();
                }
            }
        }
        if !closed {
            return None;
        }
        (val, j)
    } else if i < bytes.len() && bytes[i] == b'\'' {
        let rest = &raw[i + 1..];
        let p = rest.find('\'')?;
        (rest[..p].to_string(), i + 1 + p + 1)
    } else {
        // Bare value: runs until a `#` preceded by whitespace, or end of line.
        let mut j = i;
        let mut comment_at = None;
        while j < bytes.len() {
            if bytes[j] == b'#' && j > i && (bytes[j - 1] == b' ' || bytes[j - 1] == b'\t') {
                comment_at = Some(j);
                break;
            }
            j += 1;
        }
        let end = comment_at.unwrap_or(bytes.len());
        let val_text = raw[i..end].trim_end();
        (val_text.to_string(), i + val_text.len())
    };

    let rest = raw[vend..].trim_start();
    let comment = if rest.starts_with('#') {
        Some(rest.to_string())
    } else if rest.is_empty() {
        None
    } else {
        // Trailing garbage after a quoted value — not a well-formed entry.
        return None;
    };

    Some(Entry {
        key,
        value,
        value_span: vstart..vend,
        comment,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn parses_quoted_value_with_tag() {
        let f = EnvFile::parse(Path::new("x"), "API_KEY=\"abc123\" # work\n");
        let (i, e) = f.entries().next().unwrap();
        assert_eq!(e.key, "API_KEY");
        assert_eq!(e.value, "abc123");
        assert_eq!(e.comment.as_deref(), Some("# work"));
        assert_eq!(f.value_raw(i), "\"abc123\"");
    }

    #[test]
    fn parses_bare_value_and_export() {
        let f = EnvFile::parse(Path::new("x"), "export PORT=8080 # local\nNAME=plain\n");
        let entries: Vec<_> = f.entries().collect();
        assert_eq!(entries[0].1.value, "8080");
        assert_eq!(entries[0].1.comment.as_deref(), Some("# local"));
        assert_eq!(entries[1].1.value, "plain");
    }

    #[test]
    fn replace_value_preserves_everything_else() {
        let src = "# header\n\nA=\"old\" # tag-a\nB=keep\n";
        let mut f = EnvFile::parse(Path::new("x"), src);
        let idx = f.occurrences("A")[0];
        f.replace_value(idx, "\"new\"");
        assert_eq!(f.serialize(), "# header\n\nA=\"new\" # tag-a\nB=keep\n");
    }

    #[test]
    fn escaped_quotes_round_trip() {
        let quoted = quote_value("a\"b\\c");
        let f = EnvFile::parse(Path::new("x"), &format!("K={quoted}\n"));
        let (_, e) = f.entries().next().unwrap();
        assert_eq!(e.value, "a\"b\\c");
    }

    #[test]
    fn selector_with_tag() {
        let src = "GEMINI_API_KEY=\"v1\" # personal\nGEMINI_API_KEY=\"v2\" # work\n";
        let f = EnvFile::parse(Path::new("x"), src);
        assert!(matches!(
            f.select("GEMINI_API_KEY"),
            Err(SelectError::Ambiguous { .. })
        ));
        let (_, e) = f.select("GEMINI_API_KEY@work").unwrap();
        assert_eq!(e.value, "v2");
        assert!(matches!(
            f.select("GEMINI_API_KEY@nope"),
            Err(SelectError::NotFound(_))
        ));
    }

    #[test]
    fn unterminated_quote_is_not_an_entry() {
        let f = EnvFile::parse(Path::new("x"), "BAD=\"oops\n");
        assert_eq!(f.entries().count(), 0);
        assert_eq!(f.unparseable_lines(), vec![0]);
    }

    #[test]
    fn backslash_before_multibyte_does_not_panic() {
        // Unknown escape `\é`: the lead byte of é must not be treated as one
        // byte (that used to land mid-codepoint and panic).
        let f = EnvFile::parse(Path::new("x"), "K=\"\\é\"\n");
        let (_, e) = f.entries().next().unwrap();
        assert_eq!(e.value, "\\é");
    }

    #[test]
    fn newline_value_round_trips_without_breaking_structure() {
        let quoted = quote_value("a\nb\tc");
        assert!(!quoted.contains('\n'), "must not embed a real newline: {quoted}");
        let f = EnvFile::parse(Path::new("x"), &format!("K={quoted}\nNEXT=ok\n"));
        let entries: Vec<_> = f.entries().collect();
        assert_eq!(entries.len(), 2, "value newline must not split the file");
        assert_eq!(entries[0].1.value, "a\nb\tc");
        assert_eq!(entries[1].1.key, "NEXT");
    }
}
