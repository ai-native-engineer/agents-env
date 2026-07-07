//! Secret injection + output masking for `run`.
//!
//! Values reach the child only via its environment (and `{{KEY}}` argv
//! placeholders resolved at exec time). When masking is on, the child's
//! stdout/stderr are pumped through a leftmost-longest Aho-Corasick replacer
//! with a hold-back buffer, so any occurrence of a known secret value — even
//! one straddling a read boundary, and even when one secret is a prefix of
//! another — is rewritten to `[masked:KEY]` before it reaches the caller.

use aho_corasick::{AhoCorasick, MatchKind};
use std::io::{self, Read, Write};
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::thread;

/// Substitute `{{KEY}}` placeholders in argv with injected values.
pub fn substitute_argv(argv: &[String], inject: &[(String, String)]) -> Vec<String> {
    argv.iter()
        .map(|arg| {
            let mut s = arg.clone();
            for (k, v) in inject {
                let ph = format!("{{{{{k}}}}}");
                if s.contains(&ph) {
                    s = s.replace(&ph, v);
                }
            }
            s
        })
        .collect()
}

/// Stream `rdr` to `wtr`, masking every match. Leftmost-longest semantics mean
/// the longest secret wins at any position (so `B="abcdefXYZ"` is masked whole
/// even when `A="abcdef"` is also a pattern). A hold-back of `max_len - 1` bytes
/// guarantees no match that could still grow with future input is emitted early.
fn stream_mask<R: Read, W: Write>(
    mut rdr: R,
    mut wtr: W,
    ac: &AhoCorasick,
    repls: &[Vec<u8>],
    max_len: usize,
) -> io::Result<()> {
    let holdback = max_len.saturating_sub(1);
    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        let n = rdr.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        // Bytes in [0, settled) cannot be the start of a match that extends past
        // the current buffer, so leftmost-longest sees their true longest match.
        let settled = buf.len().saturating_sub(holdback);
        if settled == 0 {
            continue;
        }
        let consumed = mask_region(&buf, settled, ac, repls, &mut wtr)?;
        buf.drain(..consumed);
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
    // `signal(SIG_IGN)` is inherited across exec, so explicitly reset the child
    // to the default disposition before it starts. Otherwise `kill -INT $$` or a
    // terminal Ctrl-C can be ignored by the wrapped command.
    let prev_sigint = prepare_child_sigint(&mut cmd);

    let mask_values: Vec<_> = mask_values.iter().filter(|(_, v)| !v.is_empty()).collect();

    // Fast path when masking is off, or when there is no non-empty value to
    // mask. Empty values have no plaintext fragment to leak.
    if !mask || mask_values.is_empty() {
        let status = cmd.status();
        restore_sigint(prev_sigint);
        return match status {
            Ok(s) => exit_code(s),
            Err(e) => {
                eprintln!("agents-env: failed to run '{}': {e}", argv[0]);
                127
            }
        };
    }

    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            restore_sigint(prev_sigint);
            eprintln!("agents-env: failed to run '{}': {e}", argv[0]);
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
    let max_len = patterns.iter().map(|p| p.len()).max().unwrap_or(0);
    let ac = Arc::new(
        AhoCorasick::builder()
            .match_kind(MatchKind::LeftmostLongest)
            .build(&patterns)
            .expect("failed to build masking automaton"),
    );

    let stdout_pipe = child.stdout.take().expect("stdout is piped");
    let stderr_pipe = child.stderr.take().expect("stderr is piped");
    let (ac2, reps2) = (Arc::clone(&ac), Arc::clone(&replacements));

    let t_out = thread::spawn(move || {
        let _ = stream_mask(stdout_pipe, io::stdout(), &ac, &replacements, max_len);
    });
    let t_err = thread::spawn(move || {
        let _ = stream_mask(stderr_pipe, io::stderr(), &ac2, &reps2, max_len);
    });

    let _ = t_out.join();
    let _ = t_err.join();
    let status = child.wait();
    restore_sigint(prev_sigint);
    match status {
        Ok(s) => exit_code(s),
        Err(e) => {
            eprintln!("agents-env: wait failed: {e}");
            1
        }
    }
}

#[cfg(unix)]
type SigHandler = libc::sighandler_t;

#[cfg(unix)]
fn prepare_child_sigint(cmd: &mut Command) -> SigHandler {
    unsafe {
        let prev = libc::signal(libc::SIGINT, libc::SIG_IGN);
        let child_sigint = if prev == libc::SIG_IGN {
            libc::SIG_IGN
        } else {
            libc::SIG_DFL
        };
        cmd.pre_exec(move || {
            libc::signal(libc::SIGINT, child_sigint);
            Ok(())
        });
        prev
    }
}

#[cfg(unix)]
fn restore_sigint(prev: SigHandler) {
    unsafe {
        libc::signal(libc::SIGINT, prev);
    }
}

#[cfg(not(unix))]
fn prepare_child_sigint(_cmd: &mut Command) {}

#[cfg(not(unix))]
fn restore_sigint(_: ()) {}

fn exit_code(s: std::process::ExitStatus) -> i32 {
    use std::os::unix::process::ExitStatusExt;
    s.code().unwrap_or(128 + s.signal().unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mask_all(values: &[(&str, &str)], input: &str) -> String {
        let patterns: Vec<&[u8]> = values.iter().map(|(_, v)| v.as_bytes()).collect();
        let repls: Vec<Vec<u8>> = values
            .iter()
            .map(|(k, _)| format!("[masked:{k}]").into_bytes())
            .collect();
        let max_len = patterns.iter().map(|p| p.len()).max().unwrap_or(0);
        let ac = AhoCorasick::builder()
            .match_kind(MatchKind::LeftmostLongest)
            .build(&patterns)
            .unwrap();
        let mut out = Vec::new();
        stream_mask(input.as_bytes(), &mut out, &ac, &repls, max_len).unwrap();
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
        assert_eq!(mask_all(&[("PIN", "12345")], "pin=12345."), "pin=[masked:PIN].");
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
            9,
        )
        .unwrap();
        assert_eq!(String::from_utf8(out).unwrap(), "v=[masked:B]!");
    }
}
