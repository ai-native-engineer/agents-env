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
