<h1 align="center">agents-env</h1>

<p align="center">AI 코딩 에이전트가 시크릿 값을 노출하지 않고 환경변수를 사용하게 하는 CLI</p>

<p align="center">
  <a href="https://crates.io/crates/agents-env"><img src="https://img.shields.io/crates/v/agents-env.svg" alt="crates.io"></a>
  <a href="./LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue.svg" alt="license"></a>
  <img src="https://img.shields.io/badge/built%20with-Rust-orange.svg" alt="rust">
</p>

<p align="center"><b>한국어</b> · <a href="./README.en.md">English</a></p>

---

에이전트에게 API 키를 넘기면 그 값이 대화 로그에 남는다. `curl -v` 출력이나 에러 스택 한 줄로도 노출된다. agents-env는 값을 자식 프로세스로만 전달하고, 에이전트의 대화 기록에는 키 이름만 남긴다. 스키마나 암호화 vault, 클라우드 계정 없이 기존 `.env` 위에 바로 얹어 쓴다.

```
agents-env run TAVILY_API_KEY@work -- curl -H "Authorization: Bearer {{TAVILY_API_KEY}}" https://api/...
```

`{{TAVILY_API_KEY}}`는 명령이 실행되는 순간에만 실제 값으로 치환된다. 대화 로그에 남는 것은 `{{TAVILY_API_KEY}}` 문자열뿐이고, curl이 키를 출력하더라도 `[masked:TAVILY_API_KEY]`로 가려진 뒤에 나온다.

## 특징

환경변수·시크릿 관리 도구는 많지만, 값을 AI 대화 로그에 남기지 않으면서 에이전트가 `.env`를 읽고 쓰게 하는 것은 드물다. 비슷한 도구 14개를 검토했을 때 아래 네 가지를 모두 갖춘 것은 없었다.

- **출력 마스킹**: 주입한 시크릿이 자식의 stdout/stderr에 나타나면 실시간으로 `[masked:KEY]`로 치환한다. `doppler run`·`infisical run`은 주입만 하고 출력은 막지 않는다.
- **값 비경유 복사**: `copy`는 전역 store의 시크릿을 로컬 `.env`로 옮기되, 값이 에이전트 컨텍스트를 거치지 않는다.
- **에이전트 모드 감지**: Grok, Codex, OpenCode, Claude Code, AGY는 런타임 마커나 부모 CLI 이름으로 자동 인식하고, 다른 어시스턴트는 `AGENTS_ENV_AGENT_MODE=1` 또는 `markers=`로 명시 opt-in한다.
- **비대칭 쓰기 가드**: 기존 전역 키는 사람이 관리한다. 에이전트는 `generate`로 값이 노출되지 않는 새 무작위 키만 추가할 수 있다.

## 동작 방식

`get`은 키를 찾는 명령, `run`은 키를 쓰는 명령이다. `mytool --key "$(agents-env get KEY)"`처럼 명령 치환으로 값을 꺼내 쓸 수는 없다. `$(...)`는 값을 셸 명령줄, 즉 에이전트 컨텍스트로 가져오기 때문에, 에이전트 모드의 `get`은 값 대신 키 이름과 길이만 반환한다. 값은 `run`이 자식 프로세스로 직접 전달한다.

마스킹은 출력 스트림(자식 → 호출자)만 처리하고 프로그램이 받는 입력은 바꾸지 않는다. 따라서 프로그램은 실제 값으로 정상 동작하며, 그 값이 출력으로 되돌아올 때만 가려진다.

## 사용법

