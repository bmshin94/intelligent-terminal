# Intelligent Terminal 전수조사 분석 정리 📋

> 카리나가 오빠를 위해 정리한 **Intelligent Terminal 완전 분석 노트** ✨💖
>
> - **작성일**: 2026-10-01
> - **분석 대상 커밋**: `cc9ead6` (베이스 업스트림 `9dff1e9`)
> - **분석 브랜치**: `claude/kind-davinci-g187q2`

---

## 🔗 GitHub 주소 모음

| 구분 | 주소 |
|---|---|
| 🏠 **내 저장소 (이 레포)** | https://github.com/bmshin94/intelligent-terminal |
| 🌿 작업 브랜치 | https://github.com/bmshin94/intelligent-terminal/tree/claude/kind-davinci-g187q2 |
| ⬆️ **업스트림 (Microsoft 원본)** | https://github.com/microsoft/intelligent-terminal |
| 📥 릴리스 다운로드 | https://github.com/microsoft/intelligent-terminal/releases/latest |
| 🐛 이슈 트래커 | https://github.com/microsoft/intelligent-terminal/issues |
| 🪟 원조 Windows Terminal | https://github.com/microsoft/terminal |
| 🛒 Microsoft Store | https://apps.microsoft.com/detail/9NMQC2SSJX24 |
| 📰 공식 발표 블로그 | https://devblogs.microsoft.com/commandline/announcing-intelligent-terminal-version-0-1/ |
| 🔌 ACP 표준 문서 | https://agentclientprotocol.com/get-started/agents |
| 🤖 GitHub Copilot CLI | https://github.com/features/copilot/cli/ |

---

## 1️⃣ 이게 뭐하는 거야?

### 한 줄 정의

> **Intelligent Terminal** = Microsoft가 만든 **Windows Terminal의 공식 실험 포크(fork)**.
> 터미널 안에 AI 에이전트(Copilot / Claude / Codex / Gemini / OpenCode)를 **네이티브로 통합**한 버전.

README 원문:
> *"Intelligent Terminal is an experimental fork of Windows Terminal with native agent integration."*

### 저장소 규모 (직접 측정)

| 항목 | 수치 |
|---|---|
| 전체 파일 | **4,243개** |
| 용량 | **111 MB** |
| C++ (.cpp) | 590개 |
| C++ 헤더 (.h / .hpp) | 628개 |
| **Rust (.rs)** | **150개 / 약 152,700 LOC** ⭐ |
| C# (.cs) | 98개 |
| WinRT IDL | 121개 |
| XAML (UI) | 52개 |
| 문서 (.md) | **212개** |
| PowerShell | 183개 |

👉 **거대한 C++ 네이티브 앱 + 그 안의 Rust AI 엔진** 조합.

---

## 2️⃣ 아키텍처

### 공식 구조도 (`AGENTS.md`)

```
WindowsTerminal.exe  (C++ / XAML — 눈에 보이는 터미널)
  ├── TerminalProtocolComServer   ← COM 서버 (WT_COM_CLSID로 발견)
  ├── SharedWta → wta-master      ← AI 에이전트 프로세스 풀 관리
  └── 탭마다 wta-helper 1개        ← 실제 AI 채팅 패널
            ├── helper ↔ master : 네임드 파이프 통신
            └── 세션 전용 MCP 툴

에이전트 또는 사람 → wta/wtcli → COM IProtocolServer → Windows Terminal 제어
```

### 레이어별 역할

#### ① C++ 레이어 (`src/cascadia/`) — UI / 껍데기
| 파일 | 역할 |
|---|---|
| `TerminalApp/TerminalPage.cpp` | 터미널 본체 |
| `TerminalApp/TerminalPage.Protocol.cpp` | 프로토콜 브리지 |
| `TerminalApp/AgentPaneContent.cpp/.xaml` | AI 채팅 패널 UI |
| `TerminalApp/AgentPaneLifetime.cpp` | 패널 생명주기 |
| `TerminalApp/TabManagement.cpp` | 탭 생명주기 + 사전예열(pre-warm) |
| `TerminalApp/Tab.cpp` | stash / restore |
| `TerminalApp/SharedWta.cpp` | 공유 WTA 프로세스 |
| `TerminalApp/FreOverlay.cpp/.xaml` | 첫 실행 마법사(FRE) |
| `WindowsTerminal/TerminalProtocolComServer.cpp` | COM 서버 |
| `TerminalProtocol/TerminalProtocol.idl` | 프로토콜 IDL |
| `inc/AgentRegistry.h` | 에이전트 레지스트리 |
| `TerminalSettingsModel/MTSMSettings.h` | 설정 모델 (권위 있는 출처) |
| `TerminalSettingsEditor/AIAgents.xaml` | 설정 화면의 AI Agents 페이지 |

> ⚠️ **중요**: C++은 ACP를 전혀 모른다. AI 통신은 전부 Rust(WTA)가 담당.
> 에이전트 패널은 그냥 `wta-helper`를 띄운 **평범한 ConptyConnection 패널**일 뿐.

#### ② Rust 레이어 (`tools/wta/`) — AI 엔진 ⭐ 핵심
- `wta` = **W**indows **T**erminal **A**gent
- `src/master/mod.rs` : 에이전트 CLI 프로세스 **풀(pool)** 관리
  - **(에이전트 신원 + 실행 소스 + 커맨드)를 키로 묶어** 같은 키면 프로세스 1개 공유, 세션만 멀티플렉싱
- `src/helper/mod.rs` : 탭별 TUI 채팅 화면 (ratatui + crossterm)
- `src/agent_tools/session_mcp.rs` : **MCP 서버** ⭐
- `src/agent_tools/action_proposal/` : 승인 게이트 (channel / invocation / pipe / pipe_security / schema)
- `src/app/autofix.rs` : 에러 자동 감지 & 수정 제안
- `src/cli/` : tmux 스타일 CLI
- `src/agent_registry.rs`, `src/agent_sessions.rs`, `src/session_registry.rs` 등

