# cspell:word lastgroup
"""Stdlib ``unittest`` tests for the PR workflows' concurrency backstop.

Each ``pull_request`` workflow carries a ``concurrency:`` block
that cancels a superseded PR-head run. The load-bearing assertion is the other
half: a ``merge_group`` or ``push`` run must never be cancelled, neither by
``cancel-in-progress`` nor by sharing a group (a shared group evicts its
pending run even with cancellation off), since two queue entries cancelling
each other would dequeue both. This is a static read of the committed YAML
with a small evaluator for the expression subset the blocks use; no Actions
runtime is involved. Run via the repo's ``make tools-tests``.
"""

from __future__ import annotations

import re
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
WORKFLOWS = ROOT / ".github" / "workflows"
PR_WORKFLOWS = (
    "test",
    "lint",
    "sdk",
    "frontend",
    "semantic-pr",
    "token-icon-audit",
)
# The five required-check workflows, which also run as merge-queue checks.
GATE_WORKFLOWS = PR_WORKFLOWS[:5]

TOKEN = re.compile(
    r"\s*(?:(?P<str>'(?:[^']|'')*')|(?P<num>\d+)"
    r"|(?P<op>==|!=|&&|\|\||[!(),])|(?P<path>[A-Za-z_][\w.-]*))"
)


def tokenize(expr):
    tokens, pos, expr = [], 0, expr.strip()
    while pos < len(expr):
        m = TOKEN.match(expr, pos)
        if not m or m.end() == pos:
            raise ValueError(f"cannot tokenize {expr[pos:]!r}")
        kind = m.lastgroup
        tokens.append((kind, m.group(kind)))
        pos = m.end()
    return tokens


def truthy(value):
    return value not in (None, False, 0, "")


def text(value):
    if value is None:
        return ""
    if isinstance(value, bool):
        return "true" if value else "false"
    return str(value)


class Evaluator:
    """Recursive descent over ``||`` < ``&&`` < ``==``/``!=`` < ``!``."""

    def __init__(self, expr, context):
        self.tokens, self.i, self.context = tokenize(expr), 0, context

    def run(self):
        value = self.or_()
        if self.i != len(self.tokens):
            raise ValueError(f"trailing tokens {self.tokens[self.i :]}")
        return value

    def peek(self):
        return self.tokens[self.i][1] if self.i < len(self.tokens) else None

    def take(self, want=None):
        kind, value = self.tokens[self.i]
        if want is not None and value != want:
            raise ValueError(f"expected {want!r}, got {value!r}")
        self.i += 1
        return kind, value

    def or_(self):
        left = self.and_()
        while self.peek() == "||":
            self.take()
            right = self.and_()
            left = left if truthy(left) else right
        return left

    def and_(self):
        left = self.cmp()
        while self.peek() == "&&":
            self.take()
            right = self.cmp()
            left = right if truthy(left) else left
        return left

    def cmp(self):
        left = self.unary()
        while self.peek() in ("==", "!="):
            _, op = self.take()
            right = self.unary()
            left = (left == right) if op == "==" else (left != right)
        return left

    def unary(self):
        if self.peek() == "!":
            self.take()
            return not truthy(self.unary())
        return self.primary()

    def primary(self):
        kind, value = self.take()
        if kind == "str":
            return value[1:-1].replace("''", "'")
        if kind == "num":
            return int(value)
        if value == "(":
            inner = self.or_()
            self.take(")")
            return inner
        if kind == "path" and self.peek() == "(":
            return self.call(value)
        if kind == "path":
            return self.lookup(value)
        raise ValueError(f"unexpected token {value!r}")

    def call(self, name):
        self.take("(")
        args = [self.or_()]
        while self.peek() == ",":
            self.take()
            args.append(self.or_())
        self.take(")")
        if name != "format":
            raise ValueError(f"unsupported function {name!r}")
        return re.sub(r"\{(\d+)\}", lambda m: text(args[1 + int(m.group(1))]), args[0])

    def lookup(self, path):
        value = self.context
        for part in path.split("."):
            value = value.get(part) if isinstance(value, dict) else None
        return value


def render(template, context):
    """Substitute every ``${{ … }}`` span the way Actions renders a field."""
    return re.sub(
        r"\$\{\{(.*?)\}\}",
        lambda m: text(Evaluator(m.group(1), context).run()),
        template,
        flags=re.S,
    )


def concurrency_block(source):
    """Return the ``concurrency:`` mapping's scalars, folded ``>-`` joined."""
    lines = source.splitlines()
    try:
        start = lines.index("concurrency:")
    except ValueError:
        return None
    fields, key = {}, None
    for line in lines[start + 1 :]:
        if line and not line.startswith(" "):
            break
        stripped = line.strip()
        if not stripped or stripped.startswith("#"):
            continue
        m = re.match(r"^  ([\w-]+):\s*(.*)$", line)
        if m:
            key, value = m.group(1), m.group(2)
            fields[key] = [] if value == ">-" else [value.strip("'")]
        elif key is not None:
            fields[key].append(stripped)
    return {k: " ".join(v) for k, v in fields.items()}


