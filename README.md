# agents-env

Env var manager that lets AI coding agents **use** secrets without ever **seeing** them.

No schema, no encrypted vault, no cloud account. Keep your plaintext `.env` exactly as it is — `agents-env` is a zero-config safety layer on top of it. The same command prints the value for a human at a terminal and masks it for an agent (Claude Code, Cursor, …), so you never split your workflow in two.

```
agents-env run TAVILY_API_KEY@work -- curl -H "Authorization: Bearer {{TAVILY_API_KEY}}" https://api/...
```

The value goes only into the child process. `{{TAVILY_API_KEY}}` in the argv is resolved at exec time, so the transcript keeps the placeholder. If the child echoes the secret (e.g. `curl -v`, a stack trace), it is rewritten to `[masked:TAVILY_API_KEY]` in real time before it can reach the agent's context.

## Why this exists

Every secret tool injects secrets into a process. None of them solve "the value must never land in an AI's conversation log, while the agent still reads *and writes* env files." A 14-tool survey put the best coverage at 3/7 of these requirements:

- **Output masking** — `doppler/infisical run` inject fine but don't mask child output; one `curl -v` leaks the value. Only varlock and (paid) 1Password mask.
- **Value-free copy** — no surveyed tool copies a global secret into a local `.env` without the value passing through the caller. `dotenvx set` takes the value as an argument, i.e. the agent already saw it.
- **Auto agent detection** — varlock needs a manual `--agent` flag; forget it once and the value leaks. agents-env detects Claude Code (`CLAUDECODE` / `CLAUDE_CODE_ENTRYPOINT` / `AI_AGENT`) and Codex (`CODEX_SANDBOX`) automatically, and is safe by default. Other harnesses opt in with `AGENTS_ENV_AGENT_MODE=1` or a `markers=A,B,C` line in the config.
- **Asymmetric write guard** — the human-owned global store is structurally unwritable by the tool (see below). No other tool models this.

## Install

The CLI (required):

```
cargo install --path .          # or: brew install seungwonme/tap/agents-env  (planned)
```

The Claude Code plugin (optional — teaches the agent how to use the CLI). This
repo is also its own plugin marketplace:

```
claude plugin marketplace add seungwonme/agents-env
claude plugin install agents-env@agents-env
```

The plugin only ships the skill; it still needs the CLI installed above.

## Setup

Point the global store at your existing master `.env`:

```
mkdir -p ~/.config/agents-env
echo 'global_store=~/.dotfiles/.env' > ~/.config/agents-env/config
```

Default if unset: `~/.config/agents-env/global.env`. The path is **not** overridable per command — that would be a bypass surface for the write guard.

## Commands

| Command | What it does |
|---|---|
| `get <pattern>` | Look up keys (substring match). Human: prints `KEY=value`. Agent: prints `KEY [set, N chars] # tag` only. |
| `ls [pattern]` | Key names + tags. Never prints values, in any mode. |
| `run <KEY[@tag]…> -- <cmd>` | Inject into the child env, mask its output, resolve `{{KEY}}` in argv. `--all` injects the whole scope. |
| `set <KEY> <VALUE> --to <file>` | Write a **non-secret** literal to a local file. Warns if the value looks like a credential. |
| `copy <KEY[@tag]…> --to <file>` | Copy secrets from the global store into a local file — value never printed. `--as NEWKEY` renames. |
| `edit` | Open the global store in `$EDITOR`. **Human only** — refused in agent mode and on non-TTY. |
| `doctor` | Audit: file permissions, gitignore coverage, stale backups, untagged duplicate keys, Claude Code deny rules. |

### Scope and files

Default scope is the global store. `-l`/`--local` reads `./.env`; `-f <name>` reads `./<name>` (implies local) for managing `.env.local`, `.env.production`, etc.

```
agents-env -f .env.production get DATABASE
```

### Duplicate keys: `KEY@tag`

When a key has several accounts, the inline `# comment` is the tag. Decisive operations (`run`, `copy`) require a unique match — an ambiguous selector errors with the candidate tags (never the values):

```
agents-env copy NOTION_API_KEY@demodev --to .env.local
```

## Write guard

`set`/`copy` can only write `.env*` files in the current directory. The global store is unreachable by construction:

- No flag points the write side at the global scope.
- File names must be bare `.env`/`.env.*` — path separators are rejected, so `../`, absolute paths and `.bak` targets are out.
- The target is rejected if it is a **symlink**, has **hard links**, or `samefile`-matches the global store.
- Writing inside the global store's directory is refused.
- Inside a git repo, a secret-bearing `copy` target must be untracked **and** gitignored — otherwise it's a hard error (no override; fix `.gitignore`).

Every write makes a `<file>.YYMMDD.bak` backup first (first-of-the-day wins — the start-of-day state is the recovery point), then writes atomically via an `O_NOFOLLOW` temp file + rename. Backups are `.env`-prefixed so one `.env*` gitignore line covers them.

## Threat model (honest limits)

Masking is **defense in depth, not a sandbox**. It catches verbatim secret values in child output; it does not catch a value the child re-encodes (base64, URL-encoding, splitting). `cat .env` and Claude Code's `@.env` inline reference bypass this tool entirely — that gap is closed by the harness deny layer. `doctor` checks that your `~/.claude/settings.json` denies `Read(**/.env)` / `Read(**/.env.*)`; add those rules so the two layers cover each other.

Known limitations (by design, or deferred):

- **Agent detection is a signal, not a wall.** Mode is read from env markers, which an agent could unset (`env -u CLAUDECODE …`) to force human output, or `--no-mask`. This is fine for the actual threat model — an *honest* agent that shouldn't accidentally log a secret. A *malicious* agent can read `~/.dotfiles/.env` directly anyway; that's the deny layer's job, not this tool's.
- **`{{KEY}}` argv substitution is visible to same-user `ps`.** The value lands in the child's argv, readable by other processes of the same user. Use env injection (no `{{KEY}}`) for anything sensitive on a shared box.
- **Parent-directory-swap TOCTOU.** The write guard canonicalizes cwd and uses `O_NOFOLLOW` on the temp file, but a same-user attacker who renames a parent directory mid-write could still redirect it. Closing this fully needs directory-fd (`openat`/`renameat`) writes — planned for a later version. Not reachable without local same-user write access, at which point your secrets are already exposed.
- **Line endings normalize to LF.** Round-trip preserves comments, order and spacing, but CRLF files are rewritten with LF and a trailing newline is added.

## License

MIT
