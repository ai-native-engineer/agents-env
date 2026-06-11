# agents-env

**한국어** · [English](./README.en.md)

에이전트한테 API 키를 쓰게 시키다 보면 키 값이 대화 로그에 그대로 박힌다. `curl -v` 한 번, 에러 스택 한 줄이면 끝이다. 그게 거슬려서 만들었다.

agents-env는 에이전트가 시크릿을 쓰되 보지는 못하게 한다. 값은 자식 프로세스 안으로만 들어가고, 에이전트 대화 기록엔 키 이름만 남는다.

```
agents-env run TAVILY_API_KEY@work -- curl -H "Authorization: Bearer {{TAVILY_API_KEY}}" https://api/...
```

`{{TAVILY_API_KEY}}`는 명령이 실제로 실행되는 순간에만 진짜 값으로 바뀐다. 그 전까지도, 로그에 남는 것도 `{{TAVILY_API_KEY}}`라는 글자뿐이다. 혹시 curl이 키를 화면에 뱉어도 `[masked:TAVILY_API_KEY]`로 가려진 다음에 나온다.

스키마 정의도, 암호화 vault도, 클라우드 계정도 없다. 쓰던 `.env`를 그대로 두고 그 위에 얹으면 된다.

## 왜 또 만들었나

시크릿 도구는 이미 많다. 1Password, doppler, infisical, vault. 그런데 다들 "프로세스에 키를 넣어준다"까지만 한다. "그 값이 AI 대화 로그에 안 남으면서, 그러는 동안 에이전트가 .env를 읽고 쓰기까지" 하는 건 없었다. 14개를 직접 까봤는데 제일 잘 맞는 것도 7개 요구 중 3개를 채웠다.

빠져 있던 것들:

- **출력까지 가리기.** `doppler run`, `infisical run`은 키를 넣어주긴 하는데 자식이 출력하는 건 안 막는다. `curl -v` 한 번이면 샌다. 출력을 가리는 건 varlock하고 유료 1Password 정도다.
- **값 안 보고 복사하기.** 전역 키를 프로젝트 `.env`로 옮길 때 값이 에이전트 컨텍스트를 안 거치는 도구가 없다. `dotenvx set`은 값을 인자로 받는다. 그러려면 이미 값을 본 거다.
- **에이전트인지 알아서 판단하기.** varlock은 `--agent`를 직접 붙여야 한다. 한 번 까먹으면 그날 샌다. agents-env는 Claude Code랑 Codex를 자동으로 알아채고 기본이 가리는 쪽이다.
- **사람 거와 에이전트 거를 갈라두기.** 사람이 쓰는 전역 마스터 `.env`는 이 도구로 못 건드린다. 일부러 막았다(아래 참고).

## get은 찾기, run은 쓰기

제일 헷갈리는 데부터. `mytool --key "$(agents-env get KEY)"` 이렇게 쓰고 싶겠지만 안 된다. `$(...)`는 값을 셸 명령줄에, 그러니까 에이전트 컨텍스트에 끌어다 놓는다. 그래서 에이전트 모드의 `get`은 값 대신 키 이름하고 길이만 돌려준다.

값을 진짜 쓸 땐 `get`으로 꺼내는 게 아니라 `run`이 자식 안으로 흘려보내게 둔다.

마스킹은 나오는 쪽(자식 → 나)만 고친다. 프로그램이 받는 쪽은 진짜 값 그대로다. 그래서 프로그램은 멀쩡히 돌고, 그 값이 화면으로 되돌아올 때만 가려진다.

## 명령

| 명령 | 하는 일 |
|---|---|
| `get <패턴>` | 키 조회(부분 일치). 사람한텐 `KEY=value`, 에이전트한텐 `KEY [set, N chars] # tag`만. |
| `ls [패턴]` | 키 이름 + 태그. 어떤 모드에서도 값은 안 찍는다. |
| `run <KEY[@tag]…> -- <명령>` | 자식 env에 주입, 출력 마스킹, argv의 `{{KEY}}` 치환. `--all`은 스코프 전체 주입. |
| `set <KEY> <VALUE> --to <파일>` | 로컬 파일에 비밀 아닌 값 기록. 크리덴셜처럼 생겼으면 경고한다. |
| `copy <KEY[@tag]…> --to <파일>` | 전역 store의 시크릿을 로컬 파일로 복사. 값은 안 찍힌다. `--as NEWKEY`로 이름 변경. |
| `edit` | 전역 store를 `$EDITOR`로 연다. 사람 전용. 에이전트 모드·비TTY에선 거부. |
| `doctor` | 점검: 파일 권한, gitignore 커버리지, 오래된 백업, 태그 없는 중복 키, Claude Code deny 규칙. |

전체 플래그는 `agents-env --help`.

### 스코프와 파일

기본은 전역 store다. `-l`/`--local`은 `./.env`를, `-f <이름>`은 `./<이름>`을 읽어 `.env.local`, `.env.production` 같은 여러 파일을 다룬다.