**주요 의존성 (`tools/wta/Cargo.toml`)**
```toml
agent-client-protocol = "1.3.0"   # ⭐ ACP 공식 크레이트
tokio = { version = "1", features = ["full"] }
ratatui = "0.30"            # TUI
crossterm = "0.29"
windows-sys = "0.61"        # Win32 API
tracing / tracing-subscriber / tracing-appender
tracelogging = "1"          # MS 공식 ETW
strsim = "0.11"             # 오타 교정 (gti → git)
rust-i18n = "3"             # 다국어
image = "0.25"              # 클립보드 이미지 (Alt+V)
```

#### ③ ACP — 벤더 중립 표준 콘센트 🔌
```
      ┌─ Copilot  (GitHub)   ← 기본
      ├─ Claude   (Anthropic) ← npm 어댑터 @agentclientprotocol/claude-agent-acp
ACP ──┼─ Codex    (OpenAI)    ← npm 어댑터 @agentclientprotocol/codex-acp
🔌    ├─ Gemini   (Google)
      └─ OpenCode (오픈소스)
```
- **stdio 기반** → 특정 회사 AI에 종속되지 않음
- `src/cascadia/inc/AgentRegistry.h`에 ACP 에이전트 5종 + 위임(delegate) 에이전트 5종 정의
- 커스텀 에이전트는 `custom:<name>` ID 사용

---

## 3️⃣ 주요 기능 5개

### ① Agent Pane — `Ctrl+Shift+.`
- 터미널 하단(기본)에 도킹되는 AI 채팅 패널
- 🔥 **내 쉘 출력을 AI가 이미 보고 있음 → 복붙 0번!** (PowerShell / Bash / WSL 전부)
- 복잡한 작업은 **새 탭에서 백그라운드**로 돌려서 내 작업 탭 보호
- `Alt+V` 로 클립보드 **이미지** 붙여넣기
- 마우스 선택, 더블클릭(단어), 트리플클릭(줄), `Ctrl+C` 복사
- 입력창에서 `↑`/`↓` 로 이전 프롬프트 재호출
- 토글은 **stash/restore** 방식 → helper, ACP 세션, 채팅 히스토리 안 날아감

**슬래시 명령어**

| 명령 | 설명 |
|---|---|
| `/agent [id]` | 이 탭의 에이전트 소스 선택 (WSL 패널이면 Windows + 해당 distro 에이전트만) |
| `/clear` | 스크롤백 지우기 (세션은 유지) |
| `/config` | 모드(agent/plan/autopilot), 모델, reasoning effort, 승인 여부 설정 |
| `/fix [힌트]` | 현재 터미널 진단 + 수정 제안 |
| `/help` | 명령 목록 |
| `/model [id]` | 이 패널의 모델 변경 (BYOK 모델 picker) |
| `/move l\|r\|u\|d` | 이 탭의 패널 위치만 이동 |
| `/new` | 새 세션 시작 (히스토리 버림) |
| `/restart` | 클린 세션으로 재시작 |
| `/sessions` | 에이전트 관리 열기 |
| `/stop` | 진행 중인 프롬프트 취소 |

### ② Error Detection / Autofix — `Ctrl+Alt+.`
- 명령 실패하면 상태바 아이콘 점등
- 누르면 **에러 컨텍스트가 이미 로딩된** AI 패널 열림
- `strsim`(Damerau-Levenshtein)으로 `gti status` → `git status` 오타 교정
- ⚠️ 연결된 helper 세션이 있어야 동작. 세션 연결 전 실패는 **나중에 재생되지 않음**

### ③ Command Palette 위임 — `?프롬프트` / `Alt+Shift+/`
- 현재 패널 컨텍스트를 자동 주입하고 **새 백그라운드 탭**에서 에이전트 시작
- `&<프롬프트>` 는 예약됐지만 현재 no-op

### ④ Agent Management — `Ctrl+Shift+/`
- 활성 에이전트, 상태, 과거 세션 목록 전부 조회
- 끊긴 워크플로 이어받기 / 장시간 작업 확인

### ⑤ Agent Status Bar
- 왼쪽: 패널 토글, 에러 감지 아이콘
- 오른쪽: 에이전트 관리 아이콘
- **토큰 사용량 + 비용 실시간 표시** (설정에서 on/off)

---

## 4️⃣ `wta` CLI — AI에게 "눈과 손"을 달아주는 도구 🦾

```bash
wta list-windows                        # WT 창 전부 (alias: lsw)
wta list-tabs                           # 탭 나열 (lst)
wta list-panes                          # 패널 나열 (lsp)
wta active-pane --json                  # 지금 포커스된 패널
wta new-tab -c "pwsh.exe" -n "Build"    # pwsh 돌리는 새 탭 (neww)
wta split-pane -H -c "pwsh.exe"         # 가로 분할 (splitw)
wta capture-pane -t 3 -l 50             # 3번 패널 최근 50줄 읽기 ⭐
wta kill-pane -t 3                      # 패널 닫기 (killp)
wta pane-status -t 3                    # 실행 중인지 확인
wta wait-for -t 3 --timeout 30          # 종료까지 대기
wta resolve-command git --cwd . --json  # 명령 존재/경로 확인
wta pipe-id --json                      # WT_COM_CLSID 확인
wta set-env -s powershell               # 다른 쉘로 환경변수 재전파
```

- **모든 명령 `--json` 지원** → AI가 shell out 해서 터미널을 다룰 수 있음
- `-t` 생략하면 **활성 패널 자동 사용**
- `WT_COM_CLSID` 환경변수로 Windows Terminal을 찾음 (WT가 모든 conpty 자식에 자동 전파)
- `tools/wta/SKILLS.md` 는 **"이 문서는 AI 에이전트용"** 이라고 명시된 치트시트
- ⚠️ 키 입력 주입(`send-keys`) CLI 동사는 제거됨 → 캡빌리티 파이프 경유

