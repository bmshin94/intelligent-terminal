# Opt-in remote Linux tmux hooks

`it-agent-hook.sh` is a thin **POSIX-shell transport**, not an agent-event
processor. It frames and forwards stdin **unchanged**, including malformed
JSON, conflicting IDs, prompts, tool arguments/output, Unicode, and CR/LF.
Intelligent Terminal (IT) reassembles the body and owns JSON parsing, identity
checks, child-session filtering, redaction/projection, and activity/status
processing. The sender never parses, truncates, or rewrites JSON.

Requirements: Linux, **tmux 3.4+**, `sh`, and GNU/coreutils-compatible `timeout`,
`mktemp`, `rm`, `rmdir`, `head`, `wc`, and `base64` (`--wrap=6000`). No remote
`wtcli`, WTA executable, Python, Node, or `jq` is required by the transport.
OpenCode's own plugin API naturally uses its existing JavaScript runtime.

This is a **separate manual opt-in installation**. It does not replace or
modify managed local `wt-agent-hooks` plugins. IT must have the matching v2
receiver and a tmux control-mode connection to the selected session; an
ordinary terminal attachment alone cannot receive the notifications.

## Install just the shell file on Linux

Copy `it-agent-hook.sh` to the remote host. This example normalizes Windows
checkout line endings and refuses to overwrite an existing installed file:

```sh
tmux -V
destination="$HOME/.local/lib/intelligent-terminal/it-agent-hook.sh"
mkdir -p -- "${destination%/*}"
if ! (umask 077; set -C; sed 's/\r$//' ./it-agent-hook.sh >"$destination"); then
    printf '%s\n' 'Sender already exists or could not be installed; inspect it before updating.' >&2
    exit 1
fi
chmod 700 -- "$destination"
```

Manual invocation uses the same explicit arguments as the native hook bridge:

```sh
sh "$HOME/.local/lib/intelligent-terminal/it-agent-hook.sh" \
  --cli-source copilot --event agent.stop < agent-hook-input.json
```

Run the CLI **inside the tmux pane** attached through IT's integration. Never
forge `TMUX` or `TMUX_PANE` in shell startup files. **Restart the CLI after
installing or changing its hooks.** There is no automatic remote install,
upgrade, reconciliation, or removal.

## Disable, including ACP launches

Any nonempty `WTA_TMUX_HOOKS_DISABLED` value disables this transport (`0` also
disables it). For example:

```sh
WTA_TMUX_HOOKS_DISABLED=1 claude
WTA_TMUX_HOOKS_DISABLED=1 copilot
WTA_TMUX_HOOKS_DISABLED=1 codex
WTA_TMUX_HOOKS_DISABLED=1 gemini
WTA_TMUX_HOOKS_DISABLED=1 opencode
```

Set the same variable on ACP/already-tracked launchers to avoid duplicate hook
tracking; unset it for ordinary interactive launches. OpenCode additionally
honors `OPENCODE_CLIENT=acp`. `WT_SESSION` and `WT_COM_CLSID` are irrelevant.
In particular, the shell sender **does not inspect `sidekick-*` IDs** or any
other stdin fields. That filtering now belongs to IT/WTA.

## CLI-specific configuration

The following examples use a distinct **`it-tmux-hooks`** plugin name and
**`it-tmux-local`** marketplace. Each setup creates a fresh private directory
and stops if that directory already exists. Do not redirect these snippets
over existing user or managed hook files.

The static catalogs match the existing adjacent agent bundles. No
`ErrorOccurred`, tool-completion, or new subagent subscription is invented.
`agent.subagent.stop` is accepted as a compatibility topic, not subscribed
by these examples. CLI plugin APIs are version-dependent; these are Linux
configuration examples, not verification of every installed CLI version.

### Claude Code