```
agents-env -f .env.production get DATABASE
```

### 같은 키가 여러 개일 때: `KEY@tag`

한 키에 계정이 여럿이면 인라인 `# comment`가 태그가 된다. `run`, `copy`처럼 결과가 갈리는 작업은 유일 매치를 요구하고, 모호하면 후보 태그(값 말고)를 보여주며 멈춘다.

```
agents-env copy NOTION_API_KEY@demodev --to .env.local
```

## 쓰기 가드

`set`/`copy`는 현재 디렉토리의 `.env*` 파일만 쓴다. 전역 store는 애초에 닿을 수가 없다.

- 쓰기 쪽을 전역으로 향하게 하는 플래그가 없다.
- 파일명은 bare `.env`/`.env.*`만 받는다. 경로 구분자를 막아서 `../`, 절대경로, `.bak` 타겟이 걸러진다.
- 타겟이 심볼릭링크거나 하드링크를 가졌거나 전역 store와 같은 파일이면 거부한다.
- 전역 store 디렉토리 안에서 쓰는 것도 거부한다.
- git 레포 안이면 시크릿을 쓰는 `copy` 타겟이 추적 안 됨 + gitignore 둘 다여야 한다. 아니면 그냥 막는다(override 없음, `.gitignore`를 고쳐야 한다).

쓰기 전엔 항상 `<파일>.YYMMDD.bak` 백업을 먼저 만든다. 같은 날 두 번째부턴 첫 백업을 유지한다. 그날 작업을 시작하기 전 상태가 되돌릴 만한 지점이기 때문이다. 그다음 `O_NOFOLLOW` 임시 파일에 쓰고 rename으로 바꿔치기한다. 백업도 `.env`로 시작하니 `.env*` gitignore 한 줄이면 같이 덮인다.

## 한계 (솔직하게)

마스킹은 한 겹 더 막아주는 거지 샌드박스가 아니다. 자식이 출력한 시크릿 원문은 잡지만, 자식이 모양을 바꾼 값(base64, URL 인코딩, 쪼개기)은 못 잡는다. `cat .env`나 Claude Code의 `@.env` 인라인 참조는 이 도구를 통째로 건너뛴다. 그쪽은 하네스 deny 규칙이 막아야 한다. `doctor`가 `~/.claude/settings.json`에 `Read(**/.env)` 같은 deny가 있는지 봐주니, 두 겹이 서로를 받치도록 넣어두는 걸 권한다.

- **자동 감지는 Claude Code와 Codex만 된다.** Cursor, Aider, Windsurf, 직접 만든 하네스에선 agents-env가 에이전트인 줄 모른다. 그러면 `get`이 값을 그대로 찍고 `--no-mask`도 허용된다(`run`의 출력 마스킹 자체는 어디서든 동작한다). 이런 도구에선 그 환경 셸 설정에 `AGENTS_ENV_AGENT_MODE=1`을 넣거나 config에 `markers=...`를 추가해 직접 켜야 한다.
- **감지는 신호지 벽이 아니다.** 모드는 env 마커로 읽으니, 에이전트가 마커를 지워서(`env -u CLAUDECODE …`) 사람 모드를 강제하거나 `--no-mask`를 쓸 수 있다. 노리는 위협은 따로 있다. 실수로 시크릿을 로그에 남기면 안 되는 정직한 에이전트다. 작정한 에이전트는 어차피 `~/.dotfiles/.env`를 직접 읽으면 그만이고, 그건 이 도구가 아니라 deny 규칙이 막을 일이다.
- **`{{KEY}}`는 같은 유저의 `ps`에 보인다.** 값이 자식 argv에 들어가서 같은 유저의 다른 프로세스가 읽을 수 있다. 공유 머신에서 민감한 건 `{{KEY}}` 말고 env 주입을 써라.
- **부모 디렉토리 스왑 TOCTOU.** 가드가 cwd를 canonicalize하고 임시 파일에 `O_NOFOLLOW`를 쓰지만, 같은 유저 공격자가 쓰는 도중에 부모 디렉토리를 rename하면 우회할 수 있다. 완전히 막으려면 디렉토리 fd(`openat`/`renameat`) 방식이 필요한데 다음 버전 예정이다. 로컬 같은-유저 쓰기 권한 없이는 못 하고, 그 권한이 있으면 시크릿은 이미 노출된 거다.
- **줄 끝은 LF로 통일된다.** 라운드트립이 주석·순서·간격은 보존하지만 CRLF 파일은 LF로 다시 쓰이고 끝에 개행이 붙는다.

## 설치

설치랑 설정은 같이 들어 있는 에이전트 스킬이 안다. Claude Code나 Codex한테 시키면 된다. 손으로 할 거면 `cargo install agents-env`(crates.io 등록 전이면 `cargo install --git https://github.com/ai-native-engineer/agents-env`).

## 라이선스

MIT