### 비유로 보는 매핑
| AI가 원하는 것 | 명령 | 비유 |
|---|---|---|
| 지금 화면 보기 | `wta capture-pane` | 👀 눈 |
| 구조 파악 | `wta list-tabs` | 🧭 지도 |
| 탭 만들기 | `wta new-tab` | 🖐 손 |
| 끝날 때까지 대기 | `wta wait-for` | ⏳ 인내 |
| 명령 있나 확인 | `wta resolve-command` | 🔍 확인 |

---

## 5️⃣ Session MCP — 승인 게이트 🚪

`tools/wta/src/agent_tools/session_mcp.rs`

| MCP 툴 | 기능 |
|---|---|
| `run_command_in_current_shell` | 현재 쉘에서 명령 실행 (승인 필요) |
| `create_workspace` | 새 작업공간(탭) 생성 |
| `delegate_task_in_new_workspace` | 새 작업공간에 작업 위임 |
| `request_user_input` | 사용자에게 되묻기 |

**설계 원칙 (AGENTS.md 원문)**
> *"It routes requests to the owning helper and never executes terminal actions itself."*
> MCP는 요청을 소유 helper에게 **라우팅만** 하고, 절대 직접 실행하지 않는다.

> *"Terminal mutation requested by an agent goes through the confirmation-gated session MCP action path."*
> 에이전트의 터미널 변경 요청은 반드시 **확인 게이트**를 통과한다.

**흐름**
```
🤖 AI: "이 명령 실행하고 싶어요"
   ↓
🚪 MCP: "사용자 승인 먼저"
   ↓
👩‍💻 사용자: [실행] / [복사만] / [취소]  ← 결정권은 사용자
   ↓
   승인시 → 실행 ✅
```

README 원문:
> *"The agent pane does not run commands in your shell without your explicit approval"*

---

## 6️⃣ 설정 (`AGENTS.md` 기준)

```jsonc
{
    "acpAgent": "copilot",
    "acpModel": "",
    "acpCustomCommand": "",
    "delegateAgent": "copilot",
    "delegateModel": "",
    "delegateCustomCommand": "",
    "agentPanePosition": "bottom",
    "autoErrorDetectionEnabled": true,
    "autoFixEnabled": false,              // ⭐ 기본 꺼짐
    "aiIntegration.coordinator.enabled": false,
    "aiIntegration.coordinator.commandline": "wta",
    "aiIntegration.confirmation.readOperations": "auto",
    "aiIntegration.confirmation.createOperations": "auto",
    "aiIntegration.confirmation.inputOperations": "auto"
}
```

- 권위 있는 출처: `src/cascadia/TerminalSettingsModel/MTSMSettings.h`, `src/cascadia/inc/AgentRegistry.h`
- **프로필별 에이전트 고정 가능** (PowerShell 프로필과 Ubuntu 프로필이 다른 에이전트 사용 가능)
- 승인 설정이 **읽기 / 생성 / 입력** 3단계로 분리된 설계

---

## 7️⃣ BYOK (Bring Your Own Key)

Copilot / OpenCode 경유, **OpenAI 호환 Chat Completions 엔드포인트** 지원.

```text
Base URL:  http://localhost:11434/v1
Model ID:  <로컬 모델명>
API key:   <생략 가능>
```

- 🎉 **Ollama 로컬 모델 완전 지원** (키 없이 동작, 인터넷 안 나감)
- 🔒 **API 키는 Windows Credential Manager에 저장** (settings.json 평문 아님)
- `/model` picker에 `modelId (BYOK)` 로 표시

---

## 8️⃣ 프라이버시 구조 — "로컬 전송 계층"

README 원문:
> *"Intelligent Terminal is a **local transport layer**. ... Intelligent Terminal does not call any cloud APIs itself and does not persist conversation history"*

```
         ❌ (이런 구조 아님)
내 PC → MS 서버 → AI 서버

         ✅ (실제 구조)
내 PC ─[stdio/ACP 배달만]→ 내가 고른 에이전트 CLI → 그 벤더 서버
```

**터미널을 통과하는 데이터**
- 내가 입력한 프롬프트
- 쉘 출력 컨텍스트
- 기본 환경 메타데이터 (쉘 종류, OS 버전)

→ 전부 **활성 세션 메모리에만** 있고 세션 종료 시 폐기.

| 고른 에이전트 | 데이터 목적지 |
|---|---|
| GitHub Copilot | GitHub 백엔드 (Enterprise는 ZDR 가능) |
| 서드파티 / 커스텀 CLI | 그 벤더가 결정 (MS/GitHub 계약 적용 안 됨) |
| **Ollama (로컬)** | 🏠 **내 PC 안에서만** |

> ⚠️ 단, 진단 로그는 디스크에 쓰이고 텔레메트리는 전송될 수 있음 (`PRIVACY.md`에서 끄는 방법 안내)

---

## 9️⃣ 보안 — MS가 솔직히 적어둔 리스크 🔒

출처: `doc/security-model.md` (**Draft v1.4, Microsoft 내부 보안 리뷰용**)

