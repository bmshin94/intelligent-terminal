# Copyright (c) Microsoft Corporation.
# Licensed under the MIT license.

"""Real Linux transport tests. All sockets and workers belong to this test run."""

import json
import os
from pathlib import Path
import re
import select
import shlex
import shutil
import socket
import subprocess
import sys
import time
import unittest
import uuid

import it_agent_hook as hook


HERE = Path(__file__).resolve().parent
TIMEOUT = 10


def pane_worker(address):
    """Run the sender in an actual pane, keeping its PTY entirely untouched."""
    with socket.socket(socket.AF_UNIX) as connection:
        connection.settimeout(TIMEOUT * 3)
        connection.connect(address)
        with connection.makefile("rwb") as channel:
            channel.write(json.dumps({
                "tmux": os.environ["TMUX"], "pane": os.environ["TMUX_PANE"],
            }).encode() + b"\n")
            channel.flush()
            while line := channel.readline():
                request = json.loads(line)
                if request.get("close"):
                    return
                env = dict(os.environ)
                env.pop("WTA_TMUX_HOOKS_DISABLED", None)
                env.pop("OPENCODE_CLIENT", None)
                env.pop("WT_SESSION", None)
                env.pop("WT_COM_CLSID", None)
                env.update(request.get("env", {}))
                result = subprocess.run(
                    [sys.executable, "-B", str(HERE / "it_agent_hook.py"),
                     "--cli-source", request.get("source", "copilot"),
                     "--event", request.get("event", "agent.stop")],
                    input=request["raw"].encode("utf-8"), stdout=subprocess.PIPE,
                    stderr=subprocess.PIPE, env=env, timeout=TIMEOUT, check=False,
                )
                channel.write(json.dumps({
                    "code": result.returncode,
                    "stdout": result.stdout.decode(),
                    "stderr": result.stderr.decode(),
                }).encode() + b"\n")
                channel.flush()


class ControlClient:
    def __init__(self, command, session):
        self.process = subprocess.Popen(
            [*command, "-C", "attach-session", "-t", session],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        )
        self.buffer = b""
        self.lines = []
        try:
            self.barrier()
        except Exception:
            self.close()
            raise

    def readline(self, deadline):
        while b"\n" not in self.buffer:
            remaining = deadline - time.monotonic()
            if remaining <= 0 or not select.select([self.process.stdout], [], [], remaining)[0]:
                raise AssertionError("tmux control client timed out")
            data = os.read(self.process.stdout.fileno(), 65536)
            if not data:
                raise AssertionError("tmux control client exited")
            self.buffer += data
        line, self.buffer = self.buffer.split(b"\n", 1)
        self.lines.append(line)
        return line

    def barrier(self):
        marker = ("barrier-" + uuid.uuid4().hex).encode()
        self.process.stdin.write(b"display-message -p -l " + marker + b"\n")
        self.process.stdin.flush()
        deadline = time.monotonic() + TIMEOUT
        found = False
        while True:
            line = self.readline(deadline)
            if line == marker:
                found = True
            elif found and line.startswith(b"%end "):
                return
            elif line.startswith(b"%error "):
                raise AssertionError("tmux control command failed")

    def messages(self):
        self.barrier()
        return [line for line in self.lines if line.startswith(b"%message ")]

    def close(self):
        if self.process.poll() is None:
            self.process.terminate()
            try:
                self.process.wait(timeout=TIMEOUT)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait(timeout=TIMEOUT)
        self.process.stdin.close()
        self.process.stdout.close()
        self.process.stderr.close()


