# Opt-in remote Linux tmux hooks

`it_agent_hook.py` is a **standalone Python 3** sender for agent CLIs running
inside a remote Linux tmux pane. It needs **tmux 3.4 or newer**, but no `wtcli`,
WTA executable, Windows environment variables, `jq`, or Python packages on the
remote machine.

This is a separate, manual installation. It does **not** replace or update the
managed local `wt-agent-hooks` plugins. Installing it does not enable remote
session tracking by itself: the local terminal must have the matching
`IT_AGENT_HOOK/1` receiver and a tmux **control-mode** connection to this session.
An ordinary tmux attachment alone cannot receive these notifications.

## Copy and install on the remote machine

Copy just `it_agent_hook.py` to the Linux host, then run there:

```sh
python3 --version
tmux -V                         # requires 3.4+
install -D -m 644 it_agent_hook.py \
  "$HOME/.local/lib/intelligent-terminal/it_agent_hook.py"
```

The file is invoked through `python3`, so executable permission is unnecessary.
Configure the desired CLI below, **restart that CLI**, and start it inside the
tmux session attached through your terminal's control-mode integration. Do not
forge `TMUX` or `TMUX_PANE` in shell startup files.

For ACP or other already-tracked launches, explicitly disable this separate
remote integration:

```sh
WTA_TMUX_HOOKS_DISABLED=1 your-agent-command
```

Any nonempty value disables the sender. Unset it to re-enable; `0` is also
nonempty. `WT_SESSION` and `WT_COM_CLSID` are deliberately irrelevant. OpenCode
also honors its existing `OPENCODE_CLIENT=acp` signal. Copilot raw session IDs
starting with `sidekick-` are silently ignored. For other providers, configure
the opt-out on the ACP launcher; the sender does not guess how the CLI started.

## CLI hook configuration

Use a distinct plugin name, **`it-tmux-hooks`**, so remote opt-in hooks never
collide with the managed local plugin. The following one-time **Linux setup
example** creates a local marketplace/plugin (or Gemini extension) for one CLI.
Change the argument `copilot` to `claude`, `codex`, or `gemini` as appropriate.
It refuses to overwrite an existing setup directory. It uses only Python's
standard library and the event catalog already used by the adjacent bundles.

```sh
python3 - copilot <<'PY'
import json
from pathlib import Path
import shlex
import sys

source = sys.argv[1]
shared = {
    "SessionStart": "agent.session.start",
    "SessionEnd": "agent.session.end",
    "Notification": "agent.notification",
    "UserPromptSubmit": "agent.prompt.submit",
    "StopFailure": "agent.error",
    "Stop": "agent.stop",
}
catalog = {
    "claude": shared,
    "copilot": shared,
    "codex": {
        "SessionStart": "agent.session.start",
        "PermissionRequest": "agent.notification",
        "UserPromptSubmit": "agent.prompt.submit",
        "Stop": "agent.stop",
    },
    "gemini": {
        "SessionStart": "agent.session.start",
        "SessionEnd": "agent.session.end",
        "BeforeAgent": "agent.prompt.submit",
        "BeforeTool": "agent.tool.starting",
        "Notification": "agent.notification",
        "AfterAgent": "agent.stop",
    },
}
events = catalog[source]
sender = Path.home() / ".local/lib/intelligent-terminal/it_agent_hook.py"
assert sender.is_file(), "Copy the sender first"
root = Path.home() / ".local/share/it-tmux-hooks" / source
root.mkdir(parents=True, exist_ok=False)
plugin = root / "it-tmux-hooks"
plugin.mkdir()

def write(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2) + "\n", encoding="utf-8")

hooks = {}
for native, event in events.items():
    command = f"python3 {shlex.quote(str(sender))} --cli-source {source} --event {event}; exit 0"
    action = {"type": "command", "bash" if source == "copilot" else "command": command}
    if source == "copilot":
        action["timeoutSec"] = 5
    if source == "claude":
        action["shell"] = "bash"
    entry = {"hooks": [action]}
    if source != "codex":
        entry["matcher"] = ".*"
    elif native == "SessionStart":
        entry["matcher"] = "startup|resume"
    hooks[native] = [entry]
write(plugin / "hooks/hooks.json", {"hooks": hooks})
manifest = {
    "name": "it-tmux-hooks", "version": "1.0.0",
    "description": "Opt-in remote tmux session notifications",
}
manifest_path = {
    "claude": ".claude-plugin/plugin.json",
    "copilot": "plugin.json",
    "codex": ".codex-plugin/plugin.json",
    "gemini": "gemini-extension.json",
}[source]
if source == "copilot":
    manifest["hooks"] = "hooks/hooks.json"
write(plugin / manifest_path, manifest)
if source in ("claude", "copilot"):
    marketplace_path = ".claude-plugin/marketplace.json" if source == "claude" else ".github/plugin/marketplace.json"
    write(root / marketplace_path, {
        "name": "it-tmux-local", "owner": {"name": "Local user"},
        "plugins": [{"name": "it-tmux-hooks", "source": "./it-tmux-hooks", "version": "1.0.0"}],
    })
elif source == "codex":
    write(root / ".agents/plugins/marketplace.json", {
        "name": "it-tmux-local", "interface": {"displayName": "Remote tmux hooks"},
        "plugins": [{
            "name": "it-tmux-hooks",
            "source": {"source": "local", "path": "./it-tmux-hooks"},
            "policy": {"installation": "AVAILABLE", "authentication": "ON_INSTALL"},
            "category": "Productivity",
        }],
    })
print(root)
PY
```