```sh
umask 077
root="$HOME/.local/share/it-tmux-hooks/claude"
mkdir -p -- "${root%/*}"
mkdir -- "$root" || exit 1
mkdir -p -- "$root/.claude-plugin" "$root/it-tmux-hooks/.claude-plugin" "$root/it-tmux-hooks/hooks"
cat >"$root/.claude-plugin/marketplace.json" <<'JSON'
{"name":"it-tmux-local","owner":{"name":"Local user"},"plugins":[{"name":"it-tmux-hooks","source":"./it-tmux-hooks","version":"2.0.0"}]}
JSON
cat >"$root/it-tmux-hooks/.claude-plugin/plugin.json" <<'JSON'
{"name":"it-tmux-hooks","version":"2.0.0","description":"Opt-in remote tmux hook transport"}
JSON
cat >"$root/it-tmux-hooks/hooks/hooks.json" <<'JSON'
{"hooks":{
  "SessionStart":[{"matcher":".*","hooks":[{"type":"command","shell":"bash","command":"sh \"$HOME/.local/lib/intelligent-terminal/it-agent-hook.sh\" --cli-source claude --event agent.session.start; exit 0"}]}],
  "SessionEnd":[{"matcher":".*","hooks":[{"type":"command","shell":"bash","command":"sh \"$HOME/.local/lib/intelligent-terminal/it-agent-hook.sh\" --cli-source claude --event agent.session.end; exit 0"}]}],
  "Notification":[{"matcher":".*","hooks":[{"type":"command","shell":"bash","command":"sh \"$HOME/.local/lib/intelligent-terminal/it-agent-hook.sh\" --cli-source claude --event agent.notification; exit 0"}]}],
  "UserPromptSubmit":[{"matcher":".*","hooks":[{"type":"command","shell":"bash","command":"sh \"$HOME/.local/lib/intelligent-terminal/it-agent-hook.sh\" --cli-source claude --event agent.prompt.submit; exit 0"}]}],
  "StopFailure":[{"matcher":".*","hooks":[{"type":"command","shell":"bash","command":"sh \"$HOME/.local/lib/intelligent-terminal/it-agent-hook.sh\" --cli-source claude --event agent.error; exit 0"}]}],
  "Stop":[{"matcher":".*","hooks":[{"type":"command","shell":"bash","command":"sh \"$HOME/.local/lib/intelligent-terminal/it-agent-hook.sh\" --cli-source claude --event agent.stop; exit 0"}]}]
}}
JSON
claude plugin marketplace add "$root"
claude plugin install it-tmux-hooks@it-tmux-local
```

### Copilot CLI

```sh
umask 077
root="$HOME/.local/share/it-tmux-hooks/copilot"
mkdir -p -- "${root%/*}"
mkdir -- "$root" || exit 1
mkdir -p -- "$root/.github/plugin" "$root/it-tmux-hooks/hooks"
cat >"$root/.github/plugin/marketplace.json" <<'JSON'
{"name":"it-tmux-local","owner":{"name":"Local user"},"plugins":[{"name":"it-tmux-hooks","source":"./it-tmux-hooks","version":"2.0.0"}]}
JSON
cat >"$root/it-tmux-hooks/plugin.json" <<'JSON'
{"name":"it-tmux-hooks","version":"2.0.0","hooks":"hooks/hooks.json"}
JSON
cat >"$root/it-tmux-hooks/hooks/hooks.json" <<'JSON'
{"hooks":{
  "SessionStart":[{"matcher":".*","hooks":[{"type":"command","bash":"sh \"$HOME/.local/lib/intelligent-terminal/it-agent-hook.sh\" --cli-source copilot --event agent.session.start; exit 0","timeoutSec":5}]}],
  "SessionEnd":[{"matcher":".*","hooks":[{"type":"command","bash":"sh \"$HOME/.local/lib/intelligent-terminal/it-agent-hook.sh\" --cli-source copilot --event agent.session.end; exit 0","timeoutSec":5}]}],
  "Notification":[{"matcher":".*","hooks":[{"type":"command","bash":"sh \"$HOME/.local/lib/intelligent-terminal/it-agent-hook.sh\" --cli-source copilot --event agent.notification; exit 0","timeoutSec":5}]}],
  "UserPromptSubmit":[{"matcher":".*","hooks":[{"type":"command","bash":"sh \"$HOME/.local/lib/intelligent-terminal/it-agent-hook.sh\" --cli-source copilot --event agent.prompt.submit; exit 0","timeoutSec":5}]}],
  "StopFailure":[{"matcher":".*","hooks":[{"type":"command","bash":"sh \"$HOME/.local/lib/intelligent-terminal/it-agent-hook.sh\" --cli-source copilot --event agent.error; exit 0","timeoutSec":5}]}],
  "Stop":[{"matcher":".*","hooks":[{"type":"command","bash":"sh \"$HOME/.local/lib/intelligent-terminal/it-agent-hook.sh\" --cli-source copilot --event agent.stop; exit 0","timeoutSec":5}]}]
}}
JSON
copilot plugin marketplace add "$root"
copilot plugin install it-tmux-hooks@it-tmux-local
```