| 리스크 | 내용 | 현재 상태 |
|---|---|---|
| 🔓 **SendInput over COM** | `WT_COM_CLSID`를 아는 호출자면 아무 패널에나 키 입력 주입 가능 | 2026-05-21에 보호장치(상속 파이프 게이팅) **되돌림**. 의도적 공격면 확대 |
| 🔓 **Create/Split over COM** | `CreateTab`/`SplitPane` 로 임의 명령 실행 가능 | 노출됨 |
| 👁 **이벤트 브로드캐스트 노출** | legacy `agent_event` 가 모든 COM 구독자에게 방송 → 다른 패널의 프롬프트/툴 호출 관찰 가능 | 구독자별 필터링 없음 |
| 💉 **Prompt Injection** | COM 접근 권한이 LLM 요청의 안전성을 보장하지 않음 | 승인 설정은 있으나 기본값 `auto`이고 **런타임 경로에서 강제되지 않음** |
| 📤 **Autofix 컨텍스트 유출** | 조작된 OSC 133 실패 마크로 패널 내용이 승인 전에 LLM에 전송될 수 있음 | `autoFixEnabled` 기본 `false` (opt-in) |
| 📤 **위임 컨텍스트 유출** | `?<프롬프트>` 가 활성 패널 컨텍스트를 위임 CLI에 전달 | 컨텍스트 전용 확인/마스킹 없음 |
| 📝 **스크롤백/로그 노출** | 패널 출력과 로그에 비밀키·소스코드가 남을 수 있음 | ⚠️ **Redaction 미구현** |
| ⚙️ **settings.json 영속 변조** | 사용자 권한 프로세스가 설정 파일을 덮어써 에이전트/Autofix 설정 변경 가능 | 메타 확인 없음 |

### 💡 실전 권고
- 🟢 **개인 공부 / 사이드 프로젝트** → 마음껏 사용
- 🔴 **회사 기밀 코드 PC** → **Autofix 끄기 + 로컬 Ollama 사용 + 로그 주의**
- ⚠️ **v0.1 실험 단계** → 메인 작업 환경 전면 교체는 아직 위험

---

## 🔟 설치 및 사용법

### 전제조건
| 필수 | 내용 |
|---|---|
| OS | **Windows 10 2004 (빌드 19041) 이상** |
| ❌ 불가 | **macOS / Linux 아예 안 됨** (COM + WinRT + XAML + DirectX) |
| 필수 | 에이전트 CLI + 구독 (Copilot 등) |

### 설치 3가지
```powershell
# 방법 1 — Microsoft Store (추천, 자동 업데이트)
#   https://apps.microsoft.com/detail/9NMQC2SSJX24

# 방법 2 — winget
winget install --id Microsoft.IntelligentTerminal -e

# 방법 3 — 직접 다운로드
#   https://github.com/microsoft/intelligent-terminal/releases/latest
```

> 💡 기존 Windows Terminal을 **덮어쓰지 않고 별도 앱으로 나란히** 설치됨.

### 첫 실행 (FRE)
1. 에이전트 선택 화면 → 내 PC의 CLI 자동 탐지 (Copilot/Claude/Codex/Gemini/OpenCode)
2. 하나도 없으면 **GitHub Copilot CLI를 winget으로 자동 설치**
3. 미인증이면 패널이 로그인 안내 (Copilot Enterprise는 `E` 키 → `your-org.ghe.com`)
4. 바로 질문 시작

**FRE가 자동 설치**: Node.js(필요시), 쉘 통합(PowerShell / bash / WSL bash), 에이전트 훅
**직접 설치(BYO)**: Claude Code, Codex, Gemini, OpenCode

### 자주 만나는 에러
PowerShell에서 `running scripts is disabled on this system` / `UnauthorizedAccess`:
```powershell
Set-ExecutionPolicy -Scope CurrentUser -ExecutionPolicy RemoteSigned
```
그 외는 `doc/installing-dependencies.md` 참고.

### 단축키
| 단축키 | 기능 |
|---|---|
| `Ctrl+Shift+.` | AI 패널 열기/닫기 ⭐ |
| `Ctrl+Shift+I` | 터미널 ↔ AI 패널 포커스 전환 |
| `Ctrl+Alt+.` | **에러 컨텍스트 들고** AI 패널 열기 🔥 |
| `Ctrl+Shift+/` | 에이전트 관리 |
| `Alt+Shift+/` | 명령 팔레트 프롬프트 모드 |
| `Alt+Shift+B` | 위임 에이전트 탭 (빈 상태) |
| `Alt+V` | 클립보드 이미지 붙여넣기 |

### 소스 빌드 (개발자)
출처: `doc/quick-start-local-dev.md`

**설치할 것**
- **Visual Studio 2026 (18.x)** + `Desktop development with C++` + `Universal Windows Platform development`
- **Rust** (rustup)
- `OpenConsole.slnx` 열고 "extra components" → **Install** (`.vsconfig` 읽어서 자동 추가. 특히 **C++ UWP tools (Latest MSVC)** 필수)

**빌드 — 순서 중요! 빌드 시스템이 2개**
```powershell
# ① Rust 먼저
cargo build --target x86_64-pc-windows-msvc --manifest-path tools/wta/Cargo.toml

# ② Visual Studio
#    시작 프로젝트: CascadiaPackage / 플랫폼: x64
#    Properties > Debug: Application process + Background task process → Native Only
#    F5
```

**코드 수정 후**
| 수정 대상 | 할 일 |
|---|---|
| Rust (`tools/wta/`) | `cargo build` (증분, 수초) → **F5** (새 wta.exe 복사) |
| C++ (`src/`) | **F5** |

**테스트**
```powershell
cargo test --manifest-path tools/wta/Cargo.toml   # Rust
runut.cmd / runft.cmd / runuia.cmd                 # C++ (TAEF)
```
> `wta.exe in use` → `taskkill /f /im wta.exe`
> 첫 빌드는 매우 오래 걸림 (C++ 1,200+ 파일 + Rust 15만줄)

**명령줄 빌드 (razzle)**
```cmd
cmd.exe /c "tools\razzle.cmd && bcz no_clean"
```

---

## 1️⃣1️⃣ 플러그인? 스킬? MCP? → **넷 다!**

