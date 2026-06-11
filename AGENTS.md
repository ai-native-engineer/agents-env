# agents-env — agent/dev guide

Rust CLI that lets an AI agent use secrets without seeing their values. User-facing model and command reference live in `README.md`; this file is for working **on** the code.

## Build / test / lint

```
cargo build            # debug
cargo test             # unit (in each module) + integration (tests/cli.rs)
cargo clippy --all-targets   # must stay at 0 warnings
cargo build --release  # strip+lto, installs to ~/.local/bin via the commands below
```

Install the local build for dogfooding: `cp target/release/agents-env ~/.local/bin/`.

## Architecture (one responsibility per module)

- `main.rs` — clap CLI + command handlers (`get`/`ls`/`run`/`set`/`copy`/`edit`/`doctor`), scope resolution, mask-set assembly.
- `aimode.rs` — agent-mode detection from env markers (+ config `markers=`).
- `config.rs` — global store path resolution (always absolute; never per-command overridable).
- `store.rs` — line-preserving `.env` parser, `KEY@tag` selector, round-trip editing.
- `guard.rs` — write-side guards, backup, atomic write, git-ignore gate, secret heuristic.
- `mask.rs` — child injection + leftmost-longest streaming output masking + `{{KEY}}` argv substitution.

## Security invariants — do not regress (each has a test)

These are the product. A change that weakens one is a bug even if it compiles:

1. In agent mode, `get` never prints a value; `run`'s `--no-mask` is refused.
2. Every **injected** value is in the mask set regardless of length (no short-secret leak).
3. Masking is leftmost-longest with a hold-back buffer — overlapping/prefix secrets and boundary-straddling matches never leak a fragment.
4. `set`/`copy` can only write bare `.env*` files in cwd; the global store is unreachable (no flag, path-separator/symlink/hardlink/samefile/cwd-in-store-dir all rejected).
5. Secret-bearing `copy` refuses git-tracked or non-gitignored targets (no override).
6. Writes back up to `<file>.YYMMDD.bak` (first-of-day wins) then write atomically (`O_NOFOLLOW` temp + rename).

When adding a feature near these, add the adversarial test first. Known accepted limits are documented in README's threat model — don't "fix" them silently.

## Conventions

- Never commit real secrets. `.env`, `.env.*`, `*.bak`, `/target` are gitignored. Tests use fake fixtures (`tvly-aaaa…`).
- Match existing style; keep clippy clean. Errors go to stderr via `fail(code, msg)`; exit codes: 2 = guard/selector refusal, 3 = usage, 1 = io.

## Release

- crates.io: `cargo publish` (metadata in `Cargo.toml`).
- Claude Code plugin: the repo is its own marketplace (`.claude-plugin/{plugin,marketplace}.json`). The skill has one source file at `.agents/skills/agents-env/SKILL.md`; `skills/agents-env` and `.claude/skills/agents-env` are relative symlinks to it. Edit the source, never the symlinks. Validate with `claude plugin validate . --strict`.
