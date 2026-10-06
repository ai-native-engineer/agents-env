# Goal Plan Instructions

Long-running agent work that continues until a verifiable condition holds.
Start the loop with `/goal @GOAL.md`.

## Goal

Add an opt-in macOS Keychain backend alongside the existing file backend, preserve current agents-env CLI behavior and headless compatibility, and provide an explicit, tested import/migration path with no secret leakage.

## Proof

Run from the goal worktree:

```bash
cargo fmt --all -- --check && \
cargo test --all-features && \
cargo clippy --all-targets --all-features -- -D warnings && \
./scripts/proof-keychain-backend.sh
```

`scripts/proof-keychain-backend.sh` is part of this goal. It must run the
backend contract tests, import/migration tests, metadata-only listing checks,
agent-mode masking checks, and a macOS Keychain smoke test against a temporary
test keychain or isolated test context. It must never use or mutate the user's
default login keychain, print a secret, require a GUI prompt, or leave a test
keychain behind. On non-macOS hosts it may skip the native smoke test only after
running the portable fake-backend contract tests and reporting that native
coverage is unavailable.

## Acceptance Criteria

1. The existing file backend remains the default and all current file-scope
   behavior continues to pass, including selectors, round-trip editing, local
   writes, masking, write guards, and headless execution.
2. An explicit macOS Keychain backend can list key/tag metadata without
   returning values, and `run` can retrieve values only in memory for child-env
   injection and existing output masking.
3. Keychain access failures in SSH, CI, launchd, or other non-interactive
   contexts fail with a value-free diagnostic and do not wait for GUI approval;
   the file backend remains an explicit fallback.
4. An explicit `import`/`migrate` flow copies file-backed entries into the
   Keychain backend with key/tag preservation, dry-run or preflight reporting,
   rollback guidance, and no automatic source deletion.
5. README, skill guidance, `doctor`, and tests explain the backend boundary,
   the plaintext export behavior of `copy`, and the remaining runtime exposure
   after a secret enters a child process. A human-readable semantic review must
   confirm that the recommendation, exclusions, and headless gaps are explicit.

## Context

The target is the Rust CLI at `/Users/seungwonan/Dev/3-tool/agents-env`, now at
`main` commit `b5659e3`. `src/config.rs` resolves the global file, `src/store.rs`
preserves `.env` lines and `KEY@tag` selectors, `src/main.rs` owns CLI scope and
commands, `src/mask.rs` injects and masks values, and `src/guard.rs` protects
local writes. The current baseline is 12 embedded unit tests plus 36 CLI tests,
with clippy clean and a release build already verified.

Read before implementation: the repository `AGENTS.md`, README threat model,
the agents-env skill, Apple Keychain Services, Apple TN3137 on macOS keychain
implementations, and the Rust provider documentation selected during design.
The repository code and tests are the source of truth for existing behavior;
the Apple documentation is the source of truth for platform constraints.

## Scope

This goal includes:

- a `Store`/backend seam that preserves the existing file backend;
- a macOS-only Keychain backend with explicit `file`/`keychain` selection;
- metadata-only `ls`/`get` behavior and value-bearing `run`/`copy` paths;
- non-interactive access policy and `doctor` diagnostics;
- explicit file-to-Keychain import/migration with preflight and rollback
  guidance;
- fake-provider contract tests, macOS native smoke coverage, CLI regression
  tests, README/skill updates, and a reviewable branch/PR.

## Out of Scope

The goal does not include:

- deleting or silently rewriting an existing global `.env`;
- migrating or inspecting the user's real Keychain without a later explicit
  command and review;
- making Keychain mandatory for Linux, CI, SSH, launchd, or other headless
  environments;
- replacing project-local `.env` files or removing the explicit plaintext
  export behavior of `copy` in this goal;
- production Secret Manager integrations, cloud sync policy, or publishing a
  new crates.io release.

## Constraints

Keep file backend behavior and agent-mode masking intact. Do not put real
secrets in fixtures, logs, arguments, commits, or error messages. Do not call
`security find-generic-password -w` in a way that forwards its stdout to the
agent; prefer direct Security.framework/provider calls with in-memory handling.
Native tests must use an isolated temporary keychain or fake provider and must
restore any process-local search configuration. File backend remains the
default. Rust work uses Cargo.