def workflow_name(source):
    m = re.search(r"^name: '([^']+)'$", source, re.M)
    return m.group(1) if m else None


def triggers(source):
    m = re.search(r"^'on':\n((?:[ #].*\n?)*)", source, re.M)
    return set(re.findall(r"^  ([\w-]+):", m.group(1), re.M)) if m else set()


def context(event, name, run_id=9001):
    ctx = {"event_name": event, "run_id": run_id, "workflow": name}
    if event == "pull_request":
        ctx["ref"] = "refs/pull/42/merge"
        ctx["event"] = {"pull_request": {"number": 42}}
    elif event == "merge_group":
        ctx["ref"] = "refs/heads/gh-readonly-queue/main/pr-42-0123abcd"
        ctx["event"] = {"merge_group": {"head_sha": "0123abcd"}}
    else:
        ctx["ref"] = "refs/heads/main"
        ctx["event"] = {}
    return {"github": ctx}


class EvaluatorTests(unittest.TestCase):
    def test_or_returns_the_first_truthy_operand(self):
        self.assertEqual(render("${{ a.b || 'x' }}", {"a": {}}), "x")
        self.assertEqual(render("${{ a.b || 'x' }}", {"a": {"b": 7}}), "7")

    def test_and_or_is_the_ternary_idiom(self):
        tpl = "${{ k == 'y' && format('p-{0}', n) || 'q' }}"
        self.assertEqual(render(tpl, {"k": "y", "n": 3}), "p-3")
        self.assertEqual(render(tpl, {"k": "z", "n": 3}), "q")

    def test_a_boolean_renders_as_actions_spells_it(self):
        self.assertEqual(render("${{ 'a' == 'a' }}", {}), "true")
        self.assertEqual(render("${{ !('a' == 'a') }}", {}), "false")

    def test_an_unsupported_function_is_an_error_not_a_pass(self):
        with self.assertRaises(ValueError):
            render("${{ contains('ab', 'a') }}", {})


class ConcurrencyBackstopTests(unittest.TestCase):
    def load(self, stem):
        source = (WORKFLOWS / f"{stem}.yml").read_text()
        block = concurrency_block(source)
        self.assertIsNotNone(block, f"{stem}.yml has no concurrency block")
        self.assertEqual(set(block), {"cancel-in-progress", "group"}, stem)
        name = workflow_name(source)
        self.assertIsNotNone(name, f"{stem}.yml has no name")
        return source, block, name

    def resolve(self, block, event, name, run_id=9001):
        ctx = context(event, name, run_id)
        return render(block["group"], ctx), render(block["cancel-in-progress"], ctx)

    def test_each_gate_workflow_triggers_on_pr_and_queue(self):
        for stem in GATE_WORKFLOWS:
            with self.subTest(stem=stem):
                source, _, _ = self.load(stem)
                self.assertTrue({"pull_request", "merge_group"} <= triggers(source))

    def test_a_pr_head_run_is_keyed_on_the_pr_and_cancels(self):
        for stem in PR_WORKFLOWS:
            with self.subTest(stem=stem):
                _, block, name = self.load(stem)
                group, cancel = self.resolve(block, "pull_request", name)
                self.assertEqual(group, f"{name}-pr-42")
                self.assertEqual(cancel, "true")

    def test_a_merge_group_or_push_run_is_never_cancelled(self):
        for stem in PR_WORKFLOWS:
            for event in ("merge_group", "push"):
                with self.subTest(stem=stem, event=event):
                    _, block, name = self.load(stem)
                    group, cancel = self.resolve(block, event, name)
                    self.assertEqual(group, f"{name}-run-9001")
                    self.assertEqual(cancel, "false")

    def test_two_non_pr_runs_never_share_a_group(self):
        # A shared group would evict the pending run of a burst of main
        # pushes or queue entries even with cancel-in-progress off.
        for stem in PR_WORKFLOWS:
            for event in ("merge_group", "push"):
                with self.subTest(stem=stem, event=event):
                    _, block, name = self.load(stem)
                    first, _ = self.resolve(block, event, name, run_id=1)
                    second, _ = self.resolve(block, event, name, run_id=2)
                    self.assertNotEqual(first, second)

    def test_no_two_workflows_share_a_pr_group(self):
        groups = set()
        for stem in PR_WORKFLOWS:
            _, block, name = self.load(stem)
            groups.add(self.resolve(block, "pull_request", name)[0])
        self.assertEqual(len(groups), len(PR_WORKFLOWS))

    def test_every_pull_request_workflow_carries_a_block(self):
        paths = sorted([*WORKFLOWS.glob("*.yml"), *WORKFLOWS.glob("*.yaml")])
        self.assertTrue(paths)
        missing = []
        for path in paths:
            source = path.read_text()
            events = triggers(source)
            # An unparsed trigger block would silently exempt the file.
            self.assertTrue(events, f"{path.name}: no 'on': block parsed")
            if "pull_request" in events and concurrency_block(source) is None:
                missing.append(path.name)
        self.assertEqual(missing, [])


if __name__ == "__main__":
    unittest.main()