| 분류 | 맞나? | 실체 |
|---|---|---|
| 🖥 **독립 앱** | ✅✅✅ | `.msix` 설치 앱. **본질 (전체의 95%)** |
| 🔌 **MCP** | ✅ | 내장 session MCP 서버 (툴 4개). **터미널 → AI 방향** |
| 🧩 **플러그인** | ✅ | `tools/wta/wt-agent-hooks/` — 5개 CLI용 훅 번들 |
| 📜 **스킬** | ⚠️ | `SKILLS.md`는 이름만 스킬, 실제론 CLI 레퍼런스 문서 |
| ⌨️ **CLI 도구** | ✅ | `wta.exe` + `wtcli.exe` (tmux 스타일) |

> ⚠️ **"Claude Code에 설치하는 플러그인"이 아님!** 반대로 **Claude Code가 이 앱 안에서 돌아감.**

### `wt-agent-hooks` — 진짜 플러그인 (바로 참고 가능!)

```
tools/wta/wt-agent-hooks/
├── claude/                        → claude plugin marketplace add
│   ├── .claude-plugin/marketplace.json
│   └── wt-agent-hooks/
│       ├── .claude-plugin/plugin.json
│       └── hooks/hooks.json
├── copilot/                       → copilot plugin marketplace add
│   ├── .github/plugin/marketplace.json
│   └── wt-agent-hooks/{plugin.json, hooks/hooks.json}
├── codex/                         → codex plugin marketplace add
├── gemini-extension/              → gemini extensions install
│   └── {gemini-extension.json, hooks/hooks.json}
├── opencode/                      → OpenCode 전역 플러그인 디렉토리
│   └── {plugin.json, wt-agent-hooks.js}
└── hook-debug/state-logger.ps1    (개발용, 번들 제외)
```

**Claude 쪽 `hooks.json` 실제 내용** — 전부 `wtcli agent-hook` 으로 디스패치:

| Claude Code 훅 | 전달 이벤트 |
|---|---|
| `SessionStart` | `agent.session.start` |
| `SessionEnd` | `agent.session.end` |
| `Notification` | `agent.notification` |
| `UserPromptSubmit` | `agent.prompt.submit` |
| `StopFailure` | `agent.error` |
| `Stop` | `agent.stop` |

```bash
command -v wtcli.exe >/dev/null 2>&1 && \
  wtcli.exe agent-hook --cli-source claude --event agent.session.start; exit 0
```

👉 다른 패널에서 Claude Code를 따로 돌려도 세션 생명주기가 터미널로 흘러들어와 **Agent Management에서 실시간 상태 표시**.
👉 **Claude Code 플러그인 작성 실전 예제**로 그대로 활용 가능! 🎁

---

## 1️⃣2️⃣ API 토큰 필요한가?

> Intelligent Terminal **자체는 토큰 불필요** (클라우드 API를 호출하지 않음). 단, AI는 로그인/구독 필요.

| 방식 | API 키? | 비용 |
|---|---|---|
| ① **GitHub Copilot** (기본) | ❌ OAuth 로그인 | Copilot 구독 |
| ② **Claude Code** | ❌ `claude` 로그인 | Claude Pro/Max 구독 |
| ③ Codex / Gemini / OpenCode | ❌ 각 CLI 로그인 | 각 서비스 구독 |
| ④ **BYOK 커스텀** | ✅ **필요** | 토큰 종량제 |
| ⑤ **로컬 Ollama** | ❌ 불필요 | 🎉 **완전 무료** |

- 🔒 API 키는 **Windows Credential Manager**에 저장 (settings.json 평문 아님)
- 📊 상태바에 **토큰 수 + 비용 실시간 표시** → 종량제 비용 관리 가능

---

## 1️⃣3️⃣ AI 에이전트 구축에 도움될까? → **교과서 수준! 💯**

### 배울 수 있는 7가지

| # | 주제 | 소스 |
|---|---|---|
| 1 | **ACP 실전 구현** (벤더 중립 추상화, npm 어댑터 패턴) | `agent_registry.rs`, `doc/specs/acp-1.0-conductor-migration.md`, `acp-v1-support-completion-plan.md`, `tools/wta/acp-multi-session-concurrency.md` |
| 2 | **Master/Helper 프로세스 풀 + 세션 멀티플렉싱** | `src/master/mod.rs`, `src/helper/mod.rs` |
| 3 | **승인 게이트 / Human-in-the-loop** ⭐ | `src/agent_tools/action_proposal/` (channel, invocation, pipe, pipe_security, schema) |
| 4 | **MCP 서버 구현 + 라우팅 분리 원칙** | `src/agent_tools/session_mcp.rs` |
| 5 | **플러그인/훅 시스템** (5개 CLI 매니페스트 비교) | `tools/wta/wt-agent-hooks/` |
| 6 | **아키텍처 설계 문서 40개** | `doc/specs/` |
| 7 | **프로덕션 Rust 비동기 15만줄** | `tools/wta/` 전체 |

### `doc/specs/` 보물 목록
| 문서 | 배울 것 |
|---|---|
| `Multi-window-agent-pane.md` | 멀티윈도우 helper/master 생명주기 (권위 스펙) |
| `agent-oobe-design.md` | 에이전트 온보딩 UX |
| `agent-failure-handling.md` | AI 실패 복구 전략 ⭐ |
| `connection-resilience.md` | 연결 끊김 대응 |
| `llm-agent-event-integration.md` | LLM 이벤트 통합 |
| `hybrid-agent-session-tracking.md` | 세션 추적 하이브리드 |
| `byok-agent-support.md` | BYOK 구현 |
| `turn-state-refactor.md` | 멀티턴 상태 관리 |
| `wta-architecture-refactor.md` | 리팩토링 의사결정 기록 |
| `WTA-terminal-action-proposals.md` | 액션 제안 프로토콜 |
| `Yolo-mode.md` | 자동승인 모드 설계 |
| `wsl-session-management.md` | WSL 세션 관리 |
| `../security-model.md` | 🔒 위협 모델링 템플릿 |

