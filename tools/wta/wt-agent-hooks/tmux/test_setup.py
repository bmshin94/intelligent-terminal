# Copyright (c) Microsoft Corporation.
# Licensed under the MIT license.

import contextlib
import io
import json
from pathlib import Path
import re
import shlex
import shutil
import unittest
from unittest import mock
import uuid

import it_agent_hook as hook


HERE = Path(__file__).resolve().parent


class SetupExampleTests(unittest.TestCase):
    def setUp(self):
        self.home = HERE / (".test-doc-" + uuid.uuid4().hex[:8])
        self.home.mkdir()
        self.addCleanup(shutil.rmtree, self.home)
        self.sender = self.home / ".local/lib/intelligent-terminal/it_agent_hook.py"
        self.sender.parent.mkdir(parents=True)
        shutil.copyfile(HERE / "it_agent_hook.py", self.sender)
        readme = (HERE / "README.md").read_text(encoding="utf-8")
        code = re.search(r"python3 - copilot <<'PY'\n(.*?)\nPY\n", readme, re.DOTALL)
        self.assertIsNotNone(code)
        self.setup_code = compile(code[1], "README setup example", "exec")

    def generate(self, source):
        with (
            mock.patch.object(Path, "home", return_value=self.home),
            mock.patch("sys.argv", ["setup", source]),
            contextlib.redirect_stdout(io.StringIO()),
        ):
            exec(self.setup_code, {})
        return self.home / ".local/share/it-tmux-hooks" / source

    def test_setup_generates_separate_native_plugins_and_safe_hook_commands(self):
        references = {
            "claude": HERE.parent / "claude/wt-agent-hooks/hooks/hooks.json",
            "copilot": HERE.parent / "copilot/wt-agent-hooks/hooks/hooks.json",
            "codex": HERE.parent / "codex/wt-agent-hooks/hooks/hooks.json",
            "gemini": HERE.parent / "gemini-extension/hooks/hooks.json",
        }
        manifests = {
            "claude": ".claude-plugin/plugin.json",
            "copilot": "plugin.json",
            "codex": ".codex-plugin/plugin.json",
            "gemini": "gemini-extension.json",
        }
        for source, reference in references.items():
            with self.subTest(source=source):
                root = self.generate(source)
                plugin = root / "it-tmux-hooks"
                manifest = json.loads((plugin / manifests[source]).read_text())
                self.assertEqual(manifest["name"], "it-tmux-hooks")
                hooks = json.loads((plugin / "hooks/hooks.json").read_text())["hooks"]
                expected = json.loads(reference.read_text())["hooks"] if reference.is_file() else None
                if expected is not None:
                    self.assertEqual(set(hooks), set(expected))
                for native, entries in hooks.items():
                    self.assertEqual(len(entries), 1)
                    action = entries[0]["hooks"][0]
                    key = "bash" if source == "copilot" else "command"
                    command = action[key]
                    self.assertTrue(command.endswith("; exit 0"))
                    args = shlex.split(command.removesuffix("; exit 0"))
                    self.assertEqual(args[:4], ["python3", str(self.sender), "--cli-source", source])
                    self.assertEqual(args[4], "--event")
                    self.assertIn(args[5], hook.EVENTS)
                    if expected is not None:
                        old = expected[native][0]["hooks"][0]
                        old_command = old.get("bash", old.get("command", ""))
                        self.assertIn("--event " + args[5], old_command)
                        self.assertEqual(entries[0].get("matcher"), expected[native][0].get("matcher"))
                    self.assertNotIn("wtcli", command)
                with self.assertRaises(FileExistsError):
                    self.generate(source)

    def test_shared_contract_arrays_match_when_run_from_checkout(self):
        wta = HERE.parents[1]
        app = wta / "src/app.rs"
        sessions = wta / "src/agent_sessions.rs"
        if not app.is_file() or not sessions.is_file():
            self.skipTest("standalone test copy has no WTA contract arrays")

        def contract(path, name):
            text = path.read_text(encoding="utf-8")
            array = re.search(r"pub const " + name + r":.*?= &\[(.*?)\];", text, re.DOTALL)
            self.assertIsNotNone(array)
            return set(re.findall(r'"([^"]+)"', array[1]))

        self.assertEqual(
            contract(app, "CONSUMED_PAYLOAD_KEYS"),
            (set(hook.METADATA_KEYS) - {"session_id", "sessionId"}) | {"tool_input"},
        )
        self.assertEqual(contract(app, "CONSUMED_TOOL_INPUT_KEYS"), set(hook.QUESTION_KEYS))
        self.assertEqual(contract(sessions, "USER_INPUT_TOOL_NAMES"), set(hook.USER_INPUT_TOOLS))


if __name__ == "__main__":
    unittest.main()