All target code changes are committed on the goal worktree branch. Do not
modify the user's default login keychain, delete source files, force-push, or
publish packages. Any later merge or release is a separate explicit action.
Use `git revert` for rollback; never `git reset --hard` or force-push.

## Input Stability

The code and test fixtures are versioned in this goal worktree. Apple SDK,
macOS Keychain behavior, and provider documentation are time-dependent; record
the OS/toolchain/provider versions in the relevant progress artifact and treat
native smoke results as current at proof time. The proof must use only the
checked-out fixtures and an isolated temporary Keychain context.

## Target Change Tracking

Target edits are committed directly in this external worktree on branch
`goal/keychain-backend`. Each loop step also updates `progress.tsv` and commits
that ledger with the code or proof artifact. The final result is handed back as
a reviewable branch/PR against `main`; the user's default checkout and real
Keychain remain untouched during implementation.

## Bounds

Use up to 30 goal-loop turns with normal supervision. Stop earlier if the
Keychain API requires an entitlement, GUI-only approval, or platform behavior
that cannot be tested without changing the user's account; record that as a
blocked row with the exact missing evidence and keep the file backend intact.

## How Progress Is Tracked

- `progress.tsv` is the plan and progress table; the task breakdown lives in its rows. Edit it ONLY through the helper so rows never break on tab matching:
  `python3 /Users/seungwonan/.agents/skills/shared/goal-plan/scripts/goal_log.py <add|start|done|block|drop|set|show> ...` (flags: `<cmd> --help`).
- This plan lives in this goal workspace; in worktree mode the workspace is on its own goal branch, and in dedicated-repo mode the repo itself is the ledger. Each loop step ends in a commit, so the commit history is the durable record. `goal_log.py done` stamps the current `HEAD` as the row checkpoint; the commit you make immediately after records the row update. The checkpoint plus git history is your resume point: on resume, recover state with `pwd`, `goal_log.py show`, `git log`, then re-run the Proof.
- Long runs can lose conversational context. What lets the loop survive is externalized state: `progress.tsv` and git hold the plan and checkpoints, and the Proof output stays in the transcript, so you resume cleanly with `goal_log.py show` + `git log`.

## Loop Protocol

Run as an autonomous loop until the Goal holds.

1. `goal_log.py show` to see where you are.
2. Take the next row (or `goal_log.py add --task ...`), then `goal_log.py start <id>`. Pick the row most likely to advance the Goal or unblock the rest, not just the next in line.
3. For behavior changes, first add or reproduce a failing test/check. Do the minimum work within Scope and Constraints, run the Proof, read its result, and fix the root cause until it passes or a real blocker is recorded.
4. `goal_log.py done <id> --decision keep|discard|crash --artifact "<proof>"` (keep if it advanced the goal; discard if it did not but did no harm; crash if it caused a regression). Be skeptical of your own success -- if the Proof passed quietly, rerun it before marking done. Every done row needs artifact or notes evidence; for discard/crash, put WHY it failed and what to try instead in notes so a later session does not repeat the dead end. Use `goal_log.py drop <id> --notes "<reason>"` only for rows that are no longer needed.
5. Commit the changed files plus `progress.tsv`: `git add <files> progress.tsv && git commit -m "<keep|discard>: <what you did>"`. If target code changes live outside this repo, also commit the target repo or commit patch/snapshot artifacts here, according to Target Change Tracking.
6. Surface the Proof in your reply as evidence, not a claim: paste the actual command output (pass/fail, line numbers, exact errors), then name the checkpoint, what you verified this step, what remains, and whether you are blocked. The Proof must verify every Acceptance Criteria item. The `/goal` evaluator reads the conversation, not the files.
7. To undo a change that made things worse, revert with git, sparingly. Do not `git reset` away failed attempts you have already committed -- the commit log is the full record, including discards.

Do not ask "should I continue?" once the loop starts. Stop only when the Goal holds, when you hit a Bound, or when blocked by missing access, destructive risk, or an explicit user choice -- when blocked, report what specific input or access would unblock you. Budget or turn exhaustion is not completion.

## Completion

Complete only when the Goal holds and is shown with the Proof output in the conversation or logs, and required rows in `progress.tsv` are `done` or intentionally `dropped`.