### 한계
| 한계 | 설명 |
|---|---|
| 🪟 Windows 종속 | COM + WinRT + XAML + DirectX → 포팅 불가 |
| 🦀 Rust 장벽 | 핵심이 Rust |
| 🏔 덩치 | 4,243 파일 / 111MB → 전부 읽기 비현실적 |

### 💡 추천 공략 순서
```
AGENTS.md
  → tools/wta/OVERVIEW.md
    → doc/specs/Multi-window-agent-pane.md
      → src/agent_tools/session_mcp.rs
        → src/agent_tools/action_proposal/
          → doc/security-model.md
```
> C++(`src/`)은 과감히 무시하고 **`tools/wta/` + `doc/specs/` 2개만** 파면 충분!

---

## 1️⃣4️⃣ React / PHP로 만들 수 있어?

### ❌ 이 프로젝트 자체 = 불가능
| 기술 요소 | React/PHP |
|---|---|
| Win32 ConPTY | ❌ 네이티브 전용 |
| COM `IProtocolServer` | ❌ Windows 전용 |
| WinRT / XAML | ❌ |
| DirectX 11 렌더러 (`AtlasEngine`, HLSL) | ❌ |
| UIA 접근성 / Credential Manager | ❌ |

### ✅ 비슷한 것 = **Electron + React + xterm.js** 로 충분히 가능!
> 선례: **Warp**, **Hyper**(Vercel), **VS Code 내장 터미널**(= xterm.js)

```
┌──────────────────────────────────────┐
│  Electron (Chromium + Node.js)       │
│  ┌────────────────────────────────┐  │
│  │  React UI                      │  │
│  │  ├─ <Terminal/>  : xterm.js    │  │
│  │  ├─ <AgentPane/> : AI 채팅      │  │
│  │  └─ <StatusBar/> : 토큰/비용    │  │
│  └────────────────────────────────┘  │
│          ↕ IPC (ipcRenderer)         │
│  ┌────────────────────────────────┐  │
│  │  Node.js 메인 프로세스          │  │
│  │  ├─ node-pty   : 쉘 생성 🐚    │  │
│  │  ├─ ACP client : stdio 연결     │  │
│  │  └─ MCP server                 │  │
│  └────────────────────────────────┘  │
└──────────────────────────────────────┘
```

```bash
npm i electron react react-dom
npm i @xterm/xterm @xterm/addon-fit @xterm/addon-webgl
npm i node-pty
npm i @modelcontextprotocol/sdk
npm i @zed-industries/agent-client-protocol
```

🎉 **최대 장점: Windows + Mac + Linux 전부 지원** → 원본이 못 하는 걸 우리가 함!

| 기능 | 난이도 |
|---|---|
| 터미널 화면 (xterm.js + node-pty) | 🟢 쉬움 |
| AI 채팅 패널 UI | 🟢 쉬움 (React 특화) |
| 쉘 출력 → AI 컨텍스트 전달 | 🟡 보통 |
| 에러 자동 감지 (OSC 133) | 🟡 보통 |
| ACP 연결 (stdio + JSON-RPC) | 🟡 보통 |
| MCP 서버 + 승인 게이트 | 🟡 보통 |
| 프로세스 풀 공유 | 🔴 어려움 (1인 프로젝트면 생략 가능) |
| 성능 (GPU 렌더링) | 🔴 네이티브보다 느림 (WebGL addon으로 커버) |

> 🎊 **MVP는 주말 2~3일 가능!** (xterm.js 터미널 + Claude 채팅 + 에러 자동 전달)

### 🐘 PHP는?
직접 터미널 앱은 ❌. 하지만 **수익화 백엔드로는 완벽** ✅
| 용도 | 설명 |
|---|---|
| 💳 결제/라이선스 서버 | Laravel + Stripe / 토스페이먼츠 |
| 🌐 랜딩페이지 & 문서 | 제품 소개, 다운로드 |
| 📊 팀 대시보드 | 토큰 사용량/비용 집계 (B2B 핵심) |
| 🔗 프록시 API 서버 | API 키 서버에 숨기고 중계 |
| 🧩 플러그인 마켓플레이스 | 커뮤니티 플러그인 배포 |

> 💡 현실적 조합: **Electron+React = 제품** / **PHP(Laravel) = 수익화 백엔드**

---

## 1️⃣5️⃣ 유튜브 강의 영상 제작 가능?

### ✅ 가능! 그리고 지금이 골든타임
1. 🆕 **신선함** — 한국어 콘텐츠 거의 없음 (블루오션)
2. 🏢 **권위** — "마이크로소프트가 만든" 썸네일 파워
3. 👀 **비주얼** — 에러 → 전구 → 자동수정. 녹화 쉬움
4. 🏞 **깊이** — 설치(입문) ~ 아키텍처(고수) 전 스펙트럼
5. ⚖️ **법적 안전** — **MIT 라이선스** (코드 인용 자유)

### 시리즈 기획 (3시즌 13편)

**🥇 시즌 1: 입문 (조회수)**
| # | 제목 | 길이 |
|---|---|---|
| 1 | 터미널 에러, 이제 복붙 안 합니다 (훅 영상) | 8분 |
| 2 | 3분 설치 + 첫 설정 완벽 가이드 | 10분 |
| 3 | Claude Code를 터미널에 넣기 | 12분 |
| 4 | 단축키 7개로 생산성 2배 | 7분 |

**🥈 시즌 2: 활용**
| # | 제목 |
|---|---|
| 5 | 💸 무료로 쓰기: Ollama + BYOK ⭐ 조회수 유망 |
| 6 | 🦾 `wta` CLI 완전정복 — AI에게 손 달아주기 |
| 7 | Agent Pane 슬래시 명령어 12개 전부 |
| 8 | 백그라운드 위임: `?` 하나로 일 시키기 |

