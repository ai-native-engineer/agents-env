//! Secret injection + output masking for `run`.
//!
//! Values reach the child only via its environment (and `{{KEY}}` argv
//! placeholders resolved at exec time). When masking is on, the child's
//! stdout/stderr are pumped through an Aho-Corasick stream replacer, so any
//! occurrence of a known secret value — including across chunk boundaries —
//! is rewritten to `[masked:KEY]` before it can reach the caller's context.

use aho_corasick::AhoCorasick;
use std::process::{Command, Stdio};
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

/// Run `argv` with `inject` added to its environment. When `mask` is true the
/// child's stdout/stderr are filtered so that any value in `mask_values`
/// (key, value pairs) appears as `[masked:KEY]`.
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

    if !mask || mask_values.is_empty() {
        return match cmd.status() {
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
            eprintln!("agents-env: failed to run '{}': {e}", argv[0]);
            return 127;
        }
    };

    let patterns: Vec<&[u8]> = mask_values.iter().map(|(_, v)| v.as_bytes()).collect();
    let replacements: Vec<Vec<u8>> = mask_values
        .iter()
        .map(|(k, _)| format!("[masked:{k}]").into_bytes())
        .collect();
    // Standard match kind is the only one that supports streaming replacement;
    // each secret value is its own pattern, so leftmost-first matching still
    // masks every full value (the no-leak property holds).
    let ac = AhoCorasick::new(&patterns).expect("failed to build masking automaton");

    let stdout_pipe = child.stdout.take().expect("stdout is piped");
    let stderr_pipe = child.stderr.take().expect("stderr is piped");
    let ac2 = ac.clone();
    let reps2 = replacements.clone();

    let t_out = thread::spawn(move || {
        let _ = ac.try_stream_replace_all(stdout_pipe, std::io::stdout(), &replacements);
    });
    let t_err = thread::spawn(move || {
        let _ = ac2.try_stream_replace_all(stderr_pipe, std::io::stderr(), &reps2);
    });

    let _ = t_out.join();
    let _ = t_err.join();
    match child.wait() {
        Ok(s) => exit_code(s),
        Err(e) => {
            eprintln!("agents-env: wait failed: {e}");
            1
        }
    }
}

fn exit_code(s: std::process::ExitStatus) -> i32 {
    use std::os::unix::process::ExitStatusExt;
    s.code().unwrap_or(128 + s.signal().unwrap_or(0))
}