### Codex CLI

This catalog has no native end/error subscription.

```sh
umask 077
root="$HOME/.local/share/it-tmux-hooks/codex"
mkdir -p -- "${root%/*}"
mkdir -- "$root" || exit 1
mkdir -p -- "$root/.agents/plugins" "$root/it-tmux-hooks/.codex-plugin" "$root/it-tmux-hooks/hooks"
cat >"$root/.agents/plugins/marketplace.json" <<'JSON'
{"name":"it-tmux-local","interface":{"displayName":"Remote tmux hooks"},"plugins":[{"name":"it-tmux-hooks","source":{"source":"local","path":"./it-tmux-hooks"},"policy":{"installation":"AVAILABLE","authentication":"ON_INSTALL"},"category":"Productivity"}]}
JSON
cat >"$root/it-tmux-hooks/.codex-plugin/plugin.json" <<'JSON'
{"name":"it-tmux-hooks","version":"2.0.0","description":"Opt-in remote tmux hook transport"}
JSON
cat >"$root/it-tmux-hooks/hooks/hooks.json" <<'JSON'
{"hooks":{
  "SessionStart":[{"matcher":"startup|resume","hooks":[{"type":"command","command":"sh \"$HOME/.local/lib/intelligent-terminal/it-agent-hook.sh\" --cli-source codex --event agent.session.start; exit 0"}]}],
  "PermissionRequest":[{"hooks":[{"type":"command","command":"sh \"$HOME/.local/lib/intelligent-terminal/it-agent-hook.sh\" --cli-source codex --event agent.notification; exit 0"}]}],
  "UserPromptSubmit":[{"hooks":[{"type":"command","command":"sh \"$HOME/.local/lib/intelligent-terminal/it-agent-hook.sh\" --cli-source codex --event agent.prompt.submit; exit 0"}]}],
  "Stop":[{"hooks":[{"type":"command","command":"sh \"$HOME/.local/lib/intelligent-terminal/it-agent-hook.sh\" --cli-source codex --event agent.stop; exit 0"}]}]
}}
JSON
codex plugin marketplace add "$root"
codex plugin install it-tmux-hooks@it-tmux-local
```

### Gemini CLI

```sh
umask 077
root="$HOME/.local/share/it-tmux-hooks/gemini"
mkdir -p -- "${root%/*}"
mkdir -- "$root" || exit 1
mkdir -- "$root/hooks"
cat >"$root/gemini-extension.json" <<'JSON'
{"name":"it-tmux-hooks","version":"2.0.0","description":"Opt-in remote tmux hook transport"}
JSON
cat >"$root/hooks/hooks.json" <<'JSON'
{"hooks":{
  "SessionStart":[{"matcher":".*","hooks":[{"type":"command","command":"sh \"$HOME/.local/lib/intelligent-terminal/it-agent-hook.sh\" --cli-source gemini --event agent.session.start; exit 0"}]}],
  "SessionEnd":[{"matcher":".*","hooks":[{"type":"command","command":"sh \"$HOME/.local/lib/intelligent-terminal/it-agent-hook.sh\" --cli-source gemini --event agent.session.end; exit 0"}]}],
  "BeforeAgent":[{"matcher":".*","hooks":[{"type":"command","command":"sh \"$HOME/.local/lib/intelligent-terminal/it-agent-hook.sh\" --cli-source gemini --event agent.prompt.submit; exit 0"}]}],
  "BeforeTool":[{"matcher":".*","hooks":[{"type":"command","command":"sh \"$HOME/.local/lib/intelligent-terminal/it-agent-hook.sh\" --cli-source gemini --event agent.tool.starting; exit 0"}]}],
  "Notification":[{"matcher":".*","hooks":[{"type":"command","command":"sh \"$HOME/.local/lib/intelligent-terminal/it-agent-hook.sh\" --cli-source gemini --event agent.notification; exit 0"}]}],
  "AfterAgent":[{"matcher":".*","hooks":[{"type":"command","command":"sh \"$HOME/.local/lib/intelligent-terminal/it-agent-hook.sh\" --cli-source gemini --event agent.stop; exit 0"}]}]
}}
JSON
gemini extensions install "$root"
```