| 명령 | 설명 |
|---|---|
| `get <pattern>` | 키 조회(부분 일치). 사람에게는 `KEY=value`, 에이전트에게는 `KEY [set, N chars] # tag`만 출력. |
| `ls [pattern]` | 키 이름과 태그. 어떤 모드에서도 값은 출력하지 않음. |
| `run <KEY[@tag]…> -- <cmd>` | 자식 env에 주입, 출력 마스킹, argv의 `{{KEY}}` 치환. `--all`은 스코프 전체 주입. |
| `set <KEY> <VALUE> --to <file>` | 로컬 파일에 비밀이 아닌 값 기록. 크리덴셜 형태면 경고. |
| `copy <KEY[@tag]…> --to <file>` | 전역 store의 시크릿을 로컬 파일로 복사. 값은 출력하지 않음. `--as NEWKEY`로 키 이름 변경. |
| `generate <KEY>` | 설정된 전역 store에 새 무작위 키를 추가. 기존 이름은 거부하며, 값은 사람/에이전트 모두에게 출력하지 않음. |
| `edit` | 전역 store를 `$EDITOR`로 연다. 사람 전용이며 에이전트 모드·비TTY에서는 거부. |
| `doctor` | 전역 저장소 문법/읽기 오류, 파일 권한, gitignore, 오래된 백업, 태그 없는 중복 키, Claude Code deny 규칙 점검. 값이나 문제 행 원문은 출력하지 않음. |
| `config show` / `config path` | 시크릿 파일을 읽지 않고 유효 설정과 설정 파일 위치 조회. |
| `config set <key> <value>` / `config reset <key>` | 설정 한 항목을 변경하거나 기본값으로 복원. 에이전트 모드에서는 변경 거부. |

전체 옵션은 `agents-env --help`.

## 설정과 기본값

macOS, Linux, WSL의 Unix 환경을 대상으로 한다. Windows 네이티브는 지원하지
않는다. 빌드에는 Rust 1.88 이상이 필요하고, `copy`의 보호 검증에는 Git이 필요하다.

| 설정 | 기본값 | 허용 값 |
|---|---|---|
| `global_store` | 설정 파일과 같은 디렉터리의 `global.env` | 절대 경로, `~/` 경로, 설정 디렉터리 기준 상대 경로 |
| `markers` | 빈 목록 | 쉼표로 나눈 추가 환경변수 이름. 내장 감지 마커는 항상 유지 |
| `editor` | `$VISUAL`, `$EDITOR`, `vi` 순서 | 실행파일과 인자. 예: `code --wait` |
| `agent_mode` | `auto` | `auto` 또는 `always`. 보호를 끄는 값은 없음 |

