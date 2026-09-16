# WTA -- Windows Terminal Agent

A Rust TUI client and tmux-like CLI that connects AI agents to Windows Terminal.

Customization:
- See [CUSTOMIZATION.md](CUSTOMIZATION.md) for changing the agent model and runtime prompt.

## Quick Start

### Build

From the repository root:

```bash
cargo build --target x86_64-pc-windows-msvc --manifest-path tools/wta/Cargo.toml
```

The binary is output to
`tools/wta/target/x86_64-pc-windows-msvc/debug/wta.exe`. Always use the explicit
target in this repo: the package project prefers that output over the host-target
fallback.

### How WTA runs

WTA is normally launched **by Windows Terminal**, not by hand. WT spawns one
`wta-master` singleton (owns a lazily populated agent CLI pool) and one
`wta-helper` per agent pane (renders this TUI and speaks ACP to master over a
named pipe). Helpers selecting the same agent identity, source, and command
share one agent process. Bare `wta` with no subcommand and neither `--master`
nor `--connect-master` exits with an error. The existing per-tab assistant does
not have a standalone mode; the separate experimental Agent Center uses `wta ui`.

The default agent is Copilot; the agent and model come from Windows Terminal
settings (`acpAgent` / `acpModel`) and are passed through to master via `--agent`
/ `--agent-id` / `--acp-model`.

When the agent pane is connected to Windows Terminal, the agent-facing contract is
the local `wta` CLI: the agent shells out to commands like `wta active-pane --json`,
`wta list-panes --json`, `wta capture-pane --json`, and
`wta resolve-command <name> --cwd <active-pane-cwd> --json`. Terminal-control commands talk to Windows
Terminal over the COM protocol; `resolve-command` inspects the user's real,
shell-context-selected sources (active working directory, host PATH and, for
PowerShell, the profile-loaded command environment).

Autofix sends the failing command's context without pre-querying similar command
names. Its prompt advertises `wta resolve-command` for agent-initiated diagnosis,
using the failing pane's shell and working directory. Command enumeration is
uncached and runs only when requested; there is no background refresh or
startup/tab-selection prewarming. Query failures or unsupported shell contexts
are not evidence that a command is missing. The prompt directs agents to propose
obvious typos in familiar commands (such as `gti status` -> `git status`) without
lookup, while using local evidence for unfamiliar commands or ambiguous corrections.

### Experimental Agent Center

Agent Center is an opt-in implementation of the
[work-mode product experiment](../../doc/specs/agent-center-product.md).
It does not replace the default per-tab assistant. Its authoritative message
contract is [Collaboration Protocol v1](../../doc/specs/agent-center-protocol.md);
the target specification is not a statement of production readiness.

Before first launch, configure a real ACP server. `adapters.json` contains
explicit executable/argument arrays rather than an inferred interactive CLI
command. Replace the sample path below with your server and its documented
stdio arguments. Terminal's built-in resolved commands are defined in
[`AcpModelUtils.h`](../../src/cascadia/inc/AcpModelUtils.h).

```json
{
  "capabilities": [
    {
      "id": "local-agent",
      "adapter": {
        "kind": "ACP",
        "approvedModelDestination": "Approved provider/account or endpoint",
        "executable": "C:\\Tools\\acp-server.exe",
        "args": []
      }
    }
  ]
}
```

```powershell
wta center configure --input-json .\adapters.json
```

This validates and installs configuration while the authority is stopped.
`approvedModelDestination` must name the destination you have approved for the
project's data. It records human approval, not verified endpoint enforcement or
network isolation. Optional `model` and `environment` fields belong inside
`adapter`; avoid putting credentials in this file.
Configuration is loaded on service startup and never
launches a model by itself. Each configured ID can coordinate, produce results,
and review; the separately registered `native-check` capability runs declared
local command checks. Without an adapter configuration, agent work is explicitly
unavailable. The ACP server must support HTTP MCP and the work-tool contract.

Project setup then approves a repository and finite planning/execution
allowances. For example, `project.json` can contain:

