#!/usr/bin/env python3
# Copyright (c) Microsoft Corporation.
# Licensed under the MIT license.

"""Opt-in Linux hook bridge to existing tmux control clients; no wtcli required."""

import argparse
import json
import os
import re
import select
import shutil
import stat
import subprocess
import sys
import time


PREFIX = "IT_AGENT_HOOK/1 "
MAX_INPUT_BYTES = 1024 * 1024
MAX_WIRE_BYTES = 8 * 1024
MAX_ID_BYTES = 1024
INPUT_TIMEOUT = 1.0
COMMAND_TIMEOUT = 1.0
TOTAL_TIMEOUT = 4.0
SOURCES = frozenset(("claude", "copilot", "codex", "gemini", "opencode"))
EVENTS = frozenset((
    "agent.session.start",
    "agent.session.end",
    "agent.prompt.submit",
    "agent.notification",
    "agent.tool.starting",
    "agent.stop",
    "agent.error",
    "agent.subagent.stop",
))
METADATA_KEYS = (
    "cwd", "message", "reason", "error", "notification_type", "tool_name",
    "toolName", "session_id", "sessionId",
)
# Keep in sync with USER_INPUT_TOOL_NAMES in tools/wta/src/agent_sessions.rs.
USER_INPUT_TOOLS = frozenset((
    "ask_user", "askuser", "ask-user", "ask_question", "askquestion",
    "askuserquestion", "ask_user_question", "ask_for_clarification",
    "request_input", "request_user_input", "user_input", "prompt_user",
    "clarification_request",
))
QUESTION_KEYS = ("question", "prompt", "message")
CONTEXT_KEYS = ("cwd", "tool_name", "toolName", "notification_type")
TEXT_KEYS = ("message", "reason", "error")
CLIENT_FORMAT = "#{client_control_mode}\t#{client_name}\t#{session_id}"


class HookError(Exception):
    """A fixed diagnostic, never containing input or subprocess output."""


class HookParser(argparse.ArgumentParser):
    def error(self, message):
        raise HookError("invalid arguments")


def read_input(stream, deadline):
    if stream is None or stream.isatty():
        return b""
    fd = stream.fileno()
    chunks = []
    size = 0
    deadline = min(deadline, time.monotonic() + INPUT_TIMEOUT)
    while True:
        remaining = deadline - time.monotonic()
        if remaining <= 0 or not select.select([fd], [], [], remaining)[0]:
            raise HookError("stdin timed out")
        chunk = os.read(fd, min(65536, MAX_INPUT_BYTES + 1 - size))
        if not chunk:
            return b"".join(chunks)
        chunks.append(chunk)
        size += len(chunk)
        if size > MAX_INPUT_BYTES:
            raise HookError("stdin size limit exceeded")


def unique_object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise HookError("duplicate JSON key")
        result[key] = value
    return result


def invalid_constant(value):
    raise HookError("invalid JSON constant")


def parse_payload(raw):
    if len(raw) > MAX_INPUT_BYTES:
        raise HookError("stdin size limit exceeded")
    try:
        text = raw.decode("utf-8")
        payload = json.loads(
            text, object_pairs_hook=unique_object, parse_constant=invalid_constant,
        ) if text.strip() else {}
    except (UnicodeError, ValueError, RecursionError):
        raise HookError("invalid JSON payload") from None
    if payload is None:
        return {}
    if not isinstance(payload, dict):
        raise HookError("payload must be an object or null")
    return payload


def valid_id(value):
    return (
        isinstance(value, str)
        and bool(value)
        and len(value.encode("utf-8")) <= MAX_ID_BYTES
        and not any(char.isspace() or ord(char) < 32 or ord(char) == 127 for char in value)
    )