기본 설정 파일은 `~/.config/agents-env/config`다. 절대 경로인
`XDG_CONFIG_HOME`이 있으면 그 아래 `agents-env/config`를 먼저 확인한다.
그 파일이 없고 기존 `~/.config/agents-env/config`가 있으면 기존 설정을
사용한다. 파일을 자동으로 이동하거나 합치지 않는다. 상대/빈 XDG 값은
[XDG 명세](https://specifications.freedesktop.org/basedir-spec/latest/)에 따라 무시한다.

사람이 실행하는 터미널에서 다음처럼 설정할 수 있다.

```sh
agents-env config show
agents-env config set global_store '~/secrets/shared.env'
agents-env config set editor 'code --wait'
agents-env config set markers 'MY_AGENT_MODE,MY_OTHER_AGENT'
agents-env config reset editor
```

설정 변경은 원래 `.env`를 이동하거나 생성하지 않는다. 외부 서비스에서 발급받은
시크릿과 기존 키 수정은 사람이 `agents-env edit`로 처리한다. 자체 서비스용 새
무작위 키는 `generate`로 만들 수 있다. `config set/reset`은 주석과 나머지 설정을
보존하며 변경 전 일별 백업과 0600 원자 쓰기를 사용한다. 알 수 없는 키,
중복 설정, 읽기 오류는 명시적으로 실패하며 조용히 기본값으로 대체하지 않는다.
`global_store=`를 비우는 대신 `config reset global_store`로 복원할 수 있다.

`agent_mode=always`는 모든 실행을 에이전트로 취급하므로 `edit`, 설정 변경,
`--no-mask`도 거부한다. `auto`로 되돌릴 때는 사람이 `config path`가 가리키는
설정 파일에서 해당 줄을 직접 수정해야 한다. 편집기 인자는 따옴표를 지원하지만
셸 변수, 파이프, 명령 치환은 실행하지 않는다.

전역 파일을 `~/.env`처럼 넓은 디렉터리에 두면 그 하위 프로젝트의 쓰기도
보호 가드에 막힌다. 기본 위치나 `~/secrets/` 같은 전용 디렉터리를 권장한다.

## 스코프와 계정

**스코프와 파일.** 기본 스코프는 전역 store다. `-l`/`--local`은 `./.env`를, `-f <name>`은 `./<name>`을 읽어 `.env.local`, `.env.production` 등 여러 파일을 다룬다.

읽기 파서는 기존 파일의 숫자로 시작하는 이름도 인식하며 이름이나 값을 바꾸지 않는다. 새 이름을 받는 `set`, `generate`, `copy --as`는 이식 가능한 `[A-Za-z_][A-Za-z0-9_]*` 형식을 계속 요구한다. 숫자로 시작하는 환경변수는 `$NAME` 셸 문법으로 접근하지 말고 프로그램의 환경변수 API로 사용한다.

```
agents-env -f .env.production get DATABASE
```

**중복 키.** 한 키에 계정이 여럿이면 인라인 `# comment`가 태그가 된다. `run`, `copy`처럼 결과가 갈리는 작업은 유일 매치를 요구하며, 모호하면 후보 태그(값 제외)를 보여주고 멈춘다.

```
agents-env copy NOTION_API_KEY@demodev --to .env.local
```

## 값을 보지 않고 새 키 생성

```sh
agents-env generate SERVICE_API_KEY
agents-env run SERVICE_API_KEY -- my-service
```

`generate`는 OS 난수 32바이트를 64자리 소문자 hex로 저장한다. 입력은 키 이름뿐이고 값, 태그, 덮어쓰기, 교체 옵션은 받지 않는다. `--local`/`--file`도 거부하며 `get`/`run`과 같은 설정된 전역 경로에만 쓴다. 외부 서비스의 API 키를 발급하는 기능은 아니다.

- 기존 키가 있으면 빈 값, 대소문자만 다른 이름, 계정 태그가 있는 중복까지 모두 거부한다. 기존 키를 주석 처리하거나 갱신하지 않는다.
- 원본 바이트와 주석, CRLF, 줄 순서를 그대로 두고 마지막에 새 항목만 추가한다. 마지막 개행이 없으면 구분 개행을 추가한다. 완성된 내용을 다시 파싱해 기존 항목의 값과 태그가 같고 새 키가 유일하게 조회되는지 확인한 뒤 저장한다.
- 파싱할 수 없는 줄, 잘못된 UTF-8, NUL 또는 추가 후 의미가 바뀌는 원문이 있으면 파일과 백업을 건드리지 않고 실패한다. 오류에는 값 대신 줄 번호/원인만 표시한다. 주석은 설명과 기존 계정 선택용이지 중복 해결 수단이 아니다.
- 문법 오류로 중단되면 `agents-env doctor`로 같은 파서의 값 없는 진단을 확인한다. 문제가 있는 행의 원문을 출력하거나 무시하지 않는다.
- 잠금은 동시에 실행한 `generate`와 `edit`를 직렬화한다. 사용 중이면 종료 코드 2로 거부하므로 완료 후 재시도한다. 같은 이름으로 재시도해도 이미 만든 키는 바뀌지 않는다.
- 파일과 백업은 0600, 새 상위 폴더는 0700으로 만들고, 링크/특수 파일과 Git 보호가 없는 대상은 거부한다. 임시 파일도 Git에서 제외되어야 한다. 저장 전 백업과 원자적 교체 후 재조회 검증을 거친다.

## 쓰기 가드

`set`/`copy`는 현재 디렉토리의 `.env*` 파일만 쓴다. 전역 store는 구조적으로 도달할 수 없다.

- 쓰기 대상을 전역 스코프로 지정하는 플래그가 없다.
- 파일명은 bare `.env`/`.env.*`만 허용한다. 경로 구분자를 막아 `../`, 절대경로, `.bak` 타겟이 차단된다.
- `set` 키와 `copy --as` 이름은 `[A-Za-z_][A-Za-z0-9_]*` 형식만 허용해 개행이나 다른 대입문을 파일에 삽입할 수 없다.
- 타겟이 심볼릭링크거나, 하드링크를 가졌거나, 전역 store와 동일 파일이면 거부한다.
- 전역 store 디렉토리와 그 하위 디렉토리에서의 쓰기를 거부한다.
- git 레포 안에서는 시크릿을 쓰는 `copy` 타겟이 추적되지 않으면서 gitignore되어 있어야 한다. 아니면 하드 에러로 막는다(override 없음, `.gitignore` 수정 필요).
- 기존 파일에 `set`/`copy`로 쓸 때는 백업도 Git에서 제외되어 있어야 한다. `.env` 한 줄만 제외하는 대신 `.env*`로 백업까지 보호한다. Git 검사 실패 시 복사하지 않는다.

쓰기 전에는 `<file>.YYMMDD.bak` 백업을 만든다. 같은 날 두 번째 쓰기부터는 그날의 첫 백업을 유지한다(작업 시작 전 상태가 복구 지점이므로). 이후 `O_NOFOLLOW` 임시 파일에 쓰고 rename으로 교체한다. 백업도 `.env`로 시작하므로 `.env*` gitignore 한 줄로 함께 덮인다. `generate`의 예외적인 전역 추가는 위 절의 더 좁은 규칙을 따른다.

## 한계

마스킹은 심층 방어 수단이지 샌드박스가 아니다. 자식이 출력한 시크릿 원문은 잡지만, 자식이 재인코딩한 값(base64, URL 인코딩, 분할)은 잡지 못한다. `cat .env`나 Claude Code의 `@.env` 인라인 참조는 이 도구를 우회하며, 그쪽은 하네스 deny 규칙으로 막아야 한다. `doctor`가 `~/.claude/settings.json`의 `Read(**/.env)` 계열 deny 여부를 점검하므로, 두 레이어가 서로를 보완하도록 함께 설정하는 것을 권한다.

### 코딩 어시스턴트 지원

자동 감지는 공식 문서, 로컬 CLI 동작, 공개 소스에서 확인된 환경변수와 Unix 부모 프로세스의 CLI 이름을 사용한다. 부모 프로세스는 현재 명령에서 최대 3단계까지만 확인하며, 이름을 바꾸거나 더 깊게 감싼 실행은 명시 opt-in으로 처리한다.

| 도구 | 지원 결정 |
|---|---|
| xAI Grok CLI | 자동 감지: Unix 부모 프로세스의 `grok`. |
| Claude Code | 자동 감지: Claude 환경변수 또는 Unix 부모 프로세스의 `claude`. |
| OpenAI Codex CLI | 자동 감지: `CODEX_SANDBOX` 또는 Unix 부모 프로세스의 `codex`. sandbox 우회도 부모 감지로 처리. |
| OpenCode | 자동 감지: `OPENCODE` 또는 Unix 부모 프로세스의 `opencode`. |
| Google Antigravity CLI | 자동 감지: `ANTIGRAVITY_CONVERSATION_ID` 또는 Unix 부모 프로세스의 `agy`. |
| Google Gemini CLI | opt-in: 안정적인 자식 프로세스 마커 미확인. |
| Cursor CLI | opt-in: `cursor-agent` 실행 자체와 에이전트가 실행한 자식 명령을 구분하는 안정 마커 미확인. |
| GitHub Copilot CLI | opt-in: 공개 CLI 환경 설정에는 안정적인 자식 프로세스 마커 미확인. |
| Kiro CLI / Amazon Q CLI successor | opt-in: Amazon Q CLI는 Kiro CLI로 이전됐고, 안정적인 자식 프로세스 마커 미확인. |
| Aider | opt-in: 안정적인 자식 프로세스 마커 미확인. |
| Qwen Code | opt-in: 안정적인 자식 프로세스 마커 미확인. |
| Cline CLI | opt-in: 안정적인 자식 프로세스 마커 미확인. |
| Windsurf/Devin | opt-in: IDE/클라우드 에이전트 셸에 사용자가 env를 넣어야 함. |

확인되지 않은 도구에서는 에이전트를 띄우는 셸이나 래퍼에 아래처럼 넣는다.

```
export AGENTS_ENV_AGENT_MODE=1
```

한 번만 감싸 실행할 수도 있다.

```
AGENTS_ENV_AGENT_MODE=1 cursor-agent
AGENTS_ENV_AGENT_MODE=1 qwen
AGENTS_ENV_AGENT_MODE=1 copilot
AGENTS_ENV_AGENT_MODE=1 gemini
AGENTS_ENV_AGENT_MODE=1 kiro
AGENTS_ENV_AGENT_MODE=1 aider
AGENTS_ENV_AGENT_MODE=1 cline
```

직접 관리하는 하네스가 고유 마커를 넣을 수 있으면 config에 등록한다.

```
agents-env config set markers MY_AGENT_MODE
MY_AGENT_MODE=1 agents-env get TAVILY
```

- **감지는 신호이지 차단막이 아니다.** 모드는 env 마커와 가까운 부모 CLI 이름으로 판단하므로, 에이전트가 마커를 제거하고 CLI 이름도 숨기면 사람 모드가 될 수 있다. 대상 위협은 실수로 시크릿을 로깅하면 안 되는 정직한 에이전트다. 악의적 에이전트가 store를 직접 읽는 것은 이 도구가 아니라 deny 규칙이 막을 영역이다.
- **`{{KEY}}`는 같은 사용자의 `ps`에 노출된다.** 값이 자식 argv에 들어가 같은 사용자의 다른 프로세스가 읽을 수 있다. 공유 머신에서 민감한 값은 `{{KEY}}` 대신 env 주입을 사용한다.
- **동시 편집의 경계.** `generate`와 `edit`는 같은 잠금을 사용한다. 도구 밖 편집기는 잠금을 따르지 않으므로 생성 중 직접 편집하지 않는다. 생성은 열린 디렉토리 fd(`openat`/`renameat`)를 사용하고 교체 전 파일 변경을 재확인하지만, 같은 사용자 권한의 악의적인 파일 교체까지 막는 샌드박스는 아니다. 기존 `set`/`copy`에는 부모 디렉토리 교체 경쟁의 한계가 있다.
- **줄 끝 보존.** `generate`는 기존 바이트를 정규화하지 않는다. 기존 `set`/`copy` 라운드트립은 주석·순서·간격을 보존하지만 CRLF를 LF로 쓰고 끝에 개행을 추가한다.

## 설치

설치와 설정은 함께 배포되는 에이전트 스킬에 포함되어 있어, Claude Code나 Codex에 맡길 수 있다. 수동 설치는 다음과 같다.

```
cargo install agents-env
# crates.io 등록 전이면:
cargo install --git https://github.com/ai-native-engineer/agents-env
```

소스에서 설치할 때는 `cargo install --path . --locked`를 사용할 수 있다.
Cargo 설치 위치의 `bin` 디렉터리를 PATH에 넣는다. 사용자별 `.env` 경로를
배포 파일에 넣지 않고 `config set global_store`로 지정한다.

검증 명령은 `cargo test --locked`, `cargo clippy --locked --all-targets -- -D warnings`,
`cargo fmt --all -- --check`다. CI는 macOS/Linux에서 stable과 Rust 1.88을 검사한다.

## 라이선스

MIT
