---
name: agents-env
description: >-
  Use and manage secrets/env vars through the `agents-env` CLI WITHOUT ever
  putting the value into your own context. Use whenever you need to pass an API
  key, token, or secret to a command (curl, an SDK CLI, a test runner), build a
  project .env / .env.local from stored secrets, or find which env keys exist —
  without printing the value. Core verbs: run (inject into a child + mask its
  output + {{KEY}} argv substitution), copy (global store → local file,
  value never passes through you), get / ls (discovery; values hidden in agent
  mode). The global master store is read-only for agents — never try to write it.
---

# agents-env

`agents-env` lets you USE a secret without SEEING its value. The value flows
into the child process you launch; your transcript only ever holds key names and
`{{KEY}}` placeholders. Full flags: `agents-env --help`.

## Setup (run once, if `agents-env: command not found`)

Install the CLI, then point the global store at the user's existing master `.env`:

```
cargo install agents-env          # or: brew install ai-native-engineer/tap/agents-env (planned)
# fallback if not on crates.io yet:
cargo install --git https://github.com/ai-native-engineer/agents-env

mkdir -p ~/.config/agents-env
# only if no config exists — never overwrite an existing one:
[ -f ~/.config/agents-env/config ] || echo 'global_store=~/.dotfiles/.env' > ~/.config/agents-env/config
```

Set `global_store=` to wherever the user keeps their master `.env` (ask if unsure;
default when unset is `~/.config/agents-env/global.env`). Then `agents-env doctor`
to confirm setup and check the harness deny rules. This works the same for Grok,
Codex, OpenCode, Claude Code, and AGY — only the optional plugin (`claude plugin install
agents-env@agents-env`) is Claude-specific.

## Assistant support

Agent mode is automatic from verified runtime markers and nearby Unix parent CLI names:

| Assistant | Decision |
|---|---|
| xAI Grok CLI | Auto-detect from the `grok` parent CLI. |
| Claude Code | Auto-detect from Claude markers or the `claude` parent CLI. |
| OpenAI Codex CLI | Auto-detect from `CODEX_SANDBOX` or the `codex` parent CLI, including sandbox bypass. |
| OpenCode | Auto-detect from `OPENCODE` or the `opencode` parent CLI. |
| Google Antigravity CLI | Auto-detect from `ANTIGRAVITY_CONVERSATION_ID` or the `agy` parent CLI. |
| Google Gemini CLI | Opt in with `AGENTS_ENV_AGENT_MODE=1` or a user-owned `markers=` entry. |
| Cursor CLI | Opt in; no stable child-command marker is documented. |
| GitHub Copilot CLI | Opt in; no stable child-command marker is documented. |
| Kiro CLI / Amazon Q CLI successor | Opt in; Amazon Q CLI has become Kiro CLI, and no stable marker is documented. |
| Aider | Opt in; no stable child-command marker is documented. |
| Qwen Code | Opt in; no stable child-command marker is documented. |
| Cline CLI | Opt in; no stable child-command marker is documented. |
| Windsurf/Devin | Opt in by setting the env var in the IDE/cloud-agent shell. |

For any opt-in assistant, launch the agent from a shell with:

```
export AGENTS_ENV_AGENT_MODE=1
```

Or wrap a single CLI launch:

```
AGENTS_ENV_AGENT_MODE=1 cursor-agent
AGENTS_ENV_AGENT_MODE=1 qwen
AGENTS_ENV_AGENT_MODE=1 copilot
AGENTS_ENV_AGENT_MODE=1 gemini
AGENTS_ENV_AGENT_MODE=1 kiro
AGENTS_ENV_AGENT_MODE=1 aider
AGENTS_ENV_AGENT_MODE=1 cline
```

If the harness sets its own stable marker, add it to config:

```
mkdir -p ~/.config/agents-env
printf '\nmarkers=MY_AGENT_MODE\n' >> ~/.config/agents-env/config
MY_AGENT_MODE=1 agents-env get TAVILY
```

## Mental model: `get` is discovery, `run` is use

The wrong instinct is `mytool --key "$(agents-env get KEY)"`. That is broken on
purpose — command substitution routes the value through the shell line into your
context, so `get` returns only metadata in agent mode (key name + length), never
the value. To actually use a secret, let `run` carry it into the child instead.

Masking only rewrites the **output** stream (child → you); it never touches the
**input** the program receives. So the program gets the real value and works
normally; you just can't see it echoed back.

## Core workflows

Discover keys (values stay hidden):
```
agents-env ls tavily            # key names + account tags only
agents-env get gemini           # same, with [set, N chars] metadata
```

Pass a secret to a command — two shapes:
```
# program takes it as a CLI flag → {{KEY}} placeholder (resolved at exec)
agents-env run TAVILY_API_KEY@work -- curl -H "Authorization: Bearer {{TAVILY_API_KEY}}" https://api/...

# program reads it from the environment → just inject, no placeholder
agents-env run OPENAI_API_KEY -- some-cli chat
```

Build a project env file from the global store (value never passes through you):
```
agents-env copy GEMINI_API_KEY@personal --to .env.local
agents-env set NEXT_PUBLIC_URL http://localhost:3000 --to .env.local   # non-secret literals
```

## Selectors and scope

- `KEY@tag` picks one account when a key has several; the tag is the inline
  `# comment`. An ambiguous selector errors and lists the tags (no values) —
  read the tags, re-run with `@tag`.
- Default scope is the global master store. `-l` / `-f <name>` reads a local
  file (`.env`, `.env.local`, …) instead.

## Rules and limits

- **Never write the global store.** `set`/`copy` only write local `.env*` files
  in the cwd; the global store is human-only (a human edits it with
  `agents-env edit`). Don't fight the write guard — fix the actual target.
- **Masking is defense in depth, not a sandbox.** It catches verbatim values,
  not re-encoded ones (base64, splitting). Reading `.env` files directly still
  exposes secrets — that is the harness deny-rule layer's job, not this tool's.
- Run `agents-env doctor` to audit protection (file perms, gitignore, deny rules).
