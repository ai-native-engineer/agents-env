# agents-env

**한국어** · [English](./README.en.md)

AI 코딩 에이전트가 시크릿 값을 **한 번도 보지 않고** 환경변수를 사용하게 해주는 CLI.

스키마도, 암호화 vault도, 클라우드 계정도 필요 없다. 쓰던 평문 `.env`를 그대로 둔 채 그 위에 얹는 zero-config 안전 레이어다. 같은 명령이 터미널 앞의 사람에게는 값을 보여주고, 에이전트(Claude Code, Codex 등)에게는 가린다. 그래서 워크플로를 사람용/에이전트용 둘로 쪼갤 필요가 없다.

```
agents-env run TAVILY_API_KEY@work -- curl -H "Authorization: Bearer {{TAVILY_API_KEY}}" https://api/...
```

값은 자식 프로세스 안으로만 들어간다. argv의 `{{TAVILY_API_KEY}}`는 실행되는 순간 치환되므로 대화 기록에는 플레이스홀더만 남는다. 자식이 `curl -v`나 스택트레이스로 키를 토해내도 실시간으로 `[masked:TAVILY_API_KEY]`로 바뀌어 에이전트 컨텍스트에 닿기 전에 가려진다.

## 왜 필요한가

기존 시크릿 도구는 전부 "프로세스에 시크릿을 주입"하는 문제를 푼다. "값이 AI 대화 기록에 절대 남지 않으면서, 그러는 동안 에이전트가 env 파일을 읽고 쓰기까지" 하는 건 아무도 풀지 않았다. 14개 도구를 조사했을 때 이 요구사항의 최고 커버리지가 3/7이었다.

- **출력 마스킹** — `doppler`/`infisical run`은 주입은 하지만 자식 출력을 마스킹하지 않는다. `curl -v` 한 번이면 값이 샌다. 마스킹하는 건 varlock과 (유료) 1Password뿐이다.
- **값 비경유 복사** — 전역 시크릿을 로컬 `.env`로 옮길 때 값이 호출자를 거치지 않는 도구가 없다. `dotenvx set`은 값을 인자로 받는다. 즉 에이전트가 이미 값을 본 것이다.
- **에이전트 자동 감지** — varlock은 수동 `--agent` 플래그가 필요하다. 한 번 까먹으면 샌다. agents-env는 Claude Code(`CLAUDECODE`/`CLAUDE_CODE_ENTRYPOINT`/`AI_AGENT`)와 Codex(`CODEX_SANDBOX`)를 자동 감지하고 기본값이 안전하다. 다른 하네스는 `AGENTS_ENV_AGENT_MODE=1` 또는 config의 `markers=A,B,C`로 등록한다.
- **비대칭 쓰기 가드** — 사람 소유의 전역 store는 이 도구로 구조적으로 쓸 수 없다(아래 참조). 이걸 모델링한 도구는 없다.

## 동작 방식

핵심은 **`get`은 발견, `run`은 사용**이다.

흔한 실수는 `mytool --key "$(agents-env get KEY)"`처럼 쓰는 것이다. 이건 일부러 막아뒀다. 명령 치환은 값을 셸 명령줄로, 즉 에이전트 컨텍스트로 끌어들이기 때문에 `get`은 에이전트 모드에서 값 대신 메타데이터(키 이름 + 길이)만 돌려준다. 실제로 시크릿을 쓰려면 `run`이 값을 자식 안으로 흘려보내게 한다.

마스킹은 **출력** 스트림(자식 → 나)만 다시 쓴다. 프로그램이 **받는 입력**은 절대 건드리지 않는다. 그래서 프로그램은 진짜 값을 받아 정상 동작하고, 다만 그 값이 되돌아 출력될 때만 가려진다.

## 명령

| 명령 | 하는 일 |
|---|---|
| `get <pattern>` | 키 조회(부분 일치). 사람: `KEY=value` 출력. 에이전트: `KEY [set, N chars] # tag`만. |
| `ls [pattern]` | 키 이름 + 태그. 어떤 모드에서도 값은 출력 안 함. |
| `run <KEY[@tag]…> -- <cmd>` | 자식 env에 주입, 출력 마스킹, argv의 `{{KEY}}` 치환. `--all`은 스코프 전체 주입. |
| `set <KEY> <VALUE> --to <file>` | 로컬 파일에 **비밀 아닌** 리터럴 기록. 값이 크리덴셜처럼 보이면 경고. |
| `copy <KEY[@tag]…> --to <file>` | 전역 store의 시크릿을 로컬 파일로 복사 — 값은 출력되지 않음. `--as NEWKEY`로 이름 변경. |
| `edit` | 전역 store를 `$EDITOR`로 연다. **사람 전용** — 에이전트 모드·비TTY에서 거부. |
| `doctor` | 감사: 파일 권한, gitignore 커버리지, 오래된 백업, 태그 없는 중복 키, Claude Code deny 규칙. |