@unittest.skipUnless(sys.platform.startswith("linux"), "requires Linux tmux")
class TransportTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        executable = shutil.which("tmux")
        if executable is None:
            raise unittest.SkipTest("tmux is not installed")
        version = subprocess.run(
            [executable, "-V"], capture_output=True, timeout=TIMEOUT, check=True,
        ).stdout
        parsed = re.match(rb"tmux (\d+)\.(\d+)", version)
        if not parsed or tuple(map(int, parsed.groups())) < (3, 4):
            raise unittest.SkipTest("tmux >= 3.4 is required")
        cls.executable = executable

    def setUp(self):
        self.work = HERE / (".test-" + uuid.uuid4().hex[:8])
        self.work.mkdir()
        self.addCleanup(shutil.rmtree, self.work)
        # A comma proves TMUX is parsed from the right, not split at every comma.
        self.socket_path = str(self.work / "s,1")
        if len(self.socket_path.encode()) >= 104:
            self.skipTest("checkout path is too long for a Linux Unix socket")
        self.command = [self.executable, "-N", "-S", self.socket_path]
        self.addCleanup(self.stop_server)
        self.listener = socket.socket(socket.AF_UNIX)
        self.addCleanup(self.listener.close)
        self.listener.settimeout(TIMEOUT)
        address = str(self.work / "worker")
        self.listener.bind(address)
        self.listener.listen(2)
        worker = shlex.join([sys.executable, "-B", str(Path(__file__).resolve()), "--pane-worker", address])
        self.workers = []
        self.sessions = []
        self.panes = []
        self.tmux(
            "-f", "/dev/null", "new-session", "-d", "-s", "origin", worker,
            start_server=True,
        )
        self.accept_worker()
        self.tmux("new-session", "-d", "-s", "other", worker)
        self.accept_worker()
        self.clients = []
        for session in (self.sessions[0], self.sessions[0], self.sessions[1]):
            client = ControlClient(self.command, session)
            self.addCleanup(client.close)
            self.clients.append(client)
        self.ordinary_fd = self.attach_ordinary_client()

    def tmux(self, *args, start_server=False, check=True):
        command = self.command if not start_server else [self.executable, "-S", self.socket_path]
        return subprocess.run(
            [*command, *args], stdin=subprocess.DEVNULL, capture_output=True,
            timeout=TIMEOUT, check=check,
        ).stdout

    def stop_server(self):
        subprocess.run(
            [self.executable, "-N", "-S", self.socket_path, "kill-server"],
            stdin=subprocess.DEVNULL, capture_output=True, timeout=TIMEOUT, check=False,
        )

    def accept_worker(self):
        connection, _ = self.listener.accept()
        self.addCleanup(connection.close)
        connection.settimeout(TIMEOUT)
        channel = connection.makefile("rwb")
        self.addCleanup(channel.close)
        hello = json.loads(channel.readline())
        self.assertEqual(hello["tmux"].rsplit(",", 2)[0], self.socket_path)
        self.sessions.append("$" + hello["tmux"].rsplit(",", 2)[2])
        self.panes.append(hello["pane"])
        self.workers.append(channel)

    def attach_ordinary_client(self):
        import fcntl
        import pty
        import struct
        import termios

        master, slave = pty.openpty()
        self.addCleanup(os.close, master)
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 80, 0, 0))
        self.tmux("set-hook", "-g", "client-attached", "wait-for -S ordinary-ready")
        try:
            process = subprocess.Popen(
                [*self.command, "attach-session", "-t", self.sessions[0]],
                stdin=slave, stdout=slave, stderr=slave,
                env={**os.environ, "TERM": "xterm-256color"},
            )
        finally:
            os.close(slave)

        def close():
            if process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=TIMEOUT)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=TIMEOUT)
        self.addCleanup(close)
        self.tmux("wait-for", "ordinary-ready")
        self.tmux("set-hook", "-gu", "client-attached")
        clients = self.tmux("list-clients", "-t", self.sessions[0], "-F", hook.CLIENT_FORMAT)
        self.assertEqual(len(clients.splitlines()), 3)
        self.assertEqual(sum(line.startswith(b"0\t") for line in clients.splitlines()), 1)
        return master

    def emit(self, payload=None, raw=None, **options):
        channel = self.workers[0]
        channel.write(json.dumps({
            "raw": json.dumps(payload, ensure_ascii=True) if raw is None else raw,
            **options,
        }).encode() + b"\n")
        channel.flush()
        result = json.loads(channel.readline())
        self.assertEqual(result["code"], 0)
        self.assertEqual(result["stdout"], "")
        return result["stderr"]

    def assert_notifications(self, expected):
        wanted = [b"%message " + wire.encode("ascii") for wire in expected]
        self.assertEqual(self.clients[0].messages(), wanted)
        self.assertEqual(self.clients[1].messages(), wanted)
        self.assertEqual(self.clients[2].messages(), [])

    def assert_no_terminal_injection(self):
        for pane in self.panes:
            self.assertEqual(self.tmux("capture-pane", "-p", "-t", pane).strip(), b"")
        output = b""
        while select.select([self.ordinary_fd], [], [], 0)[0]:
            output += os.read(self.ordinary_fd, 65536)
        self.assertNotIn(hook.PREFIX.encode(), output)
        for client in self.clients:
            self.assertFalse(any(
                hook.PREFIX.encode() in line for line in client.lines if line.startswith(b"%output ")
            ))

    def test_exact_literal_delivery_to_origin_control_clients_with_linked_window(self):
        self.tmux("link-window", "-s", self.panes[0], "-t", self.sessions[1] + ":")
        message = '\u4f60\u597d \U0001f600\n%message forged\r\0\x1b]2;injected\x07 #{session_name} %Y %H ; "$(echo bad)"'
        message += r' \n \u0041 \\ \" C:\remote\repo'
        payload = {
            "session_id": "raw-agent-id", "cwd": "/repo/\u4f60\u597d",
            "message": message, "prompt": "secret", "tool_output": "secret",
            "tool_name": "AskUserQuestion",
            "tool_input": {"question": "Choose?\n\u4e8c", "command": "secret", "choices": ["secret"]},
            "pane_id": "%999", "window_id": "secret", "tab_id": "secret",
        }
        self.assertEqual(self.emit(payload), "")
        expected = hook.PREFIX + json.dumps({
            "session_id": self.sessions[0], "pane_id": self.panes[0],
            "cli_source": "copilot", "event": "agent.stop", "payload": {
                "cwd": payload["cwd"], "message": message, "tool_name": "AskUserQuestion",
                "session_id": "raw-agent-id", "tool_input": {"question": "Choose?\n\u4e8c"},
            },
        }, ensure_ascii=True, separators=(",", ":"))
        self.assertTrue(all(0x20 <= byte <= 0x7E for byte in expected.encode("ascii")))
        self.assert_notifications([expected])
        self.assert_no_terminal_injection()

    def test_empty_payloads_and_8k_boundary_deliver(self):
        expected = []
        for raw in ("", " \r\n", "null"):
            self.assertEqual(self.emit(raw=raw), "")
            expected.append(hook.make_wire(self.sessions[0], self.panes[0], "copilot", "agent.stop", {}))
        fixed = len(hook.make_wire(
            self.sessions[0], self.panes[0], "copilot", "agent.stop", {"message": ""},
        ))
        for size in (8191, 8192):
            payload = {"message": "x" * (size - fixed)}
            self.assertEqual(self.emit(payload), "")
            expected.append(hook.make_wire(
                self.sessions[0], self.panes[0], "copilot", "agent.stop", payload,
            ))
            self.assertEqual(len(expected[-1].encode()), size)
        self.assertEqual(self.emit({"message": "x" * (8193 - fixed)}), "")
        expected.append(hook.make_wire(
            self.sessions[0], self.panes[0], "copilot", "agent.stop",
            {"message": "x" * (8192 - fixed - 3) + "..."},
        ))
        self.assert_notifications(expected)
        self.assert_no_terminal_injection()

    def test_moved_pane_does_not_guess_its_new_session(self):
        self.tmux("new-window", "-d", "-t", self.sessions[0], "cat")
        self.tmux("move-window", "-s", self.panes[0], "-t", self.sessions[1] + ":")
        self.assertNotIn(
            self.panes[0].encode(),
            self.tmux("list-panes", "-s", "-t", self.sessions[0], "-F", "#{pane_id}").splitlines(),
        )
        self.assertEqual(self.emit({"session_id": "raw-agent-id"}), "")
        self.assert_notifications([])
        self.assert_no_terminal_injection()

    def test_large_ascii_and_unicode_metadata_reduce_and_deliver(self):
        expected = []
        for text in ("x" * 120000, "\u4f60\U0001f600\n\\\"" * 8000):
            payload = {
                "session_id": "raw-agent-id", "sessionId": "raw-agent-id", "cwd": "/repo/\u4f60\u597d",
                "tool_name": "ask_user", "notification_type": "permission_prompt",
                "reason": "waiting", "error": "short error", "message": text,
                "tool_input": {"question": text, "message": "Choose", "command": "secret"},
            }
            self.assertEqual(self.emit(payload, event="agent.tool.starting"), "")
            messages = self.clients[0].messages()
            self.assertEqual(len(messages), len(expected) + 1)
            wire = messages[-1].removeprefix(b"%message ")
            self.assertLessEqual(len(wire), 8192)
            self.assertTrue(all(0x20 <= byte <= 0x7E for byte in wire))
            self.assertTrue(wire.startswith(hook.PREFIX.encode()))
            event = json.loads(wire[len(hook.PREFIX):])
            self.assertEqual(event["session_id"], self.sessions[0])
            self.assertEqual(event["pane_id"], self.panes[0])
            self.assertEqual(event["event"], "agent.tool.starting")
            self.assertEqual(event["cli_source"], "copilot")
            reduced = event["payload"]
            for key in ("session_id", "sessionId", "cwd", "tool_name", "notification_type", "reason", "error"):
                self.assertEqual(reduced[key], payload[key])
            self.assertEqual(reduced["tool_input"]["message"], "Choose")
            for value in (reduced["message"], reduced["tool_input"]["question"]):
                self.assertGreater(len(value), 3)
                self.assertLess(len(value), len(text))
                self.assertTrue(value.endswith("..."))
                self.assertTrue(text.startswith(value[:-3]))
                value.encode("utf-8")
            self.assertNotIn("secret", wire.decode())
            expected.append(wire.decode("ascii"))
            self.assert_notifications(expected)
        self.assert_no_terminal_injection()

    def test_missing_server_no_stdin_and_hostile_inputs(self):
        for payload in (
            {"session_id": "sidekick-child"}, {"sessionId": "sidekick-child"},
        ):
            self.assertEqual(self.emit(payload), "")
        self.assertEqual(self.emit({}, env={"WTA_TMUX_HOOKS_DISABLED": "1"}), "")
        self.assertEqual(self.emit({}, env={"TMUX": ""}), "")
        self.assertEqual(self.emit({}, env={"TMUX_PANE": ""}), "")
        self.assertEqual(self.emit({}, env={"PATH": str(self.work)}), "")
        nonexistent = str(self.work / "missing")
        self.assertEqual(self.emit({}, env={"TMUX": nonexistent + ",123,0"}), "")
        self.assertFalse(Path(nonexistent).exists())
        for raw in ('{"secret":', '["secret"]', '{"session_id":"\\u0000secret"}', "x" * (hook.MAX_INPUT_BYTES + 1)):
            error = self.emit(raw=raw)
            self.assertTrue(error.startswith("it-agent-hook: "))
            self.assertNotIn("secret", error)
        self.assert_notifications([])
        self.assert_no_terminal_injection()


if __name__ == "__main__":
    if len(sys.argv) == 3 and sys.argv[1] == "--pane-worker":
        pane_worker(sys.argv[2])
    else:
        unittest.main()