### OpenCode

Create the user-owned file
`${XDG_CONFIG_HOME:-$HOME/.config}/opencode/plugins/it-tmux-hooks.js` only if it
does not already exist. Use your editor's create-new/no-overwrite operation;
do not replace the managed `wt-agent-hooks.js`.

The adapter below subscribes to native events and supplies their canonical
topic/identity. It serializes the API objects without filtering prompt/tool
content or child sessions. The shell still receives and forwards raw stdin;
IT owns validation and status decisions. This example does not interpret
`session.status`; it uses native chat/tool/idle notifications instead.

```js
import { homedir } from "node:os"
import { join } from "node:path"

export const ItTmuxHooks = async ({ directory }) => {
  const sessions = new Set()
  const enabled = Boolean(process.env.TMUX && process.env.TMUX_PANE) &&
    !process.env.WTA_TMUX_HOOKS_DISABLED && process.env.OPENCODE_CLIENT !== "acp"
  async function emit(topic, id, payload) {
    if (!enabled) return
    const bytes = new TextEncoder().encode(JSON.stringify({
      cwd: directory, ...payload, session_id: id,
    }))
    let child
    try {
      child = Bun.spawn({
        cmd: ["sh", join(homedir(), ".local/lib/intelligent-terminal/it-agent-hook.sh"),
          "--cli-source", "opencode", "--event", topic],
        stdin: bytes, stdout: "ignore", stderr: "inherit",
      })
    } catch (error) {
      console.error("it-tmux-hooks: sender could not start")
      return
    }
    await child.exited.then(
      code => { if (code !== 0) console.error("it-tmux-hooks: sender failed") },
      () => console.error("it-tmux-hooks: sender process failed"),
    )
  }
  const topics = {
    "session.created": "agent.session.start",
    "session.updated": "agent.session.start",
    "session.deleted": "agent.session.end",
    "session.idle": "agent.stop",
    "session.error": "agent.error",
    "permission.asked": "agent.notification",
    "question.asked": "agent.notification",
    "permission.replied": "agent.prompt.submit",
    "question.replied": "agent.prompt.submit",
  }
  return {
    "chat.message": async (input, output) => {
      sessions.add(input.sessionID)
      await emit("agent.prompt.submit", input.sessionID, { ...input, ...output })
    },
    "tool.execute.before": async (input, output) => {
      await emit("agent.tool.starting", input.sessionID,
        { ...input, ...output, tool_name: input.tool, tool_input: output.args })
    },
    event: async ({ event }) => {
      const topic = topics[event.type]
      if (!topic) return
      const properties = event.properties || {}
      const id = properties.info?.id || properties.sessionID
      if (event.type === "session.deleted") sessions.delete(id)
      else if (id) sessions.add(id)
      await emit(topic, id, { ...properties, opencode_event: event })
    },
    dispose: async () => {
      await Promise.all([...sessions].map(id =>
        emit("agent.session.end", id, { reason: "OpenCode exited" })))
      sessions.clear()
    },
  }
}
```

Restart OpenCode after saving the plugin.

## Wire, routing, limits, and privacy

Each tmux literal has this exact v2 shape, with single ASCII spaces:

```text
IT_AGENT_HOOK/2 <session> <pane> <source> <event> <transfer> <index> <count> <data>
```

For example, raw `{}` becomes:

```text
IT_AGENT_HOOK/2 $0 %1 copilot agent.stop it-agent-hook.A1b2C3d4E5f6 0 1 e30=
```

