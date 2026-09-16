# Copyright (c) Microsoft Corporation.
# Licensed under the MIT license.

import contextlib
import io
import json
import os
import subprocess
import time
import unittest
from unittest import mock

import it_agent_hook as hook


class SenderTests(unittest.TestCase):
    def run_hook(self, raw=b"{}", env=None, argv=None, override=None):
        calls = []

        def run(command, **kwargs):
            self.assertEqual(command[:4], ["/usr/bin/tmux", "-N", "-S", "/test/socket,comma"])
            self.assertEqual(kwargs["stdin"], subprocess.DEVNULL)
            self.assertEqual(kwargs["stdout"], subprocess.PIPE)
            self.assertEqual(kwargs["stderr"], subprocess.PIPE)
            self.assertGreater(kwargs["timeout"], 0)
            self.assertLessEqual(kwargs["timeout"], hook.COMMAND_TIMEOUT)
            self.assertNotIn("shell", kwargs)
            args = command[4:]
            calls.append(args)
            result = override(args) if override else None
            if result is not None:
                return result
            outputs = {
                "-V": b"tmux 3.6\n",
                "list-panes": b"%1\n%2\n",
                "list-clients": (
                    b"1\tclient-a\t$0\n0\t/dev/pts/2\t$0\n"
                    b"1\tclient-b\t$0\n1\tother\t$1\n1\tclient-a\t$0\n"
                ),
                "display-message": b"",
            }
            return subprocess.CompletedProcess(command, 0, outputs[args[0]], b"")

        environment = {"TMUX": "/test/socket,comma,123,0", "TMUX_PANE": "%1"}
        if env:
            environment.update(env)
        stdout, stderr = io.StringIO(), io.StringIO()
        with (
            mock.patch.object(hook.shutil, "which", return_value="/usr/bin/tmux"),
            mock.patch.object(hook, "socket_exists", return_value=True),
            mock.patch.object(hook, "read_input", return_value=raw),
            mock.patch.object(hook.subprocess, "run", side_effect=run),
            contextlib.redirect_stdout(stdout),
            contextlib.redirect_stderr(stderr),
        ):
            code = hook.main(
                ["--cli-source", "copilot", "--event", "agent.stop"] if argv is None else argv,
                environment,
            )
        self.assertEqual(code, 0)
        self.assertEqual(stdout.getvalue(), "")
        return calls, stderr.getvalue()

    def messages(self, calls):
        return [command for command in calls if command[0] == "display-message"]

    def test_exact_contract_and_control_clients_only(self):
        calls, error = self.run_hook(b'{"session_id":"raw-agent-id","cwd":"/repo"}')
        self.assertEqual(error, "")
        expected = (
            'IT_AGENT_HOOK/1 {"session_id":"$0","pane_id":"%1","cli_source":"copilot",'
            '"event":"agent.stop","payload":{"cwd":"/repo","session_id":"raw-agent-id"}}'
        )
        self.assertEqual(self.messages(calls), [
            ["display-message", "-l", "-c", "client-a", expected],
            ["display-message", "-l", "-c", "client-b", expected],
        ])
        self.assertIn(["list-panes", "-s", "-t", "$0", "-F", "#{pane_id}"], calls)
        self.assertIn(["list-clients", "-t", "$0", "-F", hook.CLIENT_FORMAT], calls)

    def test_empty_whitespace_null_and_no_agent_id(self):
        for raw in (b"", b" \r\n\t", b"null", b"{}"):
            with self.subTest(raw=raw):
                calls, error = self.run_hook(raw)
                self.assertEqual(error, "")
                self.assertEqual(json.loads(self.messages(calls)[0][-1][len(hook.PREFIX):])["payload"], {})

    def test_intentional_noops(self):
        for env in (
            {"TMUX": ""}, {"TMUX_PANE": ""}, {"WTA_TMUX_HOOKS_DISABLED": "1"},
        ):
            with self.subTest(env=env):
                calls, error = self.run_hook(env=env)
                self.assertEqual((calls, error), ([], ""))
        for first, output in (("-V", b"tmux 3.3a\n"), ("list-panes", b"%99\n"), ("list-clients", b"")):
            with self.subTest(first=first):
                calls, error = self.run_hook(override=lambda args: (
                    subprocess.CompletedProcess(args, 0, output, b"") if args[0] == first else None
                ))
                self.assertEqual((self.messages(calls), error), ([], ""))

    def test_missing_tmux_and_socket_do_not_read_input(self):
        for patcher in (
            mock.patch.object(hook.shutil, "which", return_value=None),
            mock.patch.object(hook, "socket_exists", return_value=False),
        ):
            with (
                patcher, mock.patch.object(hook, "read_input") as read,
                mock.patch.object(hook.subprocess, "run") as run,
            ):
                self.assertEqual(hook.main([], {
                    "TMUX": "/nonexistent,123,0", "TMUX_PANE": "%1",
                }), 0)
                read.assert_not_called()
                run.assert_not_called()

    def test_original_session_disappeared(self):
        calls, error = self.run_hook(override=lambda args: (
            subprocess.CompletedProcess(args, 1, b"", b"can't find session: $0")
            if args[0] == "list-panes" else None
        ))
        self.assertEqual((self.messages(calls), error), ([], ""))

    def test_connection_failures_distinguish_absence_from_real_errors(self):
        for detail in ("No such file or directory", "Connection refused", "Permission denied"):
            calls, error = self.run_hook(override=lambda args: (
                subprocess.CompletedProcess(
                    args, 1, b"", ("error connecting to secret (" + detail + ")").encode(),
                ) if args[0] == "list-panes" else None
            ))
            self.assertEqual(self.messages(calls), [])
            self.assertEqual(
                error, "it-agent-hook: tmux lookup failed\n" if detail == "Permission denied" else "",
            )

    def test_copilot_children_and_opencode_acp_are_silent(self):
        for field in ("session_id", "sessionId"):
            calls, error = self.run_hook(json.dumps({field: "sidekick-child"}).encode())
            self.assertEqual((self.messages(calls), error), ([], ""))
        calls, error = self.run_hook(
            argv=["--cli-source", "opencode", "--event", "agent.stop"],
            env={"OPENCODE_CLIENT": "acp"},
        )
        self.assertEqual((calls, error), ([], ""))
        calls, error = self.run_hook(env={"WT_SESSION": "", "WT_COM_CLSID": ""})
        self.assertEqual(len(self.messages(calls)), 2)
        self.assertEqual(error, "")

    def test_sources_events_and_arguments_are_strict(self):
        for source in hook.SOURCES:
            for event in hook.EVENTS:
                calls, error = self.run_hook(argv=["--cli-source", source, "--event", event])
                self.assertEqual((len(self.messages(calls)), error), (2, ""))
        for argv in (
            [], ["--help"], ["--cli-source", "Copilot", "--event", "agent.stop"],
            ["--cli-source", "copilot", "--event", "agent.tool.finished"],
            ["--cli-source", "copilot", "--event", "agent.stop\n%exit"],
            ["--cli-source", "copilot", "--event", "agent.stop", "--extra"],
            ["--cli-s", "copilot", "--event", "agent.stop"],
        ):
            calls, error = self.run_hook(argv=argv)
            self.assertFalse(self.messages(calls))
            self.assertEqual(error, "it-agent-hook: invalid arguments\n")

    def test_malformed_and_hostile_payloads_are_private_failures(self):
        for raw in (
            b"{secret", b"[]", b'"secret"', b"true", b"42", b"\xff",
            b'{"secret":NaN}', b'{"secret":Infinity}',
            b'{"session_id":"a","session_id":"b"}',
            b'{"session_id":12}', b'{"session_id":null}', b'{"session_id":""}',
            b'{"session_id":"a","sessionId":"b"}',
            b'{"session_id":"a\\n%exit"}', b'{"session_id":"a\\u0000b"}',
            b'{"message":"\\ud800"}', b'{"session_id":"\\ud800"}',
            b'{"session_id":"' + b"x" * (hook.MAX_ID_BYTES + 1) + b'"}',
            b'{"session_id":"' + "\u00e9".encode() * 513 + b'"}',
            b"[" * 2000 + b"]" * 2000,
            b" " * (hook.MAX_INPUT_BYTES + 1),
        ):
            with self.subTest(raw=raw[:60]):
                calls, error = self.run_hook(raw)
                self.assertFalse(self.messages(calls))
                self.assertTrue(error.startswith("it-agent-hook: "))
                self.assertNotIn("secret", error)
                self.assertLess(len(error), 80)

    def test_route_environment_is_strict(self):
        for env in (
            {"TMUX": "/socket,0,0"}, {"TMUX": "/socket,pid,0"},
            {"TMUX": "/socket,1,$0"}, {"TMUX": "relative,1,0"},
            {"TMUX": "/socket,1,0\n"}, {"TMUX": "/socket\0,1,0"},
            {"TMUX": "/socket,1," + "0" * 1024},
            {"TMUX_PANE": "%1\n"}, {"TMUX_PANE": "%1;kill-server"},
            {"TMUX_PANE": "%" + "0" * 1024},
        ):
            calls, error = self.run_hook(env=env)
            self.assertEqual(calls, [])
            self.assertEqual(error, "it-agent-hook: invalid tmux environment\n")

    def test_projection_never_forwards_arbitrary_nested_data(self):
        payload = {
            "sessionId": "raw", "cwd": "/repo", "message": "notice",
            "error": "failure", "reason": "ended", "notification_type": "permission_prompt",
            "tool_name": "bash", "tool_input": {"command": "secret", "message": "secret"},
            "tool_output": "secret", "transcript": "secret", "prompt": "secret",
            "data": {"message": "secret"}, "session": {"id": "secret"},
            "pane_id": "%999", "tab_id": "secret", "window_id": "secret",
            "WT_SESSION": "secret", "agent_session_id": "secret",
        }
        calls, error = self.run_hook(json.dumps(payload).encode())
        wire = self.messages(calls)[0][-1]
        self.assertNotIn("secret", wire)
        self.assertNotIn("%999", wire)
        self.assertEqual(error, "")
        projected = json.loads(wire[len(hook.PREFIX):])["payload"]
        self.assertEqual(set(projected), {
            "sessionId", "cwd", "message", "error", "reason", "notification_type", "tool_name",
        })
        self.assertEqual(hook.project_payload({"message": {"message": "secret"}, "error": 1}), {})

    def test_question_projection_matches_catalog(self):
        for tool in hook.USER_INPUT_TOOLS:
            for key in ("tool_name", "toolName"):
                payload = {key: tool.upper(), "tool_input": {
                    "question": "Which?", "prompt": "Pick", "message": "Please",
                    "choices": ["secret"], "command": "secret", "data": {"message": "secret"},
                }}
                projected = hook.project_payload(payload)
                self.assertEqual(projected["tool_input"], {
                    "question": "Which?", "prompt": "Pick", "message": "Please",
                })
                self.assertNotIn("secret", json.dumps(projected))
        self.assertNotIn("tool_input", hook.project_payload({
            "tool_name": "bash", "toolName": "ask_user", "tool_input": {"question": "secret"},
        }))
        self.assertNotIn("tool_input", hook.project_payload({
            "tool_name": "ask_user", "tool_input": {"question": {"message": "secret"}},
        }))
        self.assertNotIn("tool_input", hook.project_payload({
            "tool_name": "as\u212a_user", "tool_input": {"question": "secret"},
        }))

    def test_ascii_wire_preserves_unicode_controls_and_literals(self):
        message = '\u4f60\u597d \U0001f600\n%message injected\r\0\x1b]2;title\x07 #{session_name} %Y ; "$(echo unsafe)"'
        message += r' \n \u0041 \\ \" C:\remote\repo'
        calls, error = self.run_hook(json.dumps({"message": message}).encode())
        wire = self.messages(calls)[0][-1]
        self.assertTrue(wire.isascii())
        self.assertTrue(all(0x20 <= byte <= 0x7E for byte in wire.encode("ascii")))
        self.assertNotIn("\n", wire)
        self.assertNotIn("\0", wire)
        self.assertNotIn("\x1b", wire)
        self.assertEqual(json.loads(wire[len(hook.PREFIX):])["payload"]["message"], message)
        self.assertEqual(error, "")

    def test_size_boundaries_count_prefix_and_ascii_expansion(self):
        fixed = len(hook.make_wire("$0", "%1", "copilot", "agent.stop", {"message": ""}))
        capacity = hook.MAX_WIRE_BYTES - fixed
        wire = hook.make_wire("$0", "%1", "copilot", "agent.stop", {"message": "x" * capacity})
        self.assertEqual(len(wire.encode("ascii")), hook.MAX_WIRE_BYTES)
        for message in ("x" * (capacity + 1), "\u4f60" * (capacity // 6 + 1)):
            calls, error = self.run_hook(json.dumps({"message": message}).encode())
            self.assertEqual((len(self.messages(calls)), error), (2, ""))
            wire = self.messages(calls)[0][-1]
            reduced = json.loads(wire[len(hook.PREFIX):])["payload"]["message"]
            self.assertLessEqual(len(wire.encode("ascii")), 8192)
            self.assertTrue(reduced.endswith("..."))
            self.assertTrue(message.startswith(reduced[:-3]))
        raw = b'{"prompt":"' + b"x" * (hook.MAX_INPUT_BYTES - len(b'{"prompt":""} ')) + b'"} '
        self.assertEqual(len(raw), hook.MAX_INPUT_BYTES)
        calls, error = self.run_hook(raw)
        self.assertEqual((len(self.messages(calls)), error), (2, ""))
        self.assertEqual(hook.project_payload({"session_id": "x" * 1024})["session_id"], "x" * 1024)

    def test_oversized_messages_preserve_ids_context_and_short_activity_text(self):
        original = {
            "session_id": "\u00e9" * 512, "sessionId": "\u00e9" * 512,
            "cwd": "/repo/\u4f60\u597d", "tool_name": "ask_user", "toolName": "ask_user",
            "notification_type": "permission_prompt", "reason": "waiting", "error": "short error",
            "message": "\u4f60\U0001f600\n\\\"" * 20000,
            "tool_input": {"question": "\U0001f600" * 4000, "message": "Choose", "prompt": "Pick"},
        }
        calls, error = self.run_hook(json.dumps(original, ensure_ascii=False).encode("utf-8"))
        self.assertEqual((len(self.messages(calls)), error), (2, ""))
        wire = self.messages(calls)[0][-1]
        self.assertLessEqual(len(wire.encode("ascii")), hook.MAX_WIRE_BYTES)
        reduced = json.loads(wire[len(hook.PREFIX):])["payload"]
        for key in ("session_id", "sessionId", *hook.CONTEXT_KEYS, "reason", "error"):
            self.assertEqual(reduced[key], original[key])
        self.assertEqual(reduced["tool_input"]["message"], "Choose")
        self.assertEqual(reduced["tool_input"]["prompt"], "Pick")
        for value, full in (
            (reduced["message"], original["message"]),
            (reduced["tool_input"]["question"], original["tool_input"]["question"]),
        ):
            self.assertTrue(value.endswith("..."))
            self.assertTrue(full.startswith(value[:-3]))
            value.encode("utf-8")

    def test_long_cwd_is_preserved_before_large_display_text(self):
        cwd = "/" + "d" * 4094
        payload = {"cwd": cwd, "message": "x" * 100000, "tool_name": "bash"}
        calls, error = self.run_hook(json.dumps(payload).encode())
        self.assertEqual((len(self.messages(calls)), error), (2, ""))
        reduced = json.loads(self.messages(calls)[0][-1][len(hook.PREFIX):])["payload"]
        self.assertEqual(reduced["cwd"], cwd)
        self.assertEqual(reduced["tool_name"], "bash")
        self.assertTrue(reduced["message"].endswith("..."))

    def test_extreme_context_reduces_without_dropping_the_event(self):
        payload = {
            "session_id": "unchanged", "cwd": "/" + "\U0001f600" * 10000,
            "tool_name": "ask_user", "notification_type": "idle_prompt", "message": "text" * 10000,
        }
        calls, error = self.run_hook(json.dumps(payload).encode())
        self.assertEqual((len(self.messages(calls)), error), (2, ""))
        wire = self.messages(calls)[0][-1]
        self.assertLessEqual(len(wire), hook.MAX_WIRE_BYTES)
        reduced = json.loads(wire[len(hook.PREFIX):])["payload"]
        self.assertEqual(reduced["session_id"], "unchanged")
        self.assertEqual(reduced["tool_name"], "ask_user")
        self.assertEqual(reduced["notification_type"], "idle_prompt")
        self.assertTrue(reduced["cwd"].endswith("..."))

    def test_only_routing_overflow_drops_otherwise_valid_oversized_events(self):
        raw_id = "\u00e9" * 512
        payload = {"session_id": raw_id, "sessionId": raw_id, "message": "private" * 10000}
        calls, error = self.run_hook(json.dumps(payload).encode(), env={
            "TMUX": "/test/socket,comma,123," + "1" * 1023,
            "TMUX_PANE": "%" + "1" * 1023,
        })
        self.assertEqual(self.messages(calls), [])
        self.assertEqual(error, "it-agent-hook: routing size limit exceeded\n")

        # A routing-only envelope that exactly fits must still be sent, even if
        # there is no room for an optional metadata key or a question container.
        routing = {"session_id": "raw"}
        fixed = len(hook.make_wire("$0", "%1", "copilot", "agent.stop", routing))
        with mock.patch.object(hook, "MAX_WIRE_BYTES", fixed):
            wire = hook.make_wire("$0", "%1", "copilot", "agent.stop", {
                **routing, "cwd": "/repo", "tool_name": "ask_user",
                "tool_input": {"question": "private"}, "message": "private",
            })
        self.assertEqual(json.loads(wire[len(hook.PREFIX):])["payload"], routing)
        self.assertEqual(len(wire), fixed)

    def test_clamping_counts_json_escape_bytes_and_keeps_unicode_whole(self):
        for value in ("a" * 20, "\u4f60" * 20, "\U0001f600" * 20, "\n\\\"\0" * 20):
            for budget in range(40):
                with self.subTest(value=value[:5], budget=budget):
                    clipped = hook.clamp_text(value, budget)
                    self.assertLessEqual(hook.escaped_text_size(clipped), budget)
                    clipped.encode("utf-8")
                    if hook.escaped_text_size(value) <= budget:
                        self.assertEqual(clipped, value)
                        continue
                    suffix = "." * min(3, budget)
                    self.assertTrue(clipped.endswith(suffix))
                    if suffix:
                        self.assertTrue(value.startswith(clipped[:-len(suffix)]))

    def test_subprocess_failures_and_timeouts_are_private(self):
        for failed in ("-V", "list-panes", "list-clients", "display-message"):
            calls, error = self.run_hook(override=lambda args: (
                subprocess.CompletedProcess(args, 1, b"secret", b"secret")
                if args[0] == failed else None
            ))
            self.assertNotIn("secret", error)
            self.assertTrue(error.startswith("it-agent-hook: "))
        def timeout(args):
            raise subprocess.TimeoutExpired("secret", 1, b"secret", b"secret")
        calls, error = self.run_hook(override=timeout)
        self.assertEqual(error, "it-agent-hook: tmux command timed out\n")

    def test_input_reader_eof_tty_timeout_and_budget(self):
        with mock.patch.object(hook.sys, "stdin"):
            tty = mock.Mock()
            tty.isatty.return_value = True
            self.assertEqual(hook.read_input(tty, time.monotonic() + 1), b"")
            self.assertEqual(hook.read_input(None, time.monotonic() + 1), b"")
        read_fd, write_fd = os.pipe()
        try:
            with os.fdopen(read_fd, "rb", buffering=0) as stream:
                with self.assertRaisesRegex(hook.HookError, "stdin timed out"):
                    hook.read_input(stream, time.monotonic() + 0.01)
                os.write(write_fd, b'{"message":"ok"}')
                os.close(write_fd)
                write_fd = None
                self.assertEqual(hook.read_input(stream, time.monotonic() + 1), b'{"message":"ok"}')
        finally:
            if write_fd is not None:
                os.close(write_fd)
        with (
            mock.patch.object(hook.select, "select", return_value=([1], [], [])),
            mock.patch.object(hook.os, "read", side_effect=lambda fd, size: b"x" * size),
        ):
            stream = mock.Mock()
            stream.isatty.return_value = False
            with self.assertRaisesRegex(hook.HookError, "size limit"):
                hook.read_input(stream, time.monotonic() + 1)


if __name__ == "__main__":
    unittest.main()