Register **only the CLI you configured**, on the remote host:

```sh
# Claude Code
claude plugin marketplace add "$HOME/.local/share/it-tmux-hooks/claude"
claude plugin install it-tmux-hooks@it-tmux-local

# Copilot CLI
copilot plugin marketplace add "$HOME/.local/share/it-tmux-hooks/copilot"
copilot plugin install it-tmux-hooks@it-tmux-local

# Codex CLI
codex plugin marketplace add "$HOME/.local/share/it-tmux-hooks/codex"
codex plugin install it-tmux-hooks@it-tmux-local

# Gemini CLI
gemini extensions install "$HOME/.local/share/it-tmux-hooks/gemini/it-tmux-hooks"
```

CLI hook/plugin APIs are version-dependent. These examples mirror the existing
bundles' catalogs; they do not claim new events or verify every CLI release.
In particular, do not add `ErrorOccurred`, tool-completion events, or
`PreToolUse` to Claude/Copilot. Codex has no end/error hook in this catalog.
`agent.subagent.stop` remains an accepted compatibility topic, not a new
subscription.

### OpenCode

OpenCode uses its V1 JavaScript plugin API, not `hooks.json`. Save this
**user-owned example** as
`${XDG_CONFIG_HOME:-$HOME/.config}/opencode/plugins/it-tmux-hooks.js`. Do not
overwrite the managed `wt-agent-hooks.js`. This adapter only invokes the same
Python sender; it does not implement a second transport or use `wtcli`.

```js
import { homedir } from "node:os"
import { join } from "node:path"

export const ItTmuxHooks = async ({ directory }) => {
  const roots = new Map()
  const enabled = Boolean(process.env.TMUX && process.env.TMUX_PANE) &&
    !process.env.WTA_TMUX_HOOKS_DISABLED && process.env.OPENCODE_CLIENT !== "acp"
  async function emit(event, id, extra = {}) {
    if (!enabled || !id) return
    try {
      const child = Bun.spawn({
        cmd: ["python3", join(homedir(), ".local/lib/intelligent-terminal/it_agent_hook.py"),
          "--cli-source", "opencode", "--event", event],
        stdin: new TextEncoder().encode(JSON.stringify({
          session_id: id, cwd: roots.get(id) || directory, ...extra,
        })),
        stdout: "ignore", stderr: "inherit",
      })
      if (await child.exited) console.error("it-tmux-hooks: sender unavailable")
    } catch {
      console.error("it-tmux-hooks: sender unavailable")
    }
  }
  return {
    "chat.message": async (input) => {
      if (!roots.has(input.sessionID)) return
      await emit("agent.session.start", input.sessionID)
      await emit("agent.prompt.submit", input.sessionID)
    },
    "tool.execute.before": async (input) => {
      if (roots.has(input.sessionID))
        await emit("agent.tool.starting", input.sessionID, { tool_name: input.tool })
    },
    event: async ({ event }) => {
      const p = event.properties || {}
      if (event.type === "session.created" || event.type === "session.updated") {
        const info = p.info
        if (!info?.id) return
        if (info.parentID) { roots.delete(info.id); return }
        const first = !roots.has(info.id)
        roots.set(info.id, info.directory || directory)
        if (first) await emit("agent.session.start", info.id)
        return
      }
      const id = event.type === "session.deleted" ? p.info?.id : p.sessionID
      if (!roots.has(id)) return
      if (event.type === "session.deleted") {
        await emit("agent.session.end", id, { reason: "deleted" })
        roots.delete(id)
      } else if (event.type === "session.error") {
        await emit("agent.error", id, { error: "OpenCode session error" })
      } else if (event.type === "session.idle" ||
                 (event.type === "session.status" && p.status?.type === "idle")) {
        await emit("agent.stop", id)
      } else if ((event.type === "session.status" &&
                  ["busy", "retry"].includes(p.status?.type)) ||
                 ["permission.replied", "question.replied"].includes(event.type)) {
        await emit("agent.prompt.submit", id)
      } else if (["permission.asked", "question.asked"].includes(event.type)) {
        await emit("agent.notification", id, { message: "OpenCode is waiting for input" })
      }
    },
    dispose: async () => {
      await Promise.all([...roots.keys()].map(id =>
        emit("agent.session.end", id, { reason: "OpenCode exited" })))
      roots.clear()
    },
  }
}
```