def project_payload(payload):
    for key in ("session_id", "sessionId"):
        if key in payload and not valid_id(payload[key]):
            raise HookError("invalid agent session ID")
    if (
        "session_id" in payload and "sessionId" in payload
        and payload["session_id"] != payload["sessionId"]
    ):
        raise HookError("conflicting agent session IDs")

    projected = {}
    for key in METADATA_KEYS:
        value = payload.get(key)
        if isinstance(value, str):
            value.encode("utf-8")
            projected[key] = value
    tool = projected.get("tool_name", projected.get("toolName", ""))
    tool_input = payload.get("tool_input")
    if tool.isascii() and tool.lower() in USER_INPUT_TOOLS and isinstance(tool_input, dict):
        question = {}
        for key in QUESTION_KEYS:
            value = tool_input.get(key)
            if isinstance(value, str):
                value.encode("utf-8")
                question[key] = value
        if question:
            projected["tool_input"] = question
    return projected


def origin_from_env(env):
    tmux = env.get("TMUX")
    pane = env.get("TMUX_PANE")
    if not tmux or not pane:
        return None
    fields = tmux.rsplit(",", 2)
    if len(fields) != 3:
        raise HookError("invalid tmux environment")
    socket, pid, session = fields
    if (
        not socket.startswith("/") or "\0" in socket
        or len(socket.encode("utf-8")) > MAX_ID_BYTES
        or not re.fullmatch(r"[0-9]+", pid) or not pid.strip("0")
        or len(pid) > MAX_ID_BYTES
        or not re.fullmatch(r"[0-9]+", session) or len(session) + 1 > MAX_ID_BYTES
        or not re.fullmatch(r"%[0-9]+", pane) or len(pane) > MAX_ID_BYTES
    ):
        raise HookError("invalid tmux environment")
    return socket, "$" + session, pane


def socket_exists(path):
    try:
        return stat.S_ISSOCK(os.stat(path).st_mode)
    except FileNotFoundError:
        return False


def tmux_command(executable, socket, arguments, deadline):
    remaining = deadline - time.monotonic()
    if remaining <= 0:
        raise HookError("hook timed out")
    try:
        return subprocess.run(
            [executable, "-N", "-S", socket, *arguments],
            stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            timeout=min(COMMAND_TIMEOUT, remaining), check=False,
        )
    except subprocess.TimeoutExpired:
        raise HookError("tmux command timed out") from None
    except FileNotFoundError:
        # The binary may disappear between PATH lookup and execution.
        return None
    except OSError:
        raise HookError("tmux command could not run") from None


def lookup_output(result):
    if result is None:
        return None
    if result.returncode:
        # A detached/destroyed session or exited server is an intentional no-op.
        if any(message in result.stderr for message in (
            b"can't find session:", b"no server running",
        )) or (
            b"error connecting to " in result.stderr
            and any(message in result.stderr for message in (
                b"(No such file or directory)", b"(Connection refused)",
            ))
        ):
            return None
        raise HookError("tmux lookup failed")
    try:
        return result.stdout.decode("utf-8")
    except UnicodeError:
        raise HookError("invalid tmux lookup response") from None


def escaped_text_size(value):
    return len(json.dumps(value, ensure_ascii=True)) - 2


def clamp_text(value, budget):
    if escaped_text_size(value) <= budget:
        return value
    suffix = "." * min(3, budget)
    low, high = 0, min(len(value), budget)
    while low < high:
        middle = (low + high + 1) // 2
        if escaped_text_size(value[:middle]) + len(suffix) <= budget:
            low = middle
        else:
            high = middle - 1
    return value[:low] + suffix


