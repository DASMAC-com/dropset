#!/usr/bin/env python3
# cspell:word pdropset
"""Unit tests for the private-key guard hook.

One class per rule group, each pinning the shapes it must block **and** the
neighboring shapes it must let through: a guard that blocks `grep Quote` or
`cat frontend/.env.local` is the false-positive class that gets a guard
turned off, and for this one that would leave only the deny rules standing.
The vault name used here is a placeholder; the real one lives only in the
operator's runtime init.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import unittest

HOOKS = os.path.join(
    os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))),
    "hooks",
)
sys.path.insert(0, HOOKS)

import no_private_key_access as guard  # noqa: E402

ENV = {guard.VAULT_ENV: "Example Keys Vault,abc123-vault-id"}
CWD = "/Users/x/repos/dropset/.claude/worktrees/eng-1"


def _bash(command, env=ENV):
    return guard.evaluate(
        {"tool_name": "Bash", "tool_input": {"command": command}}, env, CWD
    )


def _edit(tool, path):
    return guard.evaluate(
        {"tool_name": tool, "tool_input": {"file_path": path}}, ENV, CWD
    )


class Base(unittest.TestCase):
    def assertBlocked(self, *commands):
        for command in commands:
            with self.subTest(command=command):
                self.assertEqual(_bash(command)[0], 2)

    def assertAllowed(self, *commands):
        for command in commands:
            with self.subTest(command=command):
                self.assertEqual(_bash(command), (0, ""))


class SelfTestTests(unittest.TestCase):
    def test_the_hooks_own_self_test_passes(self):
        self.assertEqual(guard._self_test(), 0)


class NameTests(Base):
    """Rule 1: the vault, a key item reference, the agent socket."""

    def test_the_vault_name_or_id_anywhere(self):
        self.assertBlocked(
            "grep -r 'Example Keys Vault' .",
            "echo abc123-VAULT-ID",
            "python3 x.py --vault 'example keys vault'",
        )

    def test_an_op_reference_to_a_key_item(self):
        self.assertBlocked(
            "echo op://Anything/eBook/private",
            "echo 'op://x/Admin signing/key'",
            "echo op://x/Lead/key",
            "echo op://x/quote-ssh/key",
        )

    def test_an_op_reference_into_the_vault(self):
        self.assertBlocked("echo op://abc123-vault-id/other/field")

    def test_the_agent_socket(self):
        self.assertBlocked(
            "ls ~/.1password/agent.sock",
            "stat '/Users/x/Library/Group Containers/2BUA8C4S2C.com.1password/t/agent.sock'",
        )

    def test_bare_titles_are_ordinary_words(self):
        self.assertAllowed(
            "grep -rn Quote programs/",
            "rg 'Admin|Lead' sdk/",
            "echo op://Dev/feeds-api/credential",
            "echo op://Dev/Leader-board/credential",
        )

    def test_unset_vault_env_matches_no_vault(self):
        self.assertEqual(_bash("echo Example Keys Vault", env={})[0], 0)

    def test_a_too_short_vault_value_is_ignored(self):
        self.assertEqual(_bash("ls", env={guard.VAULT_ENV: "a"})[0], 0)


class OpTests(Base):
    """Rule 2: `op` at any command position."""

    def test_leading_op(self):
        self.assertBlocked("op read x", "op", "/opt/homebrew/bin/op whoami")

    def test_after_control_operators(self):
        self.assertBlocked(
            "ls && op read x",
            "ls || op read x",
            "ls; op read x",
            "ls | op inject",
            "ls & op read x",
            "ls\nop read x",
            "(op read x)",
            "if true; then op read x; fi",
        )

    def test_after_wrappers_and_assignments(self):
        self.assertBlocked(
            "FOO=1 op read x",
            "env op read x",
            "env -i FOO=1 op read x",
            "env -u HOME op read x",
            "sudo op read x",
            "sudo -u root op read x",
            "nohup op read x",
            "exec op read x",
            "command op read x",
            "nice -n 5 op read x",
            "timeout 5 op read x",
            "xargs op read < refs",
            "sudo env op read x",
        )

    def test_inside_payloads(self):
        self.assertBlocked(
            "bash -c 'op read x'",
            "sh -ec 'ls; op read x'",
            'zsh -lc "op read x"',
            "eval 'op read x'",
            "env -S 'op read x'",
            "echo $(op read x)",
            'echo "$(op read x)"',
            "echo `op read x`",
            "diff <(op read x) y",
            "find . -exec op read {} +",
            "bash -c \"bash -c 'op read x'\"",
        )

    def test_op_as_an_argument(self):
        self.assertAllowed(
            "which op",
            "command -v op",
            "echo op read x",
            "grep -rn 'op run' Makefile",
            "brew info op",
            "make collectors-up",
        )


class HandleTests(Base):
    """Rule 3: the agent and session handles."""

    def test_handle_assignments(self):
        self.assertBlocked(
            "SSH_AUTH_SOCK=/tmp/s git commit -S -m x",
            "export SSH_AUTH_SOCK=/tmp/s",
            "env SSH_AUTH_SOCK=/tmp/s ssh host",
            "OP_SESSION_team=abc ls",
            "export OP_SERVICE_ACCOUNT_TOKEN=x",
        )

    def test_agent_commands(self):
        self.assertBlocked(
            "ssh-add -l",
            "ssh-add ~/.ssh/id_ed25519",
            "ssh-keygen -Y sign -n file -f k x",
            "ssh-keygen -Ysign -f k x",
        )

    def test_the_session_flag(self):
        self.assertBlocked("tool --session abc", "tool --session=abc")

    def test_neighboring_shapes(self):
        self.assertAllowed(
            "echo $SSH_AUTH_SOCK",
            "ssh-keygen -l -f key.pub",
            "ssh-keygen -Y verify -f allowed -I x -n git -s sig",
            "claude --session-id abc",
            "git commit -m x",
        )


class CockpitTests(Base):
    """Rule 4: the mainnet cockpit."""

    def test_the_mainnet_target(self):
        self.assertBlocked(
            "make tui-mainnet",
            "make -C . tui-mainnet",
            "TUI_KEYS_FILE=x make tui-mainnet",
            "make build tui-mainnet",
        )

    def test_teardown_off_localnet(self):
        self.assertBlocked(
            "make teardown ARGS='--rpc-url https://api.mainnet-beta.solana.com'",
            'make teardown ARGS="--yes --rpc-url https://api.devnet.solana.com"',
            "make teardown ARGS='--rpc-url=https://rpc.example.com'",
            "dropset-teardown --rpc-url https://rpc.example.com",
            "cargo run -p dropset-tui --bin dropset-teardown -- --rpc-url https://x.io",
        )

    def test_teardown_on_localnet(self):
        self.assertAllowed(
            "make teardown",
            "make teardown ARGS='--rpc-url http://127.0.0.1:8899'",
            "make teardown ARGS='--rpc-url http://localhost:8899'",
            "dropset-teardown --rpc-url http://[::1]:8899",
        )

    def test_the_cockpit_on_mainnet_or_with_keys(self):
        self.assertBlocked(
            "dropset-tui --cluster mainnet",
            "./target/release/dropset-tui --cluster=mainnet-beta",
            "dropset-tui --keys-file tui/private-keys.env",
            "cargo run -p dropset-tui -- --cluster mainnet",
            "cargo run --package=dropset-tui -- --keys-file x",
            "cargo run -pdropset-tui -- --cluster mainnet",
            "make tui ARGS='--cluster mainnet'",
        )

    def test_the_cockpit_elsewhere(self):
        self.assertAllowed(
            "make mainnet-demo",
            "make tui",
            "dropset-tui --cluster localnet",
            "cargo run -p dropset-tui",
            "cargo test -p dropset-tui",
            "cargo build -p dropset-tui --release",
        )


class ReferenceFileTests(Base):
    """Rule 5: mutation of the four reference files."""

    def test_edit_tools(self):
        for tool, path in (
            ("Edit", "tui/private-keys.env"),
            ("Write", "/Users/x/repos/dropset/frontend/.env.local"),
            ("MultiEdit", CWD + "/infra/localnet/secrets.local.env"),
            ("Write", "../../../tui/private-keys.env"),
            ("Edit", "~/.config/1Password/ssh/agent.toml"),
        ):
            with self.subTest(tool=tool, path=path):
                self.assertEqual(_edit(tool, path)[0], 2)

    def test_edit_tools_on_neighbors(self):
        for tool, path in (
            ("Edit", "tui/private-keys.env.example"),
            ("Write", "frontend/.env.example"),
            ("Edit", "tui/src/main.rs"),
            ("Edit", "~/.config/1Password/other.toml"),
            ("Read", "tui/private-keys.env"),
            ("NotebookEdit", "tui/private-keys.env"),
        ):
            with self.subTest(tool=tool, path=path):
                self.assertEqual(_edit(tool, path)[0], 0)

    def test_bash_mutation(self):
        self.assertBlocked(
            "echo x > tui/private-keys.env",
            "echo x >> frontend/.env.local",
            "echo x>frontend/.env.local",
            "echo x &> frontend/.env.local",
            "echo x > ../../../frontend/.env.local",
            "rm -f tui/private-keys.env",
            "mv tui/private-keys.env /tmp/x",
            "cp /tmp/x frontend/.env.local",
            "tee -a infra/localnet/secrets.local.env",
            "sed -i '' s/a/b/ frontend/.env.local",
            "perl -pi -e s/a/b/ frontend/.env.local",
            "ln -sf /tmp/x tui/private-keys.env",
            "truncate -s 0 tui/private-keys.env",
            "dd if=/dev/zero of=tui/private-keys.env",
            "echo x > ~/.config/1Password/ssh/agent.toml",
        )

    def test_bash_reads(self):
        self.assertAllowed(
            "cat tui/private-keys.env",
            "cp tui/private-keys.env.example /tmp/x",
            "cp frontend/.env.local /tmp/inspect",
            "sed -n 1p frontend/.env.local",
            "echo x > tui/private-keys.env.example",
            "diff frontend/.env.local frontend/.env.example",
        )


class MessageTests(unittest.TestCase):
    def test_the_message_names_the_trip_and_the_rule(self):
        code, message = _bash("ssh-add -l")
        self.assertEqual(code, 2)
        self.assertIn("What tripped: `ssh-add`", message)
        self.assertIn("every agent session and every", message)
        self.assertIn("sub-agent", message)
        self.assertIn("cannot be rotated", message)
        self.assertIn("Do not route around this block", message)
        self.assertIn("no\nescape marker", message)

    def test_the_message_never_echoes_the_vault_name(self):
        _, message = _bash("echo 'Example Keys Vault'")
        self.assertNotIn("Example Keys Vault", message)


class FailOpenTests(unittest.TestCase):
    def test_odd_payloads_are_allowed(self):
        for payload in (
            None,
            [],
            {},
            {"tool_name": "Bash"},
            {"tool_name": "Bash", "tool_input": "op read x"},
            {"tool_name": "Bash", "tool_input": {"command": 7}},
            {"tool_name": "Edit", "tool_input": {"file_path": None}},
        ):
            with self.subTest(payload=payload):
                self.assertEqual(guard.evaluate(payload, ENV, CWD), (0, ""))

    def test_an_unbalanced_line_fails_open_but_names_still_block(self):
        self.assertEqual(_bash("op read 'unterminated")[0], 0)
        self.assertEqual(_bash("echo 'Example Keys Vault")[0], 2)

    def test_main_reads_stdin_and_exits_two(self):
        script = os.path.join(HOOKS, "no_private_key_access.py")
        payload = json.dumps(
            {"tool_name": "Bash", "tool_input": {"command": "op read x"}}
        )
        result = subprocess.run(
            [sys.executable, script],
            input=payload,
            capture_output=True,
            text=True,
            timeout=30,
        )
        self.assertEqual(result.returncode, 2)
        self.assertIn("BLOCKED", result.stderr)

    def test_main_fails_open_on_garbage(self):
        script = os.path.join(HOOKS, "no_private_key_access.py")
        result = subprocess.run(
            [sys.executable, script],
            input="not json",
            capture_output=True,
            text=True,
            timeout=30,
        )
        self.assertEqual(result.returncode, 0)


if __name__ == "__main__":
    unittest.main()