`session`/`pane` are the original tmux IDs, never Windows GUIDs. `source` is
one of `claude`, `copilot`, `codex`, `gemini`, `opencode`. Events are
`agent.session.start`, `agent.session.end`, `agent.prompt.submit`,
`agent.notification`, `agent.tool.starting`, `agent.stop`, `agent.error`, or
`agent.subagent.stop`.

The transfer token is a per-invocation, securely created `mktemp` nonce using
only ASCII letters, digits, `.`, `_`, and `-` (at most 64 characters). It
separates simultaneous hooks; it is **not authentication**. Indexes are
zero-based. Count is 1..234. Standard Base64 data is wrapped at **6000
characters**, always a multiple of four; non-final chunks have exactly 6000
characters and no padding. Empty stdin sends index 0/count 1/empty data,
including the final space before that empty field.

The raw limit is **exactly 1 MiB**. The sender reads one extra byte solely to
detect overflow; 1 MiB+1 is rejected before any notification. Partial reads
that time out are also rejected, never mistaken for complete input.
Large bodies are chunked, not truncated. Every literal remains under 8 KiB.
IT's assembler limits storage to 16 in-flight transfers and 4 MiB of aggregate
encoded data, with a 1 MiB raw-body limit and 10-second expiry. It requires
strictly sequential chunks and consistent metadata throughout a transfer.
Only then does IT parse/project/redact the body. Base64 decoding preserves
even invalid UTF-8 bytes; JSON validation subsequently rejects invalid UTF-8.
Unreleased v1 is explicitly rejected, with no fallback.

Routing is explicit and fail-closed:

1. Split `TMUX` at its last two commas, preserving commas in socket paths.
   Validate the numeric session and `%integer` pane, and bind all tmux calls
   to `-N -S <socket>` so no server is started accidentally.
2. Use `list-panes -s -t '$N'` to verify that the pane still belongs to the
   original session. A stale or moved pane is a no-op; never guess a different
   session. Linked windows still use only the original session.
3. Enumerate only that session with `list-clients -t '$N'` and
   `client_control_mode`, `client_name`, and `session_id`. Retain only control
   clients whose session ID matches.
4. Deliver each literal with `display-message -l -c <client>`. A fixed batch
   of these commands is submitted with tmux `source-file` to avoid hundreds of
   process startups for a large body. All command fields are validated tokens
   or Base64 and are single-quoted in tmux syntax; raw stdin is never shell
   code, `eval` input, or a command argument.

`-l` prevents tmux format/strftime expansion. `%message` bodies arrive as raw
literal text; they are not `%output` escape sequences. **`display-message -C`
is not broadcast** and is not used. No pane text or normal-client status
message is emitted.

Every invocation returns **exit 0 with empty stdout**. Missing tmux,
unsupported tmux, missing environment/socket/session/pane/control clients,
and explicit opt-outs are silent no-ops. Missing required utilities and
actual failures produce a concise stderr diagnostic without raw data or
subprocess arguments.

GNU `timeout` bounds stdin and each tmux call to one second each. The entire
worker process group has a **3.2-second deadline** plus a 0.1-second kill
grace. Scratch creation and each cleanup command have a 0.1-second timeout
plus a 0.05-second kill grace. These budgets total at most 3.75 seconds,
leaving process-startup headroom under the CLI's five-second hook timeout.
No retries or cross-session replay are attempted. IT discards incomplete
transfers if a client disconnects or a batch cannot finish. **An incomplete
transfer never produces partial agent status.**

Temporary data lives in one private `mktemp -d` directory beneath
`${TMPDIR:-/tmp}`, with `umask 077`. Exit/signal traps remove only that
invocation's named files and exact directory; the outer shell also cleans
after worker timeouts. Raw/Base64 data is not retained after normal or timeout
completion. Filesystem failures are reported, not silently ignored.

**Every control client in the same tmux session can see the original raw
payload, including prompts and tool data, before IT redacts it.** This is an
intentional tradeoff of the thin remote transport: trust all those clients
and the remote tmux server. **Base64 is not encryption.** tmux command
history/debugging may also retain the encoded body. Redaction occurs in IT
only after arrival and full reassembly. This opt-in transport is not an
authentication boundary.

## Manual update and uninstall

For an update, disable the remote plugin first, inspect/back up your existing
user-owned sender/configuration, replace only those files intentionally, and
restart the CLI. Setup examples never overwrite a previous installation.

