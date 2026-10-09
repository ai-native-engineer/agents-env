//! Secret injection + output masking for `run`.
//!
//! Values reach the child only via its environment (and `{{KEY}}` argv
//! placeholders resolved at exec time). When masking is on, the child's
//! stdout/stderr are pumped through a leftmost-longest Aho-Corasick replacer
//! with a hold-back buffer, so any occurrence of a known secret value — even
//! one straddling a read boundary, and even when one secret is a prefix of
//! another — is rewritten to `[masked:KEY]` before it reaches the caller.

use aho_corasick::{AhoCorasick, MatchKind};
use std::collections::{HashMap, VecDeque};
use std::io::{self, Read, Write};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::thread;

/// Substitute `{{KEY}}` placeholders in argv with injected values.
pub fn substitute_argv(argv: &[String], inject: &[(String, String)]) -> Vec<String> {
    let mut values = HashMap::new();
    for (key, value) in inject {
        values.entry(key.as_str()).or_insert(value.as_str());
    }
    argv.iter()
        .map(|arg| {
            // Scan only the original argument: values may themselves contain
            // placeholder syntax, which must reach the child unchanged.
            let mut result = String::with_capacity(arg.len());
            let mut remaining = arg.as_str();
            while let Some(start) = remaining.find("{{") {
                result.push_str(&remaining[..start]);
                let placeholder = &remaining[start..];
                let Some(end) = placeholder[2..].find("}}") else {
                    result.push_str(placeholder);
                    remaining = "";
                    break;
                };
                let end = end + 2;
                let key = &placeholder[2..end];
                result.push_str(values.get(key).copied().unwrap_or(&placeholder[..end + 2]));
                remaining = &placeholder[end + 2..];
            }
            result.push_str(remaining);
            result
        })
        .collect()
}

#[derive(Default)]
struct PrefixNode {
    edges: HashMap<u8, usize>,
    failure: usize,
    depth: usize,
    pending_len: usize,
}

/// Tracks the longest suffix that is a proper prefix of a secret. The trie
/// stores each prefix as a node, never as a separate copied prefix string.
/// Large reads need only their bounded tail scanned from the root. Short reads
/// advance the retained state, keeping prefix detection amortized linear even
/// for long secrets arriving one byte at a time.
struct PrefixAutomaton {
    nodes: Vec<PrefixNode>,
    max_pending_len: usize,
}

impl PrefixAutomaton {
    fn new(patterns: &[&[u8]]) -> Self {
        let mut nodes = vec![PrefixNode::default()];
        for pattern in patterns {
            let mut state = 0;
            for &byte in *pattern {
                state = match nodes[state].edges.get(&byte).copied() {
                    Some(next) => next,
                    None => {
                        let next = nodes.len();
                        let depth = nodes[state].depth + 1;
                        nodes.push(PrefixNode {
                            depth,
                            ..PrefixNode::default()
                        });
                        nodes[state].edges.insert(byte, next);
                        next
                    }
                };
            }
        }

        let mut queue: VecDeque<_> = nodes[0].edges.values().copied().collect();
        while let Some(state) = queue.pop_front() {
            nodes[state].pending_len = if nodes[state].edges.is_empty() {
                nodes[nodes[state].failure].pending_len
            } else {
                nodes[state].depth
            };
            let edges: Vec<_> = nodes[state]
                .edges
                .iter()
                .map(|(&byte, &next)| (byte, next))
                .collect();
            for (byte, next) in edges {
                let mut fallback = nodes[state].failure;
                while fallback != 0 && !nodes[fallback].edges.contains_key(&byte) {
                    fallback = nodes[fallback].failure;
                }
                nodes[next].failure = nodes[fallback].edges.get(&byte).copied().unwrap_or(0);
                queue.push_back(next);
            }
        }
        Self {
            nodes,
            max_pending_len: patterns
                .iter()
                .map(|pattern| pattern.len().saturating_sub(1))
                .max()
                .unwrap_or(0),
        }
    }