전체 플래그는 `agents-env --help`.

### 스코프와 파일

기본 스코프는 전역 store다. `-l`/`--local`은 `./.env`를, `-f <name>`은 `./<name>`(로컬 함축)을 읽어 `.env.local`, `.env.production` 등 여러 파일을 다룬다.

```
agents-env -f .env.production get DATABASE
```

### 중복 키: `KEY@tag`

한 키에 계정이 여럿이면 인라인 `# comment`가 태그가 된다. 결정적 작업(`run`, `copy`)은 유일 매치를 요구하며, 모호한 셀렉터는 후보 태그(값은 절대 아님)와 함께 에러를 낸다.

```
agents-env copy NOTION_API_KEY@demodev --to .env.local
```

## 쓰기 가드

`set`/`copy`는 현재 디렉토리의 `.env*` 파일만 쓸 수 있다. 전역 store는 구조적으로 도달 불가능하다.

- 쓰기 쪽을 전역 스코프로 향하게 하는 플래그가 없다.
- 파일명은 bare `.env`/`.env.*`만 허용 — 경로 구분자가 거부되므로 `../`, 절대경로, `.bak` 타겟이 차단된다.
- 타겟이 **심볼릭링크**거나 **하드링크**를 가졌거나 전역 store와 `samefile`이면 거부.
- 전역 store 디렉토리 안에서의 쓰기 거부.
- git 레포 안에서는 시크릿을 쓰는 `copy` 타겟이 untracked **이면서** gitignore되어 있어야 한다. 아니면 하드 에러(override 없음, `.gitignore`를 고쳐야 함).

모든 쓰기는 먼저 `<file>.YYMMDD.bak` 백업을 만들고(당일 첫 백업이 이김 — 그날 작업 시작 상태가 의미 있는 복구 지점), `O_NOFOLLOW` 임시 파일 + rename으로 원자적으로 쓴다. 백업은 `.env` 프리픽스라 `.env*` gitignore 한 줄로 함께 덮인다.

## 위협 모델 (정직한 한계)

마스킹은 **심층 방어이지 샌드박스가 아니다**. 자식 출력의 시크릿 값 원문은 잡지만, 자식이 재인코딩한 값(base64, URL 인코딩, 분할)은 못 잡는다. `cat .env`와 Claude Code의 `@.env` 인라인 참조는 이 도구를 통째로 우회한다. 그 구멍은 하네스 deny 레이어가 막는다. `doctor`가 `~/.claude/settings.json`에 `Read(**/.env)`/`Read(**/.env.*)` deny가 있는지 확인하니, 두 레이어가 서로를 덮도록 추가하라.

알려진 한계(설계상 또는 연기됨):

- **에이전트 감지는 신호이지 벽이 아니다.** 모드는 env 마커로 읽으므로, 에이전트가 마커를 지워(`env -u CLAUDECODE …`) 사람 모드를 강제하거나 `--no-mask`를 쓸 수 있다. 실제 위협 모델에선 괜찮다 — 대상은 *실수로* 시크릿을 로깅하면 안 되는 *정직한* 에이전트다. *악의적* 에이전트는 어차피 `~/.dotfiles/.env`를 직접 읽을 수 있고, 그건 이 도구가 아니라 deny 레이어의 몫이다.
- **`{{KEY}}` argv 치환은 같은 유저의 `ps`에 보인다.** 값이 자식의 argv에 들어가 같은 유저의 다른 프로세스가 읽을 수 있다. 공유 머신에서 민감한 건 env 주입(`{{KEY}}` 없이)을 써라.
- **부모 디렉토리 스왑 TOCTOU.** 쓰기 가드는 cwd를 canonicalize하고 임시 파일에 `O_NOFOLLOW`를 쓰지만, 같은 유저 공격자가 쓰기 도중 부모 디렉토리를 rename하면 우회할 수 있다. 완전 차단은 디렉토리 fd(`openat`/`renameat`) 쓰기가 필요하며 다음 버전 예정. 로컬 같은-유저 쓰기 권한 없이는 도달 불가능하고, 그 시점엔 이미 시크릿이 노출된 상태다.
- **줄 끝은 LF로 정규화된다.** 라운드트립은 주석·순서·간격을 보존하지만 CRLF 파일은 LF로 다시 쓰이고 끝에 개행이 추가된다.

## 설치

설치·설정은 동봉된 에이전트 스킬에 들어 있어 Claude Code나 Codex에게 시키면 된다. 수동 설치는 `cargo install agents-env`(crates.io 등록 전이면 `cargo install --git https://github.com/ai-native-engineer/agents-env`).

## 라이선스

MIT