This conservative example ignores unknown sessions until a
`session.created/updated` event proves they are roots; resumed sessions need
that event before tracking starts. Child sessions are never promoted by a
tool/chat callback. It deliberately forwards neither tool arguments nor full
OpenCode error objects. Restart OpenCode after creating or changing the plugin.

## Routing and privacy contract

The sender issues these operations using subprocess **argument arrays**, never
a shell, `eval`, or an interpolated command string:

1. Parse `TMUX` from the right as `socket,pid,session-number`; commas in the
   socket path are supported. Validate `TMUX_PANE` as `%` plus ASCII digits.
2. Bind every tmux invocation to `-N -S <socket>`; never start a server.
3. Verify the original session contains the pane with
   `list-panes -s -t '$N' -F '#{pane_id}'`. A stale/moved pane is a no-op;
   do not infer a different session. Linked windows still use only the
   session originally inherited through `TMUX`.
4. Enumerate that session's clients with `list-clients -t '$N'` and the formats
   `#{client_control_mode}`, `#{client_name}`, and `#{session_id}`.
   Send once to each control client whose session ID matches, using
   `display-message -l -c <client-name> <literal>`.

tmux 3.4+ delivers the literal as **`%message <literal>`** to the selected
control client, without terminal output or a normal client's status message.
`-l` prevents tmux format/strftime expansion. **`display-message -C` is not
broadcast**; it is unrelated to this protocol.

The `%message` body is raw text: parse the prefixed JSON directly, exactly
once. Do not apply `%output` escape decoding or unescape JSON backslashes
before parsing it. The real transport test compares complete notification
bytes, including literal `\n`, `\u0041`, backslashes, quotes, and JSON's escaped
Unicode/control characters; the wire contains only printable ASCII.

The literal is the prefix `IT_AGENT_HOOK/1 ` followed immediately by compact,
ASCII-escaped JSON, for example:

```json
{"session_id":"$0","pane_id":"%1","cli_source":"copilot","event":"agent.stop","payload":{"session_id":"raw-agent-id"}}
```

The outer IDs are **tmux identities**, not Windows pane GUIDs. The raw agent ID
appears **only inside `payload`**, as `session_id` or `sessionId`; conflicting
aliases are rejected. No remote tab/window/local-pane identity is forwarded.
Missing raw IDs are allowed. Receivers must independently validate and bind
tmux identity to their local connection; this best-effort sender is not an
authentication boundary, and clients/panes can disconnect or move concurrently.

- Allowed sources: `claude`, `copilot`, `codex`, `gemini`, `opencode`.
- Allowed events: `agent.session.start`, `agent.session.end`,
  `agent.prompt.submit`, `agent.notification`, `agent.tool.starting`,
  `agent.stop`, `agent.error`, `agent.subagent.stop`.
- Retain only string metadata: `cwd`, `message`, `reason`, `error`,
  `notification_type`, `tool_name`, `toolName`, `session_id`, `sessionId`.
  Non-string metadata is omitted; present invalid session IDs reject the event.
- Retain string `tool_input.question`, `.prompt`, and `.message` **only**
  for the ASCII case-insensitive user-input tool names in WTA's
  `USER_INPUT_TOOL_NAMES`: `ask_user`, `askuser`, `ask-user`, `ask_question`,
  `askquestion`, `askuserquestion`, `ask_user_question`,
  `ask_for_clarification`, `request_input`, `request_user_input`, `user_input`,
  `prompt_user`, `clarification_request`. These include compatibility aliases.
  An ordinary tool's entire input is discarded.
- No arbitrary nested objects, prompts, transcripts, tool results, choices,
  tool arguments, or unrelated fields are forwarded. The specifically
  allowlisted question/notification text can still be sensitive. tmux's own
  command history/debugging may record forwarded metadata; treat the remote
  tmux server and all control clients of the selected session as trusted.
- stdin accepts absent/empty/whitespace, JSON `null`, or one UTF-8 object.
  Reject malformed/non-object JSON, duplicate keys, non-JSON numeric constants,
  and invalid retained Unicode. Read at most **1 MiB**, with a one-second input
  timeout; reject larger input. Routing IDs are limited to **1024 UTF-8 bytes**
  and reject whitespace/control characters.