    fn advance(&self, state: &mut usize, bytes: &[u8]) {
        let bytes = if bytes.len() >= self.max_pending_len {
            // Every suffix that can still grow into a secret fits entirely in
            // this new read's tail. Ignore settled leading bytes and old state.
            *state = 0;
            &bytes[bytes.len() - self.max_pending_len..]
        } else {
            bytes
        };
        for byte in bytes {
            while *state != 0 && !self.nodes[*state].edges.contains_key(byte) {
                *state = self.nodes[*state].failure;
            }
            *state = self.nodes[*state].edges.get(byte).copied().unwrap_or(0);
        }
    }

    fn retain_suffix(&self, state: &mut usize, remaining_len: usize) {
        // A complete masked match may consume some of the pending prefix.
        // Follow failures to discard states that started in consumed bytes.
        while self.nodes[*state].depth > remaining_len {
            *state = self.nodes[*state].failure;
        }
    }
}

/// Stream `rdr` to `wtr`, masking every match with leftmost-longest semantics.
/// Only a suffix that could grow into a secret is held back; settled output is
/// flushed before reading again, so interactive prompts reach the caller.
fn stream_mask<R: Read, W: Write>(
    mut rdr: R,
    mut wtr: W,
    ac: &AhoCorasick,
    repls: &[Vec<u8>],
    prefixes: &PrefixAutomaton,
) -> io::Result<()> {
    let mut state = 0;
    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        let n = match rdr.read(&mut chunk) {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            result => result?,
        };
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        prefixes.advance(&mut state, &chunk[..n]);
        loop {
            // The longest proper-prefix suffix starts at the earliest position
            // whose match could still grow. Earlier match starts are settled.
            let settled = buf.len() - prefixes.nodes[state].pending_len;
            if settled == 0 {
                break;
            }
            let consumed = mask_region(&buf, settled, ac, repls, &mut wtr)?;
            buf.drain(..consumed);
            prefixes.retain_suffix(&mut state, buf.len());
            wtr.flush()?;
            // If a complete match crossed `settled`, the retained suffix may
            // no longer be a prefix. Emit those newly settled bytes now too.
        }
    }
    // EOF: everything left is settled.
    let len = buf.len();
    mask_region(&buf, len, ac, repls, &mut wtr)?;
    wtr.flush()
}

/// Replace matches whose start is `< settled`, emit the settled literal bytes
/// between them, and return how many bytes of `buf` were consumed (emitted).
fn mask_region<W: Write>(
    buf: &[u8],
    settled: usize,
    ac: &AhoCorasick,
    repls: &[Vec<u8>],
    wtr: &mut W,
) -> io::Result<usize> {
    let mut pos = 0usize;
    for m in ac.find_iter(buf) {
        if m.start() >= settled {
            break;
        }
        wtr.write_all(&buf[pos..m.start()])?;
        wtr.write_all(&repls[m.pattern()])?;
        pos = m.end(); // may exceed `settled`: the match was complete in buf
    }
    if pos < settled {
        wtr.write_all(&buf[pos..settled])?;
        pos = settled;
    }
    Ok(pos)
}