**🥉 시즌 3: 심화 (차별화 💎)**
| # | 제목 |
|---|---|
| 9 | MS 소스코드 해부: ACP는 어떻게 동작하나 |
| 10 | MCP 서버 승인 게이트 설계 분석 |
| 11 | Rust로 만든 AI 오케스트레이터 읽기 |
| 12 | 😱 MS 보안문서가 인정한 8가지 위험 ⭐ 조회수 유망 |
| 13 | 우리도 만들자: Electron+React 클론 (시리즈물) |

### 촬영 팁
| 항목 | 추천 |
|---|---|
| 녹화 | OBS Studio + 1080p60 |
| 폰트 | **D2Coding / Cascadia Code, 16~18pt+** (모바일 가독성!) |
| 테마 | 다크 + 고대비 |
| 편집 | 긴 빌드/설치는 **배속 또는 컷** |
| 숏폼 | "에러 → 띠링 → 자동수정" 15초 클립 = 쇼츠/릴스 |

### 주의
| 리스크 | 대응 |
|---|---|
| 🪟 Windows 전용 | 제목/썸네일에 **"Windows"** 표기 |
| 🧪 v0.1 실험 단계 | "실험 버전" 고지, UI 변경 가능 |
| 🔄 업데이트 속도 | 설명란에 버전 명시, 고정 댓글 활용 |
| 💳 구독 필요 | 5편(Ollama 무료)을 먼저 올리는 전략도 |
| ⚖️ 상표권 | MS 로고는 Trademark Guidelines 준수. MS 후원처럼 보이면 안 됨 |

---

## 1️⃣6️⃣ 수익화 아이디어 💰

### 🏆 Tier 1 — 지금 당장, 리스크 0

#### #1 "ACP/MCP 에이전트 개발" 강의 & 전자책 ⭐⭐⭐
> 재료가 이미 손에 있고 투자금 0원. **MS 프로덕션 설계를 교재로** 쓸 수 있음.

**커리큘럼 (8주)**
| 주 | 내용 | 교재 소스 |
|---|---|---|
| 1 | AI 에이전트 아키텍처 개요 | `AGENTS.md`, `ARCHITECTURE.md` |
| 2 | ACP 완전정복 (벤더 중립) | `agent_registry.rs`, acp specs |
| 3 | MCP 서버 구현 | `session_mcp.rs` |
| 4 | 승인 게이트 & Human-in-the-loop 🔒 | `action_proposal/` |
| 5 | 프로세스 풀 & 세션 멀티플렉싱 | `master/`, `helper/` |
| 6 | 에러 복구 & 연결 복원력 | `agent-failure-handling.md` |
| 7 | 보안 위협 모델링 | `security-model.md` ⭐ |
| 8 | 실전: 나만의 에이전트 터미널 | Electron+React 실습 |

| 상품 | 가격 |
|---|---|
| 전자책 (PDF/위키독스) | 2~4만원 |
| 인프런/유데미 강의 | 8~15만원 |
| **기업 출강 (1일 워크숍)** | **200~500만원** 🔥 |
| 노션 분석노트 템플릿 | 1~2만원 |

#### #2 유튜브 + 콘텐츠 퍼널 ⭐⭐⭐
```
유튜브(무료 미끼) → 전자책(2~4만) → 강의(8~15만) → 기업 출강/컨설팅(200~500만) 💰
```
> 유튜브 광고수익은 덤. 핵심은 **전문가 신뢰 → 고단가 B2B 전환**.

### 🥈 Tier 2 — 제품 만들기

#### #3 크로스플랫폼 AI 터미널 ⭐⭐⭐ 🔥
> **Mac/Linux엔 이게 없다** → 시장이 비어있음

**MVP (주말 2~3일)**: xterm.js 터미널 + AI 채팅 + 쉘 출력 자동 전달 + 명령 제안 버튼

| 티어 | 가격 | 내용 |
|---|---|---|
| Free | 0원 | 기본 터미널 + BYOK |
| Pro | **$8~12/월** | 멀티 세션, 히스토리, 테마, 동기화 |
| Team | **$20/유저/월** | 토큰 대시보드, 공유 프롬프트, 정책 |
| Enterprise | 협의 | 온프레미스, SSO, 감사 로그 |

**틈새 포지셔닝 (정면승부 ❌)**
- 🇰🇷 한국어 완전 지원 AI 터미널 (IME / 한국어 응답)
- 🏠 로컬 LLM 전용 (Ollama 퍼스트, 완전 오프라인)
- 📚 학생/주니어 교육용 (친절한 에러 설명, 학습 모드)

#### #4 기업용 프라이빗 AI 터미널 SI/컨설팅 ⭐⭐
> 단가 최고. 금융/의료/공공/국방 타겟.

```
우리 클론 or Intelligent Terminal
  + Ollama / vLLM 사내 서버 (완전 오프라인 🏠)
  + 보안 커스터마이징
→ "소스코드가 회사 밖으로 한 바이트도 안 나가는 AI 개발환경"
```

🔑 **결정적 무기**: `doc/security-model.md` — MS가 직접 쓴 위협 분석.
"MS가 이렇게 분석했고 우리는 이 8개 리스크를 이렇게 막았습니다" → **보안팀 설득 완료** ✅

| 상품 | 가격 |
|---|---|
| 도입 컨설팅 (1~2주) | 500~1,500만원 |
| 구축 + 커스터마이징 | 2,000~8,000만원 |
| 연간 유지보수 | 구축비 10~20% |
| 개발자 교육 (반일) | 300~500만원 |

### 🥉 Tier 3 — 생태계 틈새

#### #5 유료 플러그인 / 훅 비즈니스 ⭐⭐
`wt-agent-hooks` 패턴 응용 → 5개 CLI 전부에 꽂히는 플러그인