- The **sender budget is 8 KiB (8192 ASCII bytes), including the prefix**,
  measured after JSON escaping. This conservative budget fits tmux's command
  IPC and matches the native bridge's event budget; the receiver's defensive
  acceptance limit remains 64 KiB.
- Oversized input is projected first, then oversized retained strings are
  shortened to fit. Raw agent IDs and outer routing fields are **never**
  shortened. Cwd and activity metadata (`tool_name`, `toolName`,
  `notification_type`) have priority over display text; normal paths and
  activity values stay intact even when messages/questions are huge.
  Within each group, short values stay intact and large values share the
  remaining space. Shortened values end with `...` when space allows. Even
  unusually large context values can be shortened; never interpret a
  shortened cwd as a complete path.
- JSON framing and field overhead count toward the budget. If routing nearly
  fills it, optional fields can be empty or omitted. An oversized valid event
  is logged/dropped **only if its routing alone cannot fit**; ordinary large
  messages and Unicode questions reduce and deliver without diagnostics.
  Newlines, NUL, escape characters, Unicode, and percent/format text cannot
  break the control-protocol line. Truncation respects Unicode code-point
  boundaries and escaped byte costs; events are never split.

Every invocation exits successfully and writes **nothing to stdout**, including
invalid arguments (`--help` is not a special output mode). Missing tmux,
unsupported tmux, missing environment/socket/session/pane/control clients,
ACP opt-out, and Copilot child sessions are silent no-ops. Actual failures
write a short fixed diagnostic to stderr, never payloads or tmux error text.
Each subprocess has a one-second timeout, with a four-second overall budget.
Delivery is best effort, without retries or replay to a newly attached client.

## Manual updates and removal

Updates are manual: replace the installed Python file, update your own hook
configuration if necessary, and restart the CLI. If changing a generated
plugin, refresh/reinstall **`it-tmux-hooks`**, not the managed `wt-agent-hooks`.
Do not rerun the setup example over an existing directory.

To disable immediately, set `WTA_TMUX_HOOKS_DISABLED=1` before launching the CLI.
To remove permanently, use the remote CLI's plugin/extension uninstall command
for `it-tmux-hooks`, remove the `it-tmux-local` marketplace registration and
your own generated directory, or remove the OpenCode `it-tmux-hooks.js` file.
Restart affected CLIs. Once nothing references it, delete the installed
`it_agent_hook.py`. Local WTA install/status/uninstall commands do not manage
this remote installation.

## Tests

Run on a **Linux-native filesystem** from the repository root:

```sh
python3 -B -m unittest discover -s tools/wta/wt-agent-hooks/tmux -v
```

For WSL Ubuntu, run that command in a short Linux-native checkout (for example,
under `~/Git/it`), not a Windows-mounted `/mnt/c` checkout: that mount may not
support Unix sockets. No dependency installation is needed. Python-only fake
tests can also be selected with `-p test_sender.py`.

To run the sender, setup example, and existing hook-contract parity checks from
the original checkout (including a Windows-mounted WSL checkout):

```sh
python3 -B -m unittest discover -s tools/wta/wt-agent-hooks/tmux -p 'test_s*.py' -v
```

The suite creates only `.test-*` directories beside these files, never a system
temporary directory. A real test owns one unique socket, two sessions, two
control clients in the origin session, one in the other session, and an
ordinary attached client. A socket-connected worker invokes the actual sender
**inside a real pane's inherited environment**. Control-protocol barriers and
`tmux wait-for` synchronize tests without sleeps. Assertions cover exact JSON,
linked/moved panes, Unicode/newlines/percent/formats, delivery of 8191-byte and
8192-byte envelopes, and delivery after reducing large ASCII messages and
Unicode questions while preserving IDs/cwd/activity metadata. They also check
no terminal or normal-client status injection, silent prerequisites, and
private failures.
Cleanup kills only the server at that test's socket and its own child PIDs.
Real tests skip if Linux/tmux is unavailable or the checkout's socket path is
too long; those skips are not evidence of successful transport validation.

No CI workflow or runner hook is required for this opt-in sender, and this suite
is not currently wired into CI. An optional future job needs only a Linux
runner with Python 3 and tmux 3.4+, a short native-filesystem checkout (for
example, checkout `path: it`), and the full-suite command above. Keep the full
checkout for Rust-array/catalog parity, and verify that no transport tests
were skipped. No Windows package deployment or live agent credentials are
needed.

Verified with Ubuntu **Python 3.14.4 and tmux 3.6**. This suite validates the
sender/transport, not the separately implemented Windows receiver, Rust
consumer, or live provider authentication/ACP flows.