```json
{
  "name": "Local experiment",
  "root": "C:\\Source\\project",
  "coordinatorCapabilityId": "local-agent",
  "workerCapabilityId": "local-agent",
  "checkCapabilityId": "native-check",
  "limits": {
    "concurrency": 2,
    "executionAttempts": 8,
    "evaluationAttempts": 8,
    "coordinationTurns": 12,
    "contextRounds": 4,
    "executionSeconds": 600,
    "coordinationSeconds": 180
  }
}
```

```powershell
wta project configure --input-json .\project.json --confirm --json
```

Use the returned project ID with `/project use <id>`. Project planning approval
is distinct from work execution approval: `/work new "<goal>"` prepares an
intake/brief; `/work start` previews the exact grant and asks for confirmation.
Changing a work or accepting a result must name its current recorded versions.

`/work revise "<change>"` asks the coordinator for a proposal without applying
it. `/work apply <proposal-id>` previews the current grant and captures the
exact work/proposal versions for a final `Ctrl+Enter` confirmation. The CLI
equivalent is `wta work apply <proposal-id> --work <work-id> --confirm`.
Applying a revision preserves grant limits and consumed usage, revokes old
dispatch authority, and waits for affected execution to settle and release
before replanning. Old results and delivery candidates remain historical, not
current acceptance targets.

```powershell
wta ui
wta work list --json
wta work show <work-id> --json
wta task list --work <work-id> --json
wta result show <result-id> --json
wta work events <work-id> --after <cursor> --jsonl
```

`wta ui` and structured clients connect to the same local authority. The first
connection starts `wta center serve` in an independent process; this does not
itself approve a project or start model work. Closing a Console does not stop
the authority. If a launcher job disallows independent process creation, startup
fails explicitly: run `wta center serve` outside that job rather than tying
background work to the Console's lifetime.

State is stored in `agent-center\work.db` below the shared application-state
root, with immutable artifacts and managed workspaces alongside it. A private
current-user named pipe and an exclusive state-root lock protect the local
authority; SQLite uses WAL and durable commits. Diagnostics use the
`wta-center-service` and `wta-center-ui` log streams. This prototype does not
qualify provider-owned tools as an operating-system isolation boundary.

To try the native window host, launch a new Intelligent Terminal process with
`INTELLIGENT_TERMINAL_AGENT_CENTER=1` in its environment. It creates one direct
`wta ui` terminal surface, not a PowerShell/cmd process or a per-tab helper.
The initial shell area is empty. `Ctrl+Shift+G` focuses the Console;
`Ctrl+Shift+H` shows/hides existing shell content without closing its tabs.
The configurable `focusAgentConsole` action is also available. Explicit user
key bindings, including unbindings, take precedence over these defaults.

The experimental native layout does not replay or overwrite legacy saved
workspace layouts and disables tab dragging. Native policy configuration
currently blocks this independent entry rather than bypassing the agent
allowlist. Shell slash-command presentation adapters, external publication,
and production recovery/isolation qualification are not implied by enabling
the experiment. Unsupported commands report a capability error; they do not
claim that a shell, publication, or repair completed.

Structured output distinguishes `ok` (exit 0), `pending` (2), `needs_input`
(3), `conflict` (4), `unsupported` (5), and `error` (1). A recorded task result
is not an accepted result, and a successful provider turn is not delivery.
Acceptance requires the exact current candidate and its evidence. Preserve a
mutation's command ID and payload when retrying transport delivery; changing
the intention requires a new command ID.

Initial plans must preserve approved criterion descriptions and evidence rules,
not just criterion IDs. Delivery acceptance verifies captured manifests and
content again, including required check/review evidence; changing bytes at the
same locator does not retain a valid acceptance proof. Coordinators can read
recorded progress bodies with `progress.get` and verified, bounded diagnostic
content with `artifact.read`, without asking the user to copy logs.

Command checks support File-only reports as well as code snapshots. Evaluation
pins both the exact producing dispatch inputs and the submitted outputs. A fresh
check directory combines their captured member paths, with current output files
superseding prior input files; differing same-layer collisions fail explicitly.
File captures use their basename; capture a Tree when a nested layout matters.
A complete submitted Code/Tree/GitCommit snapshot remains authoritative, so older
inputs cannot resurrect deleted files. Checks never fall back to mutable
workspace contents. Independently pinned File inputs remain available as
supplemental check scripts or data when input snapshots are replaced.
Read-only LocalCode integration can retain a single
unambiguous accepted input code snapshot, with provenance and content rechecked
at final acceptance; report-only delivery remains supported independently.

