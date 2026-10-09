# agents-env — agent/dev guide

Rust CLI that lets an AI agent use secrets without seeing their values. User-facing model and command reference live in `README.md`; this file is for working **on** the code.

## Build / test / lint

```
cargo build            # debug
cargo fmt --all -- --check
cargo test --locked    # embedded unit tests (src) + integration tests (tests/cli.rs)
cargo clippy --locked --all-targets -- -D warnings
cargo build --locked --release  # release binary (strip+lto)
```

## Default completion: merge and install

Implementation and fix requests in this repository authorize the full workflow below without another prompt. An explicit plan-only, review-only, or no-merge request takes precedence.

1. Run the checks above and review the final diff. Commit only task-owned paths, push a topic branch, and create or update its PR against `main`.
2. Mark the PR ready and wait for all CI checks on its exact head to pass. Resolve blocking review findings, then merge through the PR with a matching-head guard. Do not bypass failed checks or branch protections.
3. Verify the server reports the PR merged, then fast-forward a clean local `main` to the remote merge result and build its release binary. Preserve unrelated work; use a clean worktree if necessary.
4. Resolve the installed executable with `command -v agents-env` and follow symlinks to its actual destination. If absent, use `~/.local/bin/agents-env`. Keep a rollback copy in an OS-provided temporary directory, stage the new executable beside the destination, and atomically rename it into place. Never overwrite a running executable in place.
5. Compare built and installed SHA-256 hashes, check the installed CLI, and run isolated smoke tests with a temporary HOME and fake secrets for config/path selection, agent guards, masking, interactive output, and child exit codes. On failure, restore the previous binary and verify the rollback before reporting it.
6. Report the PR/merge commit, local `main` sync, installed path, and actual verification results. Completion includes the installed binary working; a release build alone is insufficient.

This authorization covers the local CLI and its PR workflow. Registry publishing, real secret/config changes, and installation on other machines require their own request.

## Architecture (one responsibility per module)

- `main.rs` — clap CLI + command handlers (`get`/`ls`/`run`/`set`/`copy`/`edit`/`doctor`/`config`), scope resolution, mask-set assembly.
- `aimode.rs` — agent-mode detection from env markers (+ config `markers=`).
- `config.rs` — typed settings loaded once per invocation, portable path resolution, guarded config edits. The global store resolves to an absolute path and is never per-command overridable.
- `store.rs` — line-preserving `.env` parser, `KEY@tag` selector, round-trip editing.
- `guard.rs` — write-side guards, backup, atomic write, git-ignore gate, secret heuristic.
- `mask.rs` — child injection + leftmost-longest streaming output masking + `{{KEY}}` argv substitution.

## Security invariants — do not regress (each has a test)

These are the product. A change that weakens one is a bug even if it compiles:

1. In agent mode, `get` never prints a value; `run`'s `--no-mask` is refused.
2. Every **injected** value is in the mask set regardless of length (no short-secret leak).
3. Masking is leftmost-longest with a hold-back buffer — overlapping/prefix secrets and boundary-straddling matches never leak a fragment.
4. `set`/`copy` can only write bare `.env*` files in cwd; the global store is unreachable (no flag, path-separator/symlink/hardlink/samefile/cwd-in-store-dir-or-descendant all rejected).
5. Secret-bearing `copy` refuses git-tracked or non-gitignored targets (no override).
6. Writes back up to `<file>.YYMMDD.bak` (first-of-day wins) then write atomically (`O_NOFOLLOW` temp + rename).

When adding a feature near these, add the adversarial test first. Known accepted limits are documented in README's threat model — don't "fix" them silently.

## Conventions

- Never commit real secrets. `.env`, `.env.*`, `*.bak`, `/target` are gitignored. Tests use fake fixtures (`tvly-aaaa…`).
- Match existing style; keep clippy clean. Errors go to stderr via `fail(code, msg)`; exit codes: 2 = guard/selector refusal, 3 = usage, 1 = io.

## Release

- crates.io: `cargo publish` (metadata in `Cargo.toml`).
- Claude Code plugin: the repo is its own marketplace (`.claude-plugin/{plugin,marketplace}.json`). The skill has one source file at `.agents/skills/agents-env/SKILL.md`; `skills/agents-env` and `.claude/skills/agents-env` are relative symlinks to it. Edit the source, never the symlinks. Validate with `claude plugin validate . --strict`.