/// Run `argv` with `inject` added to its environment. When `mask` is true the
/// child's stdout/stderr are filtered so that any value in `mask_values`
/// (key, value) appears as `[masked:KEY]`.
/// Returns the child's exit code (128+signal if killed by a signal).
pub fn run(
    inject: &[(String, String)],
    argv: &[String],
    mask_values: &[(String, String)],
    mask: bool,
) -> i32 {
    let argv = substitute_argv(argv, inject);
    let mut cmd = Command::new(&argv[0]);
    cmd.args(&argv[1..]);
    for (k, v) in inject {
        cmd.env(k, v);
    }

    // Let Ctrl-C go to the child (same process group); the wrapper waits.
    unsafe {
        libc::signal(libc::SIGINT, libc::SIG_IGN);
    }

    // The wrapper ignores SIGINT while it waits so the terminal interrupt is
    // handled by the child. Restore the default disposition after fork; signal
    // dispositions are inherited across exec, and leaving SIG_IGN here would
    // make `sleep`, shells, and other ordinary commands ignore Ctrl-C too.
    #[cfg(unix)]
    unsafe {
        use std::os::unix::process::CommandExt;
        cmd.pre_exec(|| {
            if libc::signal(libc::SIGINT, libc::SIG_DFL) == libc::SIG_ERR {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }

    let mask_values: Vec<_> = mask_values.iter().filter(|(_, v)| !v.is_empty()).collect();

    // Fast path when masking is off, or when there is no non-empty value to
    // mask. Empty values have no plaintext fragment to leak.
    if !mask || mask_values.is_empty() {
        return match cmd.status() {
            Ok(s) => exit_code(s),
            Err(_) => {
                // The substituted executable and even OS error details may
                // contain secrets. Wrapper diagnostics never echo either.
                eprintln!("agents-env: failed to run command");
                127
            }
        };
    }

    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(_) => {
            eprintln!("agents-env: failed to run command");
            return 127;
        }
    };

    let patterns: Vec<&[u8]> = mask_values.iter().map(|(_, v)| v.as_bytes()).collect();
    let replacements: Arc<Vec<Vec<u8>>> = Arc::new(
        mask_values
            .iter()
            .map(|(k, _)| format!("[masked:{k}]").into_bytes())
            .collect(),
    );
    let prefixes = Arc::new(PrefixAutomaton::new(&patterns));
    let ac = Arc::new(
        AhoCorasick::builder()
            .match_kind(MatchKind::LeftmostLongest)
            .build(&patterns)
            .expect("failed to build masking automaton"),
    );

    let stdout_pipe = child.stdout.take().expect("stdout is piped");
    let stderr_pipe = child.stderr.take().expect("stderr is piped");
    let (ac2, reps2) = (Arc::clone(&ac), Arc::clone(&replacements));
    let prefixes2 = Arc::clone(&prefixes);

    let t_out = thread::spawn(move || {
        stream_mask(stdout_pipe, io::stdout(), &ac, &replacements, &prefixes)
    });
    let t_err =
        thread::spawn(move || stream_mask(stderr_pipe, io::stderr(), &ac2, &reps2, &prefixes2));

    let stdout_result = t_out.join();
    let stderr_result = t_err.join();
    let output_failed =
        !matches!(stdout_result, Ok(Ok(()))) || !matches!(stderr_result, Ok(Ok(())));
    match child.wait() {
        Ok(_) if output_failed => {
            eprintln!("agents-env: failed to forward masked command output");
            1
        }
        Ok(s) => exit_code(s),
        Err(_) => {
            eprintln!("agents-env: wait failed");
            1
        }
    }
}

fn exit_code(s: std::process::ExitStatus) -> i32 {
    use std::os::unix::process::ExitStatusExt;
    s.code().unwrap_or(128 + s.signal().unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_prompt_is_flushed_before_next_read() {
        use std::cell::RefCell;
        use std::rc::Rc;

        #[derive(Default)]
        struct Output {
            bytes: Vec<u8>,
            flushed: usize,
        }
        struct PromptReader {
            output: Rc<RefCell<Output>>,
            read_once: bool,
            input: &'static [u8],
            expected_before_next_read: &'static [u8],
        }
        impl Read for PromptReader {
            fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
                if self.read_once {
                    let output = self.output.borrow();
                    assert_eq!(output.bytes, self.expected_before_next_read);
                    assert_eq!(output.flushed, output.bytes.len());
                    return Ok(0);
                }
                self.read_once = true;
                buf[..self.input.len()].copy_from_slice(self.input);
                Ok(self.input.len())
            }
        }
        struct PromptWriter(Rc<RefCell<Output>>);
        impl Write for PromptWriter {
            fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
                self.0.borrow_mut().bytes.extend_from_slice(buf);
                Ok(buf.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                let mut output = self.0.borrow_mut();
                output.flushed = output.bytes.len();
                Ok(())
            }
        }
        type PromptCase<'a> = (&'a [&'a [u8]], &'a [u8], &'a [u8]);
        let cases: &[PromptCase<'_>] = &[
            (&[b"fake-secret-value"], b"READY\n", b"READY\n"),
            // Masking "ab" consumes the "b" in pending prefix "bcd".
            // The remaining "cd" is safe and must be flushed before reading.
            (&[b"ab", b"bcde"], b"abcd", b"[masked:K]cd"),
            // A complete short secret cannot be emitted while the same start
            // might still become a longer secret.
            (&[b"ab", b"abcdef"], b"abc", b""),
        ];
        for (patterns, input, expected_before_next_read) in cases {
            let output = Rc::new(RefCell::new(Output::default()));
            let ac = AhoCorasick::builder()
                .match_kind(MatchKind::LeftmostLongest)
                .build(*patterns)
                .unwrap();
            let repls = vec![b"[masked:K]".to_vec(); patterns.len()];
            stream_mask(
                PromptReader {
                    output: Rc::clone(&output),
                    read_once: false,
                    input,
                    expected_before_next_read,
                },
                PromptWriter(output),
                &ac,
                &repls,
                &PrefixAutomaton::new(patterns),
            )
            .unwrap();
        }
    }

    struct Chunks<'a> {
        input: &'a [u8],
        sizes: std::vec::IntoIter<usize>,
    }
    impl Read for Chunks<'_> {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if self.input.is_empty() {
                return Ok(0);
            }
            let size = self.sizes.next().unwrap_or(self.input.len());
            let size = size.min(buf.len()).min(self.input.len());
            buf[..size].copy_from_slice(&self.input[..size]);
            self.input = &self.input[size..];
            Ok(size)
        }
    }

    fn assert_chunked_matches_whole(patterns: &[&[u8]], input: &[u8], sizes: Vec<usize>) {
        let ac = AhoCorasick::builder()
            .match_kind(MatchKind::LeftmostLongest)
            .build(patterns)
            .unwrap();
        let repls: Vec<_> = (0..patterns.len())
            .map(|i| format!("[masked:{i}]").into_bytes())
            .collect();
        let expected = ac.replace_all_bytes(input, &repls);
        let mut output = Vec::new();
        stream_mask(
            Chunks {
                input,
                sizes: sizes.into_iter(),
            },
            &mut output,
            &ac,
            &repls,
            &PrefixAutomaton::new(patterns),
        )
        .unwrap();
        assert_eq!(output, expected);
    }

    #[test]
    fn streaming_matches_whole_for_every_short_input_partition() {
        let sets: &[&[&[u8]]] = &[
            &[b"a", b"ab", b"aba"],
            &[b"ab", b"baba", b"ba"],
            &[b"aaa", b"aab", b"bab"],
        ];
        for length in 1..=7 {
            for bits in 0..(1usize << length) {
                let input: Vec<_> = (0..length)
                    .map(|i| if bits & (1 << i) == 0 { b'a' } else { b'b' })
                    .collect();
                for cuts in 0..(1usize << (length - 1)) {
                    let mut sizes = Vec::new();
                    let mut start = 0;
                    for end in 1..length {
                        if cuts & (1 << (end - 1)) != 0 {
                            sizes.push(end - start);
                            start = end;
                        }
                    }
                    sizes.push(length - start);
                    for patterns in sets {
                        assert_chunked_matches_whole(patterns, &input, sizes.clone());
                    }
                }
            }
        }
    }

    #[test]
    fn streaming_matches_whole_at_every_byte_split() {
        let cases: &[(&[&[u8]], &[u8])] = &[
            (&[b"ab", b"bcde"], b"abcdexabcd!"),
            (&[b"abcdef", b"abcdefXYZ"], b"v=abcdefXYZ!abcdef!"),
            (&[b"aaaaab", b"aaab"], b"aaaaaaaaabaaaaacaaaaab"),
            (
                &["비밀".as_bytes(), "비밀값".as_bytes()],
                "문구=비밀값/비밀!".as_bytes(),
            ),
            (&[b"\xff\x00", b"\x00\xfe"], b"\xff\x00\xfe\xff\x00"),
        ];
        for (patterns, input) in cases {
            for split in 1..input.len() {
                assert_chunked_matches_whole(patterns, input, vec![split, input.len() - split]);
            }
            assert_chunked_matches_whole(patterns, input, vec![1; input.len()]);
        }
    }

    #[test]
    fn substitution_does_not_expand_placeholders_in_values() {
        let inject = vec![
            ("A".to_owned(), "{{B}}".to_owned()),
            ("B".to_owned(), "fake-secret".to_owned()),
        ];
        let argv = vec!["prefix={{A}},{{B}},{{A}}".to_owned()];
        assert_eq!(
            substitute_argv(&argv, &inject),
            vec!["prefix={{B}},fake-secret,{{B}}".to_owned()]
        );
        let reversed: Vec<_> = inject.into_iter().rev().collect();
        assert_eq!(
            substitute_argv(&argv, &reversed),
            vec!["prefix={{B}},fake-secret,{{B}}".to_owned()]
        );
    }

    #[test]
    fn substitution_preserves_unknown_and_unclosed_placeholders() {
        let inject = vec![("A".to_owned(), "값".to_owned())];
        let argv = vec!["{{UNKNOWN}}{{A}}{{A}}/{{unfinished".to_owned()];
        assert_eq!(
            substitute_argv(&argv, &inject),
            vec!["{{UNKNOWN}}값값/{{unfinished".to_owned()]
        );
    }

    #[test]
    fn masking_retries_interrupted_reads() {
        struct InterruptedReader {
            data: io::Cursor<Vec<u8>>,
            interrupt_next: bool,
        }
        impl Read for InterruptedReader {
            fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
                if self.interrupt_next {
                    self.interrupt_next = false;
                    return Err(io::Error::from(io::ErrorKind::Interrupted));
                }
                self.interrupt_next = true;
                self.data.read(&mut buf[..1])
            }
        }
        let ac = AhoCorasick::builder()
            .match_kind(MatchKind::LeftmostLongest)
            .build(["fake-secret"])
            .unwrap();
        let mut out = Vec::new();
        stream_mask(
            InterruptedReader {
                data: io::Cursor::new(b"x=fake-secret!".to_vec()),
                interrupt_next: true,
            },
            &mut out,
            &ac,
            &[b"[masked:K]".to_vec()],
            &PrefixAutomaton::new(&[b"fake-secret"]),
        )
        .unwrap();
        assert_eq!(out, b"x=[masked:K]!");
    }

    fn mask_all(values: &[(&str, &str)], input: &str) -> String {
        let patterns: Vec<&[u8]> = values.iter().map(|(_, v)| v.as_bytes()).collect();
        let repls: Vec<Vec<u8>> = values
            .iter()
            .map(|(k, _)| format!("[masked:{k}]").into_bytes())
            .collect();
        let prefixes = PrefixAutomaton::new(&patterns);
        let ac = AhoCorasick::builder()
            .match_kind(MatchKind::LeftmostLongest)
            .build(&patterns)
            .unwrap();
        let mut out = Vec::new();
        stream_mask(input.as_bytes(), &mut out, &ac, &repls, &prefixes).unwrap();
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn masks_basic() {
        assert_eq!(
            mask_all(&[("K", "secret123")], "x=secret123;"),
            "x=[masked:K];"
        );
    }

    #[test]
    fn overlapping_secret_masked_whole_no_suffix_leak() {
        // B is a superstring of A; the full B must be masked, not "A]XYZ".
        let out = mask_all(&[("A", "abcdef"), ("B", "abcdefXYZ")], "v=abcdefXYZ!");
        assert_eq!(out, "v=[masked:B]!");
        assert!(!out.contains("XYZ"));
    }

    #[test]
    fn short_secret_is_masked() {
        assert_eq!(
            mask_all(&[("PIN", "12345")], "pin=12345."),
            "pin=[masked:PIN]."
        );
    }

    #[test]
    fn match_straddling_read_boundary() {
        // Drive the chunked path directly: secret split across two reads.
        struct TwoChunks(Vec<u8>, usize);
        impl Read for TwoChunks {
            fn read(&mut self, b: &mut [u8]) -> io::Result<usize> {
                if self.1 >= self.0.len() {
                    return Ok(0);
                }
                // hand back one byte at a time to stress the hold-back buffer
                b[0] = self.0[self.1];
                self.1 += 1;
                Ok(1)
            }
        }
        let patterns: Vec<&[u8]> = vec![b"abcdefXYZ"];
        let repls = vec![b"[masked:B]".to_vec()];
        let ac = AhoCorasick::builder()
            .match_kind(MatchKind::LeftmostLongest)
            .build(&patterns)
            .unwrap();
        let mut out = Vec::new();
        stream_mask(
            TwoChunks(b"v=abcdefXYZ!".to_vec(), 0),
            &mut out,
            &ac,
            &repls,
            &PrefixAutomaton::new(&patterns),
        )
        .unwrap();
        assert_eq!(String::from_utf8(out).unwrap(), "v=[masked:B]!");
    }
}
