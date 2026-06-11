# agents-env

[한국어](./README.md) · **English**

When you let an agent use an API key, the key value ends up in the conversation log. One `curl -v`, one stack trace, and it's there. That bugged me enough to build this.

agents-env lets an agent use secrets without seeing them. The value goes only into the child process; the agent's transcript keeps just the key name.

```
agents-env run TAVILY_API_KEY@work -- curl -H "Authorization: Bearer {{TAVILY_API_KEY}}" https://api/...
```

`{{TAVILY_API_KEY}}` turns into the real value only at the moment the command runs. Before that, and in the log, all that's there is the text `{{TAVILY_API_KEY}}`. If curl spits the key onto the screen, it comes out as `[masked:TAVILY_API_KEY]`.

No schema to define, no encrypted vault, no cloud account. Leave your `.env` where it is and put this on top.

## Why build another one

There are plenty of secret tools already: 1Password, doppler, infisical, vault. But they all stop at "inject the key into the process." None of them keep the value out of the AI's log while the agent still reads and writes `.env` files. I went through 14 of them; the best fit covered 3 of 7 things I wanted.

What was missing:

- **Masking the output too.** `doppler run` and `infisical run` inject the key but don't touch what the child prints. One `curl -v` leaks it. varlock and (paid) 1Password are about the only ones that mask.
- **Copying without looking.** Moving a global key into a project `.env` without the value passing through the agent's context: no tool does it. `dotenvx set` takes the value as an argument, which means you already saw it.
- **Knowing it's an agent on its own.** varlock needs a manual `--agent` flag. Forget it once and you leak that day. agents-env spots Claude Code and Codex automatically and defaults to hiding.
- **Keeping the human side and the agent side apart.** The human's global master `.env` can't be touched by this tool. On purpose (below).

## get is for finding, run is for using

Start with the part that trips people up. You'll want to write `mytool --key "$(agents-env get KEY)"`, but it won't work. `$(...)` pulls the value onto the shell command line, which is the agent's context. So in agent mode `get` hands back the key name and length, not the value.

To actually use a secret, you don't pull it out with `get`. You let `run` carry it into the child.

Masking only rewrites what comes out (child → you). What the program takes in is the real value. So the program runs fine, and the value is hidden only when it tries to come back to you.

## Commands

| Command | What it does |
|---|---|
| `get <pattern>` | Look up keys (substring match). Human gets `KEY=value`; agent gets `KEY [set, N chars] # tag` only. |
| `ls [pattern]` | Key names + tags. Never prints values, in any mode. |
| `run <KEY[@tag]…> -- <cmd>` | Inject into the child env, mask its output, fill in `{{KEY}}` in argv. `--all` injects the whole scope. |
| `set <KEY> <VALUE> --to <file>` | Write a non-secret value to a local file. Warns if it looks like a credential. |
| `copy <KEY[@tag]…> --to <file>` | Copy secrets from the global store into a local file; value is never printed. `--as NEWKEY` renames. |
| `edit` | Open the global store in `$EDITOR`. Human only; refused in agent mode and on non-TTY. |
| `doctor` | Check file permissions, gitignore coverage, stale backups, untagged duplicate keys, Claude Code deny rules. |

Full flags: `agents-env --help`.

### Scope and files

Default is the global store. `-l`/`--local` reads `./.env`; `-f <name>` reads `./<name>`, so you can manage `.env.local`, `.env.production`, and so on.

```
agents-env -f .env.production get DATABASE
```

### Same key, several accounts: `KEY@tag`

When a key has more than one account, the inline `# comment` is the tag. Operations where the choice matters (`run`, `copy`) need a unique match; an ambiguous one stops and shows you the candidate tags, not the values.

```
agents-env copy NOTION_API_KEY@demodev --to .env.local
```

## Write guard

`set`/`copy` only write `.env*` files in the current directory. The global store can't be reached at all:

- No flag points the write side at the global scope.
- File names have to be a bare `.env`/`.env.*`. Path separators are rejected, which rules out `../`, absolute paths, and `.bak` targets.
- The target is refused if it's a symlink, has hard links, or is the same file as the global store.
- Writing inside the global store's own directory is refused.
- Inside a git repo, a secret-bearing `copy` target has to be both untracked and gitignored. Otherwise it's a hard error, no override; fix `.gitignore`.

Every write makes a `<file>.YYMMDD.bak` backup first. On the second write of the day it keeps the first backup, since the state before you started that day is the one worth going back to. Then it writes to an `O_NOFOLLOW` temp file and renames it into place. Backups start with `.env` too, so one `.env*` gitignore line covers them.

## Limits (the honest version)

Masking is one more layer, not a sandbox. It catches a secret the child prints verbatim, but not one the child reshapes (base64, URL-encoding, splitting). `cat .env` and Claude Code's `@.env` inline reference skip this tool entirely; that's for the harness deny rules to stop. `doctor` checks whether your `~/.claude/settings.json` denies `Read(**/.env)` and the like, so put those in and let the two layers back each other up.

- **Auto-detection only works for Claude Code and Codex.** In Cursor, Aider, Windsurf, or a harness you wrote yourself, agents-env doesn't know it's talking to an agent. There, `get` prints values and `--no-mask` is allowed (`run`'s output masking still works everywhere). In those tools you turn it on yourself with `AGENTS_ENV_AGENT_MODE=1` in that environment's shell config, or a `markers=...` line in the config.
- **Detection is a signal, not a wall.** Mode comes from env markers, so an agent can unset them (`env -u CLAUDECODE …`) to force human mode, or pass `--no-mask`. That's fine for the threat I care about: an honest agent that shouldn't accidentally log a secret. A determined agent can just read `~/.dotfiles/.env` directly, and that's for the deny rules to stop, not this tool.
- **`{{KEY}}` is visible to a same-user `ps`.** The value lands in the child's argv, where another process of the same user can read it. On a shared box, use env injection instead of `{{KEY}}` for anything sensitive.
- **Parent-directory-swap TOCTOU.** The guard canonicalizes cwd and uses `O_NOFOLLOW` on the temp file, but a same-user attacker who renames a parent directory mid-write could still redirect it. Closing that fully needs directory-fd (`openat`/`renameat`) writes, which is planned. It's not reachable without local same-user write access, and if someone has that, your secrets are already exposed.
- **Line endings become LF.** Round-trips keep comments, order, and spacing, but a CRLF file is rewritten with LF and gets a trailing newline.

## Install

Install and setup live inside the bundled agent skill, so you can have Claude Code or Codex do it. By hand: `cargo install agents-env` (or `cargo install --git https://github.com/ai-native-engineer/agents-env` before it's on crates.io).

## License

MIT