| 플러그인 | 기능 | 가격 |
|---|---|---|
| 💰 Token Tracker Pro | 팀 토큰/비용 집계 + 예산 알림 | $5/월 |
| 📝 Session Recorder | AI 세션 → 문서 자동 변환 | $8/월 |
| 🔒 **Secret Guard** ⭐ | 전송 전 API 키/비번 자동 마스킹 | $10/월 |
| 📊 Team Dashboard | 팀 AI 활용도 분석 | $15/유저/월 |

> 💎 **Secret Guard가 가장 유망!** MS 보안문서에 **"Redaction is not implemented"** 라고 **명시**돼있음 → MS가 안 만든 명확한 빈칸 🎯

#### #6 템플릿 / 보일러플레이트 판매 ⭐⭐
- ACP 에이전트 스타터킷 (TS) — $29~79
- MCP 서버 보일러플레이트 (승인게이트 포함) — $39~99
- Electron+React AI 터미널 스타터 — $49~149
> 판매처: Gumroad / Lemon Squeezy / GitHub Sponsors / PHP 자체 결제
> 한 번 만들면 계속 팔리는 패시브 인컴 💸

#### #7 AI 터미널 워크플로 컨설팅 ⭐⭐
팀 환경 세팅 + 프롬프트 라이브러리 + 비용 최적화 + 2시간 교육
> 건당 100~400만원 / 월 리테이너 50~150만원 (확장성은 낮음)

### 📊 전체 비교
| # | 아이디어 | 투자 | 속도 | 규모 | 리스크 | 추천 |
|---|---|---|---|---|---|---|
| 1 | 🎓 강의/전자책 | 0원 | ⚡⚡⚡ | 💵💵 | 🟢 | ⭐⭐⭐ |
| 2 | 🎬 유튜브 퍼널 | 장비 | ⚡ | 💵💵💵 | 🟢 | ⭐⭐⭐ |
| 3 | 🖥 크로스플랫폼 제품 | 💰💰💰 | ⚡ | 💵💵💵💵 | 🟡 | ⭐⭐⭐ |
| 4 | 🔒 기업 SI | 💰💰 | ⚡ | 💵💵💵💵 | 🟡 | ⭐⭐ |
| 5 | 🧩 유료 플러그인 | 💰 | ⚡⚡ | 💵💵 | 🟢 | ⭐⭐ |
| 6 | 📦 템플릿 판매 | 💰 | ⚡⚡ | 💵 | 🟢 | ⭐⭐ |
| 7 | 🛠 컨설팅 | 0원 | ⚡⚡⚡ | 💵💵 | 🟢 | ⭐⭐ |

### 🎯 추천 실행 로드맵

**📅 Phase 1 (1~2개월) — 씨 뿌리기 🌱**
```
✅ 전자책 집필 (이 분석 확장) → 2~4만원 판매
✅ 유튜브 3편 (설치 / Ollama무료 / wta CLI)
✅ 블로그 연재 (벨로그/티스토리/브런치)
```
💰 월 30~100만원 + **전문가 포지션 확보** (이게 더 중요)

**📅 Phase 2 (2~5개월) — 신뢰 쌓기 🏗**
```
✅ Electron+React MVP (오픈소스 공개 → GitHub 스타 = 포트폴리오)
✅ 유료 강의 출시 (8~15만원)
✅ Secret Guard 플러그인 + Laravel 라이선스 서버 ⭐ PHP 활용
```
💰 월 200~500만원

**📅 Phase 3 (6개월~) — 수확 🌾**
```
✅ 제품 유료화 (Pro/Team) — Laravel 결제 서버
✅ 기업 출강 / 컨설팅 (강의 수강생 회사부터 영업)
✅ 보안 중시 기업에 프라이빗 AI 터미널 제안
```
💰 월 500~2,000만원+

### 💎 핵심 재료 3개
```
1. 🔒 doc/security-model.md  → "AI 에이전트 보안" 콘텐츠의 금광
                               (MS가 솔직히 쓴 위협분석)
2. 📚 doc/specs/ 40개         → "아키텍처 설계" 강의 교재
                               (유료강의 8주분이 그냥 들어있음)
3. 🧩 wt-agent-hooks          → 플러그인 비즈니스 설계도
                               (5개 CLI 매니페스트 비교표)
```

### 💡 "빈칸"을 노려라 — 여기에 돈이 있다
| MS가 **안 한** 것 | 👉 우리의 기회 |
|---|---|
| 🪟 Windows만 지원 | **Mac/Linux 버전** (#3) |
| 🔓 Redaction 미구현 | **Secret Guard 플러그인** (#5) |
| 🇺🇸 영어 중심 | **한국어 특화** 터미널/콘텐츠 |
| 📄 영어 문서 212개 | **한국어 분석 콘텐츠** (#1, #2) |

---

## 🎀 최종 요약 5줄

1. 🤖 **Intelligent Terminal** = MS가 만든 **AI 에이전트 내장 Windows Terminal 포크** (v0.1 실험, Windows 전용)
2. 🔌 **ACP 표준**으로 Copilot/Claude/Codex/Gemini/OpenCode를 **자유롭게 교체**, **Rust `wta`**(15만줄)가 엔진
3. 🦾 `wta` CLI가 AI에게 **눈과 손**을, **Session MCP 승인 게이트**가 **안전장치** 역할
4. 📚 가치는 "제품"보다 **"교재"** — ACP/MCP/승인게이트/프로세스풀 설계를 **프로덕션 코드로** 배울 수 있는 레퍼런스 1순위
5. 💰 수익화는 **Phase 1(강의·전자책·유튜브)이 가장 확실** / **Mac·Linux 버전**과 **Secret Guard**가 가장 큰 빈칸

---

<p align="center">
  <b>made with 💖 by 카리나 (Karina)</b><br>
  <i>오빠 화이팅! 이거 진짜 보물 받은 거야~ ✨🔥</i>
</p>