def make_wire(session, pane, source, event, payload):
    def encode(value):
        return PREFIX + json.dumps({
            "session_id": session,
            "pane_id": pane,
            "cli_source": source,
            "event": event,
            "payload": value,
        }, ensure_ascii=True, separators=(",", ":"), allow_nan=False)

    wire = encode(payload)
    if len(wire) <= MAX_WIRE_BYTES:
        return wire

    reduced = {key: payload[key] for key in ("session_id", "sessionId") if key in payload}
    if len(encode(reduced)) > MAX_WIRE_BYTES:
        raise HookError("routing size limit exceeded")

    groups = [
        [((key,), payload[key]) for key in CONTEXT_KEYS if key in payload],
        [((key,), payload[key]) for key in TEXT_KEYS if key in payload]
        + [(("tool_input", key), payload["tool_input"][key])
           for key in QUESTION_KEYS if key in payload.get("tool_input", {})],
    ]
    accepted = []
    for fields in groups:
        group = []
        for path, value in fields:
            target = reduced if len(path) == 1 else reduced.setdefault("tool_input", {})
            key = path[-1]
            target[key] = ""
            if len(encode(reduced)) <= MAX_WIRE_BYTES:
                group.append((escaped_text_size(value), target, key, value))
            else:
                del target[key]
                if len(path) == 2 and not target:
                    del reduced["tool_input"]
        accepted.append(group)

    # Reserve routing and field overhead first. Preserve context before display
    # text; within each group, short fields stay intact and large fields share
    # the remainder. IDs are never shortened, and Unicode is cut at code points.
    remaining = MAX_WIRE_BYTES - len(encode(reduced))
    for group in accepted:
        for index, (size, target, key, value) in enumerate(sorted(group, key=lambda item: item[0])):
            allocation = min(size, remaining // (len(group) - index))
            target[key] = clamp_text(value, allocation)
            remaining -= escaped_text_size(target[key])
    return encode(reduced)


def emit(argv, env, stream):
    deadline = time.monotonic() + TOTAL_TIMEOUT
    if env.get("WTA_TMUX_HOOKS_DISABLED"):
        return
    origin = origin_from_env(env)
    if origin is None:
        return
    executable = shutil.which("tmux")
    if executable is None:
        return
    socket, session, pane = origin
    if not socket_exists(socket):
        return

    parser = HookParser(add_help=False, allow_abbrev=False)
    parser.add_argument("--cli-source", required=True, choices=sorted(SOURCES))
    parser.add_argument("--event", required=True, choices=sorted(EVENTS))
    args = parser.parse_args(argv)
    if args.cli_source == "opencode" and env.get("OPENCODE_CLIENT") == "acp":
        return

    version_result = tmux_command(executable, socket, ["-V"], deadline)
    if version_result is None:
        return
    if version_result.returncode:
        raise HookError("tmux version lookup failed")
    version = re.match(rb"tmux ([0-9]+)\.([0-9]+)", version_result.stdout)
    if version is None or tuple(map(int, version.groups())) < (3, 4):
        return

    payload = project_payload(parse_payload(read_input(stream, deadline)))
    if args.cli_source == "copilot" and any(
        payload.get(key, "").startswith("sidekick-") for key in ("session_id", "sessionId")
    ):
        return
    wire = make_wire(session, pane, args.cli_source, args.event, payload)

    panes = lookup_output(tmux_command(
        executable, socket, ["list-panes", "-s", "-t", session, "-F", "#{pane_id}"], deadline,
    ))
    if panes is None or pane not in panes.splitlines():
        return
    clients = lookup_output(tmux_command(
        executable, socket, ["list-clients", "-t", session, "-F", CLIENT_FORMAT], deadline,
    ))
    if clients is None:
        return
    seen = set()
    for line in clients.splitlines():
        fields = line.split("\t")
        if len(fields) != 3:
            raise HookError("invalid tmux client response")
        control, client, client_session = fields
        if control != "1" or client_session != session:
            continue
        if not valid_id(client):
            raise HookError("invalid tmux client ID")
        if client in seen:
            continue
        seen.add(client)
        result = tmux_command(
            executable, socket, ["display-message", "-l", "-c", client, wire], deadline,
        )
        if result is not None and result.returncode:
            raise HookError("tmux notification failed")


def main(argv=None, env=None, stream=None):
    try:
        emit(
            sys.argv[1:] if argv is None else argv,
            os.environ if env is None else env,
            sys.stdin if stream is None else stream,
        )
    except HookError as error:
        diagnostic = str(error)
    except (Exception, KeyboardInterrupt):
        diagnostic = "hook failed"
    else:
        return 0
    try:
        sys.stderr.write("it-agent-hook: " + diagnostic + "\n")
    except Exception:
        pass
    return 0


if __name__ == "__main__":
    sys.exit(main())
