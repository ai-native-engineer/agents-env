<h1 align="center">agents-env</h1>

<p align="center">A CLI that lets AI coding agents use secrets without exposing their values</p>

<p align="center">
  <a href="https://crates.io/crates/agents-env"><img src="https://img.shields.io/crates/v/agents-env.svg" alt="crates.io"></a>
  <a href="./LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue.svg" alt="license"></a>
  <img src="https://img.shields.io/badge/built%20with-Rust-orange.svg" alt="rust">
</p>

<p align="center"><a href="./README.md">한국어</a> · <b>English</b></p>

---

When you hand an API key to an agent, the value ends up in the conversation log. A line of `curl -v` output or a stack trace is enough to expose it. agents-env passes the value only into the child process and leaves nothing but the key name in the agent's transcript. No schema, no encrypted vault, no cloud account; it sits on top of your existing `.env`.

```
agents-env run TAVILY_API_KEY@work -- curl -H "Authorization: Bearer {{TAVILY_API_KEY}}" https://api/...
```

`{{TAVILY_API_KEY}}` is replaced with the real value only at the moment the command runs. All the log keeps is the string `{{TAVILY_API_KEY}}`, and if curl prints the key it comes out as `[masked:TAVILY_API_KEY]`.

## Features

There is no shortage of env and secret tools, but few keep the value out of the AI's log while still letting the agent read and write `.env` files. Across 14 comparable tools, none had all four of the following.

- **Output masking** — when an injected secret appears in the child's stdout/stderr, it is replaced with `[masked:KEY]` in real time. `doppler run` and `infisical run` inject but leave the output untouched.
- **Value-free copy** — `copy` moves a secret from the global store into a local `.env` without the value passing through the agent's context.
- **Agent-mode detection** — it auto-detects Grok, Codex, OpenCode, Claude Code, and AGY from runtime markers or parent CLI names; other assistants opt in with `AGENTS_ENV_AGENT_MODE=1` or `markers=`.
- **Asymmetric write guard** — the human-managed global master `.env` cannot be modified through this tool.

## How it works

`get` finds a key; `run` uses one. You cannot pull a value out with command substitution like `mytool --key "$(agents-env get KEY)"`. `$(...)` brings the value onto the shell command line, which is the agent's context, so in agent mode `get` returns the key name and length rather than the value. The value is delivered straight to the child process by `run`.

Masking only touches the output stream (child → caller); it never alters the input the program receives. The program runs with the real value, and that value is hidden only when it comes back out.

## Usage

| Command | Description |
|---|---|
| `get <pattern>` | Look up keys (substring match). Humans get `KEY=value`; agents get `KEY [set, N chars] # tag` only. |
| `ls [pattern]` | Key names and tags. Never prints values, in any mode. |
| `run <KEY[@tag]…> -- <cmd>` | Inject into the child env, mask its output, substitute `{{KEY}}` in argv. `--all` injects the whole scope. |
| `set <KEY> <VALUE> --to <file>` | Write a non-secret value to a local file. Warns if it looks like a credential. |
| `copy <KEY[@tag]…> --to <file>` | Copy secrets from the global store into a local file; values are not printed. `--as NEWKEY` renames. |
| `edit` | Open the global store in `$EDITOR`. Human only; refused in agent mode and on non-TTY. |
| `doctor` | Check file permissions, gitignore coverage, stale backups, untagged duplicate keys, Claude Code deny rules. |
| `config show` / `config path` | Show effective settings or the config path without reading secret values. |
| `config set <key> <value>` / `config reset <key>` | Change one setting or restore its default; refused in agent mode. |

Full options: `agents-env --help`.

## Settings and defaults

The target platforms are macOS, Linux, and WSL. Native Windows is not supported.
Building requires Rust 1.88 or newer; `copy` requires Git for protection checks.

