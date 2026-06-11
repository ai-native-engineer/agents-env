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
- **Automatic agent detection** — it recognizes Claude Code and Codex and hides values by default, with no manual flag.
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

Full options: `agents-env --help`.

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
- The target is refused if it is a symlink, has hard links, or is the same file as the global store.
- Writing inside the global store's directory is refused.
- Inside a git repo, a secret-bearing `copy` target must be both untracked and gitignored. Otherwise it is a hard error (no override; fix `.gitignore`).

Every write makes a `<file>.YYMMDD.bak` backup first. From the second write of the day onward it keeps that day's first backup, since the state before the day's work is the recovery point. It then writes to an `O_NOFOLLOW` temp file and renames it into place. Backups also start with `.env`, so one `.env*` gitignore line covers them.

## Limitations

Masking is defense in depth, not a sandbox. It catches a secret the child prints verbatim, but not one the child re-encodes (base64, URL-encoding, splitting). `cat .env` and Claude Code's `@.env` inline reference bypass the tool and must be stopped by the harness deny rules. `doctor` checks whether `~/.claude/settings.json` denies `Read(**/.env)` and friends, so configure both layers to back each other up.

- **Auto-detection supports only Claude Code and Codex.** In Cursor, Aider, Windsurf, or a custom harness, the tool cannot tell it is talking to an agent. There, `get` prints values and `--no-mask` is allowed (`run`'s output masking still works everywhere). In those tools, enable it yourself with `AGENTS_ENV_AGENT_MODE=1` in that environment's shell config, or a `markers=...` line in the config.
- **Detection is a signal, not a barrier.** Mode is decided from env markers, so an agent can remove them (`env -u CLAUDECODE …`) or pass `--no-mask` to force human mode. The intended threat is an honest agent that should not accidentally log a secret. A malicious agent can read `~/.dotfiles/.env` directly, which is for the deny rules to stop, not this tool.
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

## License

MIT