Failures before a check starts carry their exact bounded diagnostic and
evaluation/result identities into coordinator snapshots and rework records,
rather than masquerading as a process test failure or a missing model submission.
The coordinator can revise captures under the unchanged contract, or explicitly
retry evaluation after repairing a transient cause. Unchanged repeated failures
remain bounded and visible; generic evidence collection is not an automatic fix.

Plan tools advertise the exact, case-sensitive output categories: `File`,
`Tree`, `GitCommit`, `Report`, `Code`, and `Evidence`. Admission and the tool
schema share that vocabulary. Unknown categories return `INVALID_ARGUMENT`
with an indexed `fieldErrors` path and the allowed values, rather than claiming
that plan creation is unsupported. Corrected proposals use a new command ID;
rejected proposals do not admit tasks or change the work version. Code, Tree
and GitCommit outputs still require a concrete required check. Approved scope,
exclusions and criterion/evidence constraints are not relaxed.

Approved prose evidence rules are copied verbatim into the integration task's
`criteria[].evidenceRule` and remain in its immutable dispatch. Separately,
`requiredEvidence` names concrete required gates or captured output slots.
This permits ordinary-language work briefs without treating prose as a gate ID
or silently replacing the approved meaning. Explicit `command:<gate>`,
`artifact:<slot>`, and legacy bare ASCII references retain their exact binding;
existing reference-based contracts do not need the new optional field.

Before a worker acknowledges its dispatch, ACP permission permits only an
exact bound `task_acknowledge` call, including its current revision and
continuation ID. Copilot's bare `task_acknowledge` title with kind `other` and
known server-qualified spellings are supported; a title alone never grants
permission. The tool must be in the invocation's bindings, with a valid command
ID and closed acknowledgement arguments. Permission selects only `allow_once`
and does not itself acknowledge the task: the successful MCP receipt does.
Unrelated pre-acknowledgement tools and permissions after cancellation,
release, or the end of a running turn remain denied. Host-policy denial is
logged explicitly, rather than treated as evidence of a human rejection.
Continuation turns reset text chunk numbering for their new message part.

Work MCP initialization negotiates a supported protocol version rather than
closing the connection when a client offers a newer version. It preserves known
versions and otherwise offers `2025-06-18`, matching the existing session MCP
negotiation. The client decides whether it supports that response. Diagnostics
record the offered/selected versions and invocation ID, not authorization
headers; malformed version fields receive a JSON-RPC invalid-params error.
Notification responses now use the same bounded writer and explicit TCP send
shutdown as JSON responses. A real Windows queued-input close/reset regression
is covered; the exact trigger of the earlier intermittent HTTP test reset
remains unproven.

Managed Git worktrees are registered with `--no-checkout`, then populated from
within the worktree. This avoids Git for Windows' absolute `GIT_DIR` size limit
in its automatic checkout subprocess. WTA's Git commands use command-local
`core.longpaths=true`; no user/global Git setting changes or relocation outside
the managed state root are involved. Failed checkout explicitly reports the
failure and cleanup outcome. Cleanup removes incomplete worktree registration,
not source-repository branch references; a failed cleanup can leave state for
reconciliation. External provider Git commands retain their own
configuration; this is not a claim of unlimited Windows/Git path support.

Immutable capture publication retries only the atomic rename on Windows access,
sharing, or lock errors, with at most six attempts and 620 ms of scheduled delay.
It never recopies changing source data, replaces an existing capture, or falls
back to mutable output. Persistent failure remains explicit. Acceptance failures
identify the affected artifact reference. This handles reproduced transient
delete-sharing locks; it does not identify the holder of the lock in the earlier
verification failure.

`--input-json <file>` reads a complete protocol request; `--input-json -` reads
it from CLI stdin. The Console accepts files, not stdin. Command IDs, guards,
approval references, and payloads from complete requests are preserved, and
conflicting command-line arguments are rejected. Configuration and transfer
commands also accept documented params-only files; `--params-json` is the
explicit inline-JSON form.