Remove the remote plugin/extension with the matching CLI's supported command:

```sh
claude plugin uninstall it-tmux-hooks@it-tmux-local
copilot plugin uninstall it-tmux-hooks@it-tmux-local
codex plugin uninstall it-tmux-hooks@it-tmux-local
gemini extensions uninstall it-tmux-hooks
# OpenCode: remove only the user-owned example created above.
rm -- "${XDG_CONFIG_HOME:-$HOME/.config}/opencode/plugins/it-tmux-hooks.js"
```

For Claude/Copilot/Codex, also remove the `it-tmux-local` marketplace
registration using that CLI's marketplace removal command and delete only
your generated `~/.local/share/it-tmux-hooks/<cli>` directory after inspection.
For OpenCode, remove only your `it-tmux-hooks.js` file. Restart all affected
CLIs, then delete the installed `it-agent-hook.sh` when nothing references it.
Do **not** remove managed `wt-agent-hooks` plugins. Local WTA install/status/
uninstall commands do not manage this remote installation.

## Tests

From a fresh Windows checkout with PowerShell 7 and WSL Ubuntu:

```powershell
.\tools\wta\wt-agent-hooks\tmux\Test-TmuxAgentHook.ps1 -Distribution Ubuntu
```

`Test-TmuxAgentHook.ps1` copies only `it-agent-hook.sh` and
`test-tmux-hooks.sh` into a private, short Linux-native directory, normalizing
CRLF. This avoids `/mnt/c` Unix-socket/interop issues without requiring an
extra Linux language runtime. The Bash test driver additionally uses standard
GNU text/file tools and util-linux `script` to attach an ordinary PTY client.
Missing test prerequisites fail explicitly; nothing is installed automatically.

The real tmux suite owns one unique socket containing a comma, two sessions,
two origin control clients, another-session control client, and an ordinary
client. A FIFO-driven worker invokes the sender inside an actual pane's
inherited environment. Control-protocol barriers and `tmux wait-for` provide
synchronization without sleeps.

Assertions validate exact v2 framing, ordering, Base64, raw byte equality,
empty stdin, large Unicode/tool output, exactly 1 MiB acceptance, 1 MiB+1
rejection, distinct concurrent transfer IDs, no cross-session/status/pane
injection, opt-outs, utility failures, deadlines, and scratch cleanup.
Only owned client PIDs and the test socket's server are terminated; the
PowerShell wrapper removes its private Linux-native files even on failure.

### Capture a real stream for native receiver tests

After building the x64 Debug TerminalApp unit tests, use a TAEF-enabled
developer shell to export and consume a real two-chunk transfer:

```powershell
$capture = Join-Path $PWD ("obj\tmux-hook-capture-" + [guid]::NewGuid().ToString("N"))
.\tools\wta\wt-agent-hooks\tmux\Test-TmuxAgentHook.ps1 -Distribution Ubuntu `
  -CaptureDirectory $capture
te.exe .\bin\x64\Debug\UnitTests_TerminalApp\Terminal.App.Unit.Tests.dll `
  '/name:*ConsumesCapturedShellMessages*' "/p:TmuxHookCaptureDirectory=$capture"
```

The destination must not already exist. It contains:

- `real-shell-v2.messages`: exact `%message IT_AGENT_HOOK/2 ...` lines with
  their original tmux IDs, transfer nonce, and LF delimiters.
- `real-shell-v2.payload`: the exact unredacted stdin bytes, including UTF-8
  tool output and trailing CR/LF, for a byte-for-byte native comparison.

The body uses `session_id: native-fixture-session`, `cwd: /repo`, and a
`TEST_PROMPT_REDACT_ME` prompt plus tool output to exercise native projection.
The native test feeds the captured messages through the production parser
and assembler, compares the original bytes, and checks native redaction.
These requested artifacts persist in the ignored build directory; Linux
scratch resources are still cleaned. They contain only generated test data.

No CI workflow is added. A future Windows job needs WSL Ubuntu and the command
above; a Linux job can run the Bash driver in the same private-directory
layout. This transport coverage does not exercise packaged XAML pane routing
or live provider authentication.