| Setting | Default | Accepted values |
|---|---|---|
| `global_store` | `global.env` next to the selected config file | Absolute, `~/`, or config-directory-relative path |
| `markers` | Empty | Comma-separated extra environment variable names; built-in markers always remain active |
| `editor` | `$VISUAL`, then `$EDITOR`, then `vi` | Executable and arguments, e.g. `code --wait` |
| `agent_mode` | `auto` | `auto` or `always`; no protection-disabling value |

The config path defaults to `~/.config/agents-env/config`. An absolute
`XDG_CONFIG_HOME` selects `agents-env/config` under that directory first. If that
file is absent but the legacy config exists, the legacy config and its paths
are retained. Files are never moved or merged automatically. Empty or relative
XDG values are ignored as required by the
[XDG specification](https://specifications.freedesktop.org/basedir-spec/latest/).

From a human terminal:

```sh
agents-env config show
agents-env config set global_store '~/secrets/shared.env'
agents-env config set editor 'code --wait'
agents-env config set markers 'MY_AGENT_MODE,MY_OTHER_AGENT'
agents-env config reset editor
```

Changing settings does not create or move the secret store. Humans register
secrets with `agents-env edit`. Config writes preserve comments and other
settings, back up once per day, and use atomic 0600 writes. Unknown keys,
duplicate settings, and read failures produce errors rather than silently
falling back to defaults. Use `config reset global_store` instead of an empty
`global_store=` setting.

`agent_mode=always` also refuses `edit`, config changes, and `--no-mask`. To
return to `auto`, a human must edit the setting at `config path` directly.
Editor arguments support quoting; shell expansion, pipes, and command
substitution are not evaluated.

Placing a global store at `~/.env` also protects projects below that directory
from writes. Prefer the default location or a dedicated `~/secrets/` directory.

## Scope and accounts

**Scope and files.** The default scope is the global store. `-l`/`--local` reads `./.env`; `-f <name>` reads `./<name>`, which handles `.env.local`, `.env.production`, and the like.

```
agents-env -f .env.production get DATABASE
```

**Duplicate keys.** When a key has several accounts, the inline `# comment` is the tag. Operations where the choice matters (`run`, `copy`) require a unique match; an ambiguous one shows the candidate tags (not the values) and stops.

```
agents-env copy NOTION_API_KEY@demodev --to .env.local
```

## Write guard

`set`/`copy` only write `.env*` files in the current directory. The global store is unreachable by construction.

- No flag points the write target at the global scope.
- File names must be a bare `.env`/`.env.*`. Path separators are rejected, which rules out `../`, absolute paths, and `.bak` targets.
- `set` keys and `copy --as` names must match `[A-Za-z_][A-Za-z0-9_]*`, so newlines or extra assignments cannot be inserted into the file.
- The target is refused if it is a symlink, has hard links, or is the same file as the global store.
- Writing inside the global store's directory or any descendant is refused.
- Inside a git repo, a secret-bearing `copy` target must be both untracked and gitignored. Otherwise it is a hard error (no override; fix `.gitignore`).
- Backups of existing targets must also be untracked and ignored for both `set` and `copy`. Use `.env*` rather than only `.env`; a Git verification failure refuses the copy.

Every write makes a `<file>.YYMMDD.bak` backup first. From the second write of the day onward it keeps that day's first backup, since the state before the day's work is the recovery point. It then writes to an `O_NOFOLLOW` temp file and renames it into place. Backups also start with `.env`, so one `.env*` gitignore line covers them.

## Limitations

Masking is defense in depth, not a sandbox. It catches a secret the child prints verbatim, but not one the child re-encodes (base64, URL-encoding, splitting). `cat .env` and Claude Code's `@.env` inline reference bypass the tool and must be stopped by the harness deny rules. `doctor` checks whether `~/.claude/settings.json` denies `Read(**/.env)` and friends, so configure both layers to back each other up.

### Coding Assistant Support

Auto-detection uses environment variables verified from official docs, local CLI behavior, or public source, plus CLI names in the Unix parent-process chain. It checks at most three ancestors; renamed or more deeply wrapped launches use explicit opt-in.

| Tool | Support decision |
|---|---|
| xAI Grok CLI | Auto-detect: `grok` in the Unix parent-process chain. |
| Claude Code | Auto-detect: Claude environment markers or `claude` in the Unix parent-process chain. |
| OpenAI Codex CLI | Auto-detect: `CODEX_SANDBOX` or `codex` in the Unix parent-process chain, including sandbox bypass. |
| OpenCode | Auto-detect: `OPENCODE` or `opencode` in the Unix parent-process chain. |
| Google Antigravity CLI | Auto-detect: `ANTIGRAVITY_CONVERSATION_ID` or `agy` in the Unix parent-process chain. |
| Google Gemini CLI | Opt-in: no stable child-process marker verified. |
| Cursor CLI | Opt-in: no stable marker verified that distinguishes `cursor-agent` itself from commands it runs. |
| GitHub Copilot CLI | Opt-in: public CLI environment settings do not expose a stable child-process marker. |
| Kiro CLI / Amazon Q CLI successor | Opt-in: Amazon Q CLI has become Kiro CLI; no stable child-process marker verified. |
| Aider | Opt-in: no stable child-process marker verified. |
| Qwen Code | Opt-in: no stable child-process marker verified. |
| Cline CLI | Opt-in: no stable child-process marker verified. |
| Windsurf/Devin | Opt-in: set the env var in the IDE/cloud-agent shell that runs commands. |

For unverified tools, set this in the shell or wrapper that launches the agent:

```
export AGENTS_ENV_AGENT_MODE=1
```

You can also wrap one launch:

```
AGENTS_ENV_AGENT_MODE=1 cursor-agent
AGENTS_ENV_AGENT_MODE=1 qwen
AGENTS_ENV_AGENT_MODE=1 copilot
AGENTS_ENV_AGENT_MODE=1 gemini
AGENTS_ENV_AGENT_MODE=1 kiro
AGENTS_ENV_AGENT_MODE=1 aider
AGENTS_ENV_AGENT_MODE=1 cline
```

If your own harness can set a custom marker, register it in config:

```
agents-env config set markers MY_AGENT_MODE
MY_AGENT_MODE=1 agents-env get TAVILY
```

- **Detection is a signal, not a barrier.** Mode is decided from env markers and nearby parent CLI names, so an agent can reach human mode by hiding both. The intended threat is an honest agent that should not accidentally log a secret. Direct store access by a malicious agent is for the deny rules to stop, not this tool.
- **`{{KEY}}` is visible to a same-user `ps`.** The value lands in the child's argv, where another process of the same user can read it. On a shared machine, use env injection instead of `{{KEY}}` for sensitive values.
- **Parent-directory-swap TOCTOU.** The guard canonicalizes cwd and uses `O_NOFOLLOW` on the temp file, but a same-user attacker who renames a parent directory mid-write could still redirect it. Closing this fully needs directory-fd (`openat`/`renameat`) writes, planned for a later version. It is unreachable without local same-user write access, and with that access the secrets are already exposed.
- **Line endings normalize to LF.** Round-trips preserve comments, order, and spacing, but a CRLF file is rewritten with LF and gains a trailing newline.

## Install

Install and setup ship inside the bundled agent skill, so you can leave it to Claude Code or Codex. Manual install:

```
cargo install agents-env
# before it's on crates.io:
cargo install --git https://github.com/ai-native-engineer/agents-env
```

For a local checkout, use `cargo install --path . --locked` and put the Cargo
installation's `bin` directory on PATH. Store user-specific paths with
`config set global_store`; do not embed them in distributed files.

Validate with `cargo test --locked`, `cargo clippy --locked --all-targets -- -D warnings`,
and `cargo fmt --all -- --check`. CI checks macOS/Linux with stable and Rust 1.88.

## License

MIT