Workspace inspection is available before a delivery candidate exists. Guided
transfers use the selected work, or an explicit `--work <work-id>`:

```text
/workspace takeover
/workspace handback --summary "Updated input" --resume-affected false
/inbox
/inbox --work <work-id>
```

The Console previews the exact workspace version and affected contracts;
**Ctrl+Enter** confirms and **Esc** cancels without clearing drafts. Handback
requires a summary and an explicit `--resume-affected true|false`; even `true`
does not override Work Hold. Structured CLI guided transfers require
`--work <work-id> --confirm`. `/inbox` is global; the optional work filter does
not replace the Console's global attention set.

This experiment supports command gates and internal ACP review, not human
checkpoint gates. Spec revisions stay within the existing project,
destination, and authority; permission/budget expansion, removal of executed
tasks, cross-work dependency edits, external publication, and automated repair
are not implemented. Manual takeover/handback and same-grant revisions have
controller coverage; the deterministic real-process conformance journey
requests ACP permission before invoking MCP (including initial and continuation
acknowledgements), rejects unrelated pre-acknowledgement permission probes, and
covers two independent works, context continuation, failure/rework, immutable
captures, and accepted local delivery. That evidence is not a live-model
product-acceptance run.

The [next verification plan](../../doc/specs/agent-center-verification-plan.md)
separates defect regressions, packaged Console behavior, and the real-model
two-work journey. Manual handback conservatively invalidates consumers in the
edited workspace; it does not yet preserve unaffected same-workspace results
at fine granularity. The proposed `$` ordinary-shell shortcut remains deferred.

The packaged app registers `wta.exe` as an App Execution Alias. Before spawning
the host agent, WTA puts the current package family's alias directory first on
`PATH`; unpackaged builds use the running binary's directory. Agent prompts can
therefore use short `wta.exe` commands without selecting another installed
branding or reproducing a protected package path.

### tmux-like CLI

WTA exposes tmux-equivalent subcommands for controlling Windows Terminal from the shell. Useful for humans and AI agents that can shell out.

```bash
wta list-windows                          # list all WT windows
wta list-tabs                             # list tabs in first window
wta list-panes                            # list panes in first tab
wta active-pane                           # show focused pane
wta new-tab -c "pwsh.exe" -n "Build"      # create tab running pwsh
wta split-pane -H -c "pwsh.exe"           # split horizontal
wta capture-pane -t 3 -l 50              # read last 50 lines from pane 3
wta kill-pane -t 3                        # close pane 3
wta pane-status -t 3                      # check if running
wta wait-for -t 3 --timeout 30           # wait for pane 3 to exit
wta resolve-command which --cwd . --json  # resolve from cwd + PATH + shell-specific sources
wta list-windows --json                   # raw JSON output
```

Short aliases are supported: `lsw`, `lst`, `lsp`, `neww`, `splitw`, `capturep`,
`killp`, `setenv`, and `mon`.

When `-t` (target pane) is omitted, the active pane is used automatically.

### Protocol Discovery & Environment Setup

WTA finds Windows Terminal via the `WT_COM_CLSID` environment variable, which
WT propagates into every conpty child it spawns. You usually don't need to do
anything — just run `wta` inside a WT pane.

```bash
# Inspect the inherited value
wta pipe-id                               # print CLSID
wta pipe-id --json                        # JSON with metadata

# Re-export it into another shell session (rarely needed)
eval "$(wta set-env)"                     # bash/zsh
wta set-env -s powershell | Invoke-Expression   # PowerShell
wta set-env -s fish | source              # fish
wta set-env -s cmd                        # cmd (copy-paste output)
```

### Test connectivity

```bash
wta test-pipe
wta --test-pipe     # legacy flag, still works
```

Connects to the WT protocol, prints `list_windows` + `get_capabilities`.

## Protocol Connection

WTA discovers Windows Terminal via the `WT_COM_CLSID` environment variable. WT
sets this in its own environment at startup and propagates it to every conpty
shell, so any pane-launched process — including wta and wtcli — inherits it.

## Environment Variables

| Variable | Required | Description |
|----------|----------|-------------|
| `WT_COM_CLSID` | Yes* | Stringified GUID of WT's `TerminalProtocolComServer` COM class |
| `WTA_LOG` | No | Rust tracing filter, such as `debug` or `trace` |

\* Set automatically by WT when it spawns a conpty child. If you launch `wta` from outside WT, run `eval "$(wta set-env)"` to copy the value over (only useful when you've previously captured it from a WT shell).

## Global CLI Options

| Flag | Description |
|------|-------------|
| `--json` | Output raw JSON instead of human-readable tables |
| `--agent <CMD>` | Agent CLI command for ACP mode (default: `copilot --acp --stdio`) |

## TUI Controls

Tool rows keep a localized type label such as **Run**, **Read**, **Search**, or
**Edit** visible across pending, running, and completed states. Consecutive
successful Read, Search, Edit, and Delete calls collapse into one summary row;
click that row to inspect each call. ACP thought chunks appear in an expanded
**Think** block with muted italic text and a left rule. Each thinking phase
automatically collapses when an answer or tool activity starts, thinking ends,
or the turn completes or is canceled. Click its header to reopen it, including
in completed history. Ctrl+O toggles thinking in the selected history turn, or
the active/latest turn when none is selected. Phase duration is measured locally;
replayed thinking has no duration because ACP does not supply historical timing.
Each block retains the latest 4,000 Unicode characters. No thought text is
invented when a provider is silent. Synthetic waiting feedback uses only the
shimmering Thinking indicator above the input box, never a transcript row.
Expanded Edit details show bounded line-level `+`/`-` hunks computed from ACP
snapshots. Tool headers and groups can be expanded during the active turn as well
as in history. Expanded Search details wrap the provider's `rawInput.query`
(or its title when no query is supplied) and any returned text results. WTA
does not reconstruct queries or results omitted by the provider. Queries retain
the first 4,000 Unicode characters, all scrollable when expanded. Text results show
up to 12 wrapped lines, with `…` for omitted text. Expansion follows the tool
into completed history.

Chat follows new output while you are at the bottom. Scrolling up preserves your
reading position as text streams, tools update, and turns finish; scrolling back
to the bottom resumes following. Sending a prompt or clearing/loading a session
still resets the view. Streaming thinking retains its latest 4,000 characters;
your reading position follows the same retained text even when older text is
trimmed. If the text you were reading is removed or a thinking block collapses,
the view clamps to surviving content.

| Key | Action |
|-----|--------|
| Type + Enter | Send prompt to agent |
| Ctrl+C | Copy selected text; otherwise cancel streaming / quit |
| Up / Down | Browse prompt input history |
| Mouse wheel | Scroll chat (hold Alt to scroll one line) |
| Click a tool header | Expand or collapse that tool's details, live or completed |
| Click a thinking header | Expand or collapse that block, live or completed |
| Ctrl+O | Expand or collapse thinking in the selected/latest turn (or the active turn), and all live and completed tool details |
| Mouse drag | Select a continuous text range |
| Double / triple click | Select a word / line |
| PageUp / PageDown | Scroll chat |
| F12 | Toggle debug panel (pipe traffic viewer) |
| Shift+PageUp/Down | Scroll debug panel |
| Y / N | Quick allow/reject on permission dialog |
| Up / Down / Enter | Navigate permission options |

WTA automatically selects **Allow once** only when the tool matches the exact MCP
server currently bound to that ACP session by master. Master overwrites provider
metadata with that identity on each forwarded permission request and tool update;
correlated calls must match the session, call ID, and current server identity.
Terminal actions still require their action-card confirmation, and
`request_user_input` still presents its question. Foreign or missing identities
(even with the same tool name or server-name prefix) and requests without an
**Allow once** option keep the normal permission dialog. WTA does not grant
persistent approval automatically.

Pending and replayed command suggestions show only the command, without assuming
Run or Insert. After the user chooses, history uses the localized
`Run: <command>` or `Insert: <command>` label. Cancelling retains the command with
a localized cancellation status on the same line, not on the conversation title.
History has no suggestion counts, numbering, or recommendation checkmarks.

## Debug Panel

Press **F12** to open a side panel showing all JSON-RPC messages between WTA and Windows Terminal in real time.

```
[3456.1] >>> {"type":"request","id":"3","method":"list_windows","params":{}}
[3456.1] <<< {"type":"response","id":"3","result":{"windows":[...]},"error":null}
```

- Green `>>>` = request sent to WT
- Cyan `<<<` = response from WT
- Shift+PageUp/Down to scroll

## Debug Logs

WTA writes structured logs under the package log dir, in a per-version
subfolder: `…\LocalCache\Local\IntelligentTerminal\logs\<pkgver>\` when
packaged (or bare `%LOCALAPPDATA%\IntelligentTerminal\logs\` unpackaged):

| File | Contents |
|------|----------|
| `wta-main_master.<UTC-date>.log` | `wta-master`: agent CLI pool, pipe accept loop, per-helper routing |
| `wta-main_helper-{pid}.<UTC-date>.log` | each `wta-helper`: pipe connect, ACP init, prompts, agent responses, TUI lifecycle |
| `wta-cli.<UTC-date>.log` | short-lived CLI helpers (`list-*`, `capture-pane`, `listen`, `sessions`) |
| `terminal-agent-pane.log` | Agent-pane chrome (C++ TerminalApp side) |
| `wta-ensure-host.log` | Background host startup / COM connection / SharedWta lifecycle |
| `wta-acp-debug.log` | ACP protocol debug trace |
| `wta-delegate.<UTC-date>.log` | `?<prompt>` delegation flow |
| `wta-probe.<UTC-date>.log` | Agent/model/session capability probes |
| `wta-install-hooks.<UTC-date>.log` | Hook installation and upgrade diagnostics |
| `wta-panic.<UTC-date>.log` | Synchronous panic backstop when the normal buffered record may not flush |
| `hook-trace.log` | Shell-hook event diagnostics |

Rust WTA streams with dated names rotate daily and retain up to three matching
files. If a daily writer cannot initialize, that stream uses the fixed
`wta-<stream>.log` name in the same directory. Per-PID helper logs are also
reclaimed after three days.

Set `WTA_LOG=debug` for verbose output (debug builds default to `debug`, release
to `info`). The F12 debug panel in the TUI shows protocol traffic live without
tailing log files.

## Project Structure

```
tools/wta/src/
+-- main.rs                    Entry point, role/CLI dispatch, protocol discovery
+-- master/mod.rs             wta-master: owns the agent CLI pool, multiplexes helpers
+-- helper/mod.rs             wta-helper: per-pane entry (reuses the TUI over a pipe)
+-- app.rs                     TUI state machine, event loop, per-tab sessions
|   +-- app/autofix.rs         Autofix detection + suggestion
|   +-- app/turn_state.rs      Per-turn state machine
+-- event.rs                   Crossterm event reader
+-- coordinator.rs             Delegate (?<prompt>) execution
+-- agent_sessions.rs          Session registry (status / liveness model)
+-- session_watcher/           CLI-log status classification per agent
+-- theme.rs                   Color constants
+-- protocol/
|   +-- acp/client.rs          ACP client (agent-CLI side) + helper-side WtaClient
+-- shell/
|   +-- shell_manager.rs       Terminal abstraction (local subprocess or WT pane)
|   +-- wt_channel/
|       +-- mod.rs             WtChannel trait definition
|       +-- cli_channel.rs     wtcli subprocess (CoCreateInstance via wtcli.exe) — all methods
+-- ui/
    +-- layout.rs              Main layout (+ debug panel split)
    +-- chat.rs                Message rendering
    +-- input.rs               Input box with cursor
    +-- permission.rs          Permission modal dialog
    +-- agents_view.rs         Session-management (/sessions) view
    +-- debug_panel.rs         Protocol traffic viewer (F12)
```

## Development

### Prerequisites

- Rust toolchain (edition 2021)
- Windows Terminal with protocol server enabled (for WT integration)
- An ACP-compatible agent CLI (Copilot, Claude ACP adapter, etc.)

### Build and run

Run these commands from the repository root. CI resolves the
`tools/wta/rust-toolchain.toml` `ms-prod-1.93` pin through MSRustup; local
repo-root commands use your installed active toolchain, so changes must remain
compatible with Rust 1.93.

```bash
# A live process may lock the output. Stop only a PID whose executable path
# matches this target; do not kill every wta.exe by name.
cargo build --target x86_64-pc-windows-msvc --manifest-path tools/wta/Cargo.toml

# cargo build does not compile #[cfg(test)] code.
cargo test --target x86_64-pc-windows-msvc --manifest-path tools/wta/Cargo.toml
```

The TUI (master + helper) is launched by Windows Terminal as an agent pane — see
the C++ F5 / `bcz` flow in the repo `AGENTS.md`. From a WT pane you can exercise
the CLI helpers directly with the packaged `wta` app execution alias.

### Development workflow

1. Open Windows Terminal (with the agent pane / protocol server enabled)
2. Run `wta pipe-id` to verify `WT_COM_CLSID` is set
3. Open the agent pane (`>Toggle AI assistant` / `Ctrl+Shift+.`) — WT spawns the
   helper, which connects to master and renders this TUI
4. Press F12 to open the debug panel and see all protocol traffic
5. Interact with the agent -- watch requests/responses flow in real time
6. Use `wta list-panes`, `wta capture-pane` etc. in another pane for debugging

While connecting, the chat activity row shows WTA's current operation: preparing
the agent connection, connecting to the local coordinator, initializing the
connection, refreshing user authentication after login (when supported), reading
the coordinator's session registry snapshot, creating a session, or setting its
model (when requested). Initialization and creation include local preparation
and registration, not just waiting on the agent. `/restart` first shows
"Restarting agent" while old sessions retire. These are local operation
boundaries, not agent-reported progress: they do not expose internal MCP or
model-catalog loading, and reading the registry does not fetch agent history.
A queued session restore shows the actual connection stage first, followed by
short resume context; once connected, it shows only "Resuming session" until
the load completes. The pane does not become connected earlier, and these labels
do not reduce startup time.
After loading, the pane header and model picker use the restored session's
agent-reported model, when available, without switching it to the current
default model. Settings still supplies the requested model for new sessions
and later model changes; an existing confirmed selection stays visible until
the agent confirms the switch.

### Diagnosing a missing current-shell pane

Default logs record failures without requiring `WTA_LOG=debug`:

- `terminal-agent-pane.log`: the actual server PID/window/tab, requested source,
  and why pane selection failed (for example, `active_agent_without_source`,
  `selected_pane_has_no_session`, or `explicit_source_unresolved`). Exceptions from
  the page-context query are logged once at the COM boundary with their HRESULT.
- `wta-main_helper-{pid}.<UTC-date>.log` (or the fixed
  `wta-main_helper-{pid}.log` fallback): `pane_context_unavailable` reasons distinguish
  protocol failure, an agent pane, and unresolved legacy lookup.
  `pane_context_response_contract_error` records invalid responses.
  `prompt_has_no_bound_pane` identifies the affected helper/prompt;
  `terminal_action_no_active_target` records rejection at the action check.

Use **Report a bug** to collect these in the existing log ZIP. These new lines
omit commands, terminal output, titles, and working directories; other existing
logs may contain private data, so inspect the ZIP before sharing it. These are
failure-time observations, not a history of how pane/source state changed.

### Adding a new WT protocol method

1. Declare the method in `src/cascadia/TerminalProtocol/TerminalProtocol.idl`
2. Implement it on `TerminalProtocolComServer` (`src/cascadia/WindowsTerminal/TerminalProtocolComServer.cpp`)
3. Add a `wtcli` subcommand in `src/tools/wtcli/main.cpp` that calls the new method
4. Add a `CliChannel::request` arm in `tools/wta/src/shell/wt_channel/cli_channel.rs` mapping a method name to the new `wtcli` subcommand
5. Rebuild WT, wtcli, and wta

## Architecture Notes

- **ShellManager** owns local terminals and the active `WtChannel`
- **CliChannel** shells out to `wtcli.exe` per call; `wtcli` does `CoCreateInstance` to reach WT's COM server. All methods, including `send_input` (via `wtcli send-keys`), go through this path.
- **Protocol discovery**: `WT_COM_CLSID` env var, inherited from the WT-spawned conpty
- **CLI subcommands** call `CliChannel::connect()` directly; no ShellManager needed
- **Pane identity** is discovered at startup via PID matching (list all panes, find ours)
