#!/usr/bin/env python3
"""Keep the CI-mirror manifest honest against the workflows and the Makefile.

``cfg/ci-mirror.json`` (read by ``ci_mirror.py``) is the record of what the
local full suite runs for each required status context, so adding a CI job
never needs a doc edit to say what to run locally. This check keeps it from
drifting. It fails, one line per finding, when:

- a workflow that gates the merge queue (a ``merge_group`` trigger, the only
  workflows a required check can live in) runs a ``make`` target no
  manifest row for that workflow lists in ``ci``, or runs ``make`` with an
  argument the walk cannot read as a target (an expansion, a quoted word);
- a row's ``ci`` target is one its workflow no longer runs;
- a required context in the ruleset mirror (``docs/conventions/github-mcp.md``
  → "Required status checks") has no row, or a row names a context the mirror
  does not list;
- a row names a workflow that does not gate the queue;
- a row names a ``ci`` or ``local`` target the Makefile does not define.

Otherwise it exits 0 with one summary line. The YAML walk is line-based and
deliberately minimal: top-level ``on`` / ``jobs`` keys, job ids at indent 2,
and each ``run:`` value (inline, quoted across lines, or a block scalar).

Wired as the ``ci-mirror-check`` hook in ``cfg/pre-commit-lint.yml``, so
``make lint`` and the Lint workflow both run it. Standard library only; not a
Cargo workspace member (``CLAUDE.md`` → "Skill tooling"). Tests:
``tests/test_ci_mirror_check.py``, via ``make tools-tests``.
"""

from __future__ import annotations

import argparse
import re
import sys
from dataclasses import dataclass, field
from pathlib import Path

import ci_mirror

ROOT = Path(__file__).resolve().parents[2]
WORKFLOWS = Path(".github") / "workflows"
MANIFEST = Path("cfg") / "ci-mirror.json"
MAKEFILE = Path("Makefile")
RULESET_DOC = Path("docs") / "conventions" / "github-mcp.md"
RULESET_HEADING = "### Required status checks"

_TOP_KEY_RE = re.compile(r"""^(['"]?)([A-Za-z_][\w-]*)\1:\s*(.*)$""")
_JOB_RE = re.compile(r"^  ([A-Za-z_][\w-]*):\s*(?:#.*)?$")
_COMMENT_RE = re.compile(r"\s+#.*$")
#: ``make`` flags whose value is the next word (``make -C dir x``).
#: ``-j`` / ``-l`` take one only when it is a number, handled inline.
_ARG_FLAGS = frozenset({"-C", "-f", "-I", "-o", "-W", "--directory", "--file"})
_JOB_NAME_RE = re.compile(r"^    name:\s*(.+?)\s*$")
_RUN_RE = re.compile(r"^(\s*(?:-\s+)?)run:\s*(.*)$")
_SHELL_SPLIT_RE = re.compile(r"[;&|()]+")
_TARGET_RE = re.compile(r"^[A-Za-z0-9][\w.-]*$")
_RULE_RE = re.compile(r"^([^\s#:=.][^:=]*?|\.[^\s:=]+)\s*::?(?!=)")
_CONTEXT_LIST_RE = re.compile(r"(?:`[^`]+`[,.]?\s*)+")


@dataclass
class Job:
    name: str
    targets: list[str] = field(default_factory=list)
    unreadable: list[str] = field(default_factory=list)


@dataclass
class Workflow:
    gates_queue: bool
    jobs: dict[str, Job]


def _indent(line: str) -> int:
    return len(line) - len(line.lstrip(" "))


def _unquote(value: str) -> str:
    """A YAML scalar's text: the inside of a leading quoted span (any trailing
    `` # comment`` dropped), else the plain value up to a `` #`` comment."""
    value = value.strip()
    if value[:1] in ("'", '"'):
        end = value.find(value[0], 1)
        return value[1:end] if end > 0 else value[1:]
    return _COMMENT_RE.sub("", value)


def make_targets(command: str) -> tuple[list[str], list[str]]:
    """The targets every ``make`` invocation in a shell command names, and the
    arguments of any invocation this walk cannot read as a target.

    A ``make`` word counts wherever it sits in a ``;`` / ``&&`` / ``|``
    segment, so a keyword or wrapper before it (``then make x``,
    ``FOO=1 make x``, ``timeout 600 make x``) cannot hide it. An argument that
    is not a plain target name (``make "$T"``, ``make ${{ matrix.t }}``, a
    trailing ``\\``) is returned as unreadable rather than skipped, so the
    check fails loudly instead of passing falsely.
    """
    targets: list[str] = []
    unreadable: list[str] = []
    for line in command.splitlines():
        if line.lstrip().startswith("#"):
            continue
        line = _COMMENT_RE.sub("", line)
        for segment in _SHELL_SPLIT_RE.split(line):
            words = segment.split()
            for at, word in enumerate(words):
                if word != "make":
                    continue
                found = False
                rest = words[at + 1 :]
                skip = False
                for k, token in enumerate(rest):
                    if skip:
                        skip = False
                        continue
                    if token in _ARG_FLAGS or (
                        token in ("-j", "-l")
                        and k + 1 < len(rest)
                        and rest[k + 1].isdigit()
                    ):
                        skip = True
                        continue
                    if token.startswith("-") or "=" in token:
                        continue
                    if not _TARGET_RE.match(token):
                        unreadable.append(token)
                        found = True
                        break
                    targets.append(token)
                    found = True
                if not found:
                    unreadable.append("(no target)")
    return targets, unreadable


def parse_workflow(text: str) -> Workflow:
    """Whether a workflow gates the queue, and each job's ``make`` targets."""
    lines = text.splitlines()
    gates = False
    jobs: dict[str, Job] = {}
    section = None
    job: Job | None = None
    i = 0
    while i < len(lines):
        line = lines[i]
        i += 1
        top = _TOP_KEY_RE.match(line)
        if top:
            section, job = top.group(2), None
            if section == "on" and "merge_group" in top.group(3):
                gates = True
            continue
        if section == "on" and re.match(r"^\s+merge_group\b", line):
            gates = True
        if section != "jobs":
            continue
        job_match = _JOB_RE.match(line)
        if job_match:
            job = jobs.setdefault(job_match.group(1), Job(job_match.group(1)))
            continue
        if job is None:
            continue
        name = _JOB_NAME_RE.match(line)
        if name:
            job.name = _unquote(name.group(1))
            continue
        run = _RUN_RE.match(line)
        if not run:
            continue
        key_col = len(run.group(1))
        value = run.group(2)
        body = [] if value[:1] in ("|", ">") else [value]
        while i < len(lines) and (not lines[i].strip() or _indent(lines[i]) > key_col):
            body.append(lines[i].strip())
            i += 1
        command = "\n".join(body)
        if value[:1] not in ("|", ">"):
            command = _unquote(" ".join(body))
        targets, unreadable = make_targets(command)
        job.targets.extend(targets)
        job.unreadable.extend(unreadable)
    return Workflow(gates, jobs)


def makefile_targets(text: str) -> set[str]:
    """Every target a rule line of the Makefile defines."""
    out: set[str] = set()
    for line in text.splitlines():
        if line.startswith("\t"):
            continue
        match = _RULE_RE.match(line)
        if match:
            out.update(t for t in match.group(1).split() if not t.startswith("."))
    return out


def required_contexts(doc: str) -> list[str] | None:
    """The ruleset mirror's context list, or ``None`` when it is not found."""
    _, sep, rest = doc.partition(RULESET_HEADING + "\n")
    if not sep:
        return None
    section = re.split(r"^#", rest, maxsplit=1, flags=re.MULTILINE)[0]
    for para in re.split(r"\n\s*\n", section):
        flat = " ".join(para.split())
        if flat and _CONTEXT_LIST_RE.fullmatch(flat):
            return re.findall(r"`([^`]+)`", flat)
    return None


def check(
    workflows: dict[str, str], manifest: str, makefile: str, doc: str
) -> list[str]:
    """Every drift finding, one line each; empty when the manifest is in sync.

    ``workflows`` maps a file name under ``.github/workflows/`` to its text.
    """
    try:
        rows = ci_mirror.parse(manifest)
    except ci_mirror.ManifestError as exc:
        return [f"{MANIFEST}: {exc}"]
    problems: list[str] = []

    parsed = {n: parse_workflow(t) for n, t in sorted(workflows.items())}
    gating = {n: w for n, w in parsed.items() if w.gates_queue}
    for name, wf in gating.items():
        listed = {t for r in rows if r.workflow == name for t in r.ci}
        for job in wf.jobs.values():
            for arg in dict.fromkeys(job.unreadable):
                problems.append(
                    f"{name} job '{job.name}' runs `make` with an argument this "
                    f"check cannot read as a target ({arg}); name the target "
                    f"literally"
                )
            for target in dict.fromkeys(job.targets):
                if target not in listed:
                    problems.append(
                        f"{name} job '{job.name}' runs `make {target}`, which no "
                        f"{MANIFEST} row for {name} lists in `ci`"
                    )

    required = required_contexts(doc)
    if required is None:
        problems.append(
            f"{RULESET_DOC}: no code-span context list under '{RULESET_HEADING}'"
        )
    else:
        have = {r.context for r in rows}
        for context in required:
            if context not in have:
                problems.append(
                    f"required context '{context}' ({RULESET_DOC}) has no "
                    f"{MANIFEST} row"
                )

    defined = makefile_targets(makefile)
    for r in rows:
        if required is not None and r.context not in required:
            problems.append(
                f"{MANIFEST} row '{r.context}' is not a required context in "
                f"{RULESET_DOC}"
            )
        wf = gating.get(r.workflow)
        if wf is None:
            problems.append(
                f"{MANIFEST} row '{r.context}' names workflow {r.workflow}, "
                f"which is not a merge_group workflow under {WORKFLOWS}"
            )
        else:
            run = {t for job in wf.jobs.values() for t in job.targets}
            for target in r.ci:
                if target not in run:
                    problems.append(
                        f"{MANIFEST} row '{r.context}' lists `ci` target "
                        f"`{target}`, which {r.workflow} no longer runs"
                    )
        for key, targets in (("ci", r.ci), ("local", r.local)):
            for target in targets:
                if target not in defined:
                    problems.append(
                        f"{MANIFEST} row '{r.context}' names `{key}` target "
                        f"`{target}`, which the Makefile does not define"
                    )
    return problems


def check_tree(root: Path) -> tuple[list[str], str]:
    """The findings over a checkout, and the summary line for a clean run."""
    workflows = {
        p.name: p.read_text()
        for p in sorted((root / WORKFLOWS).iterdir())
        if p.suffix in (".yml", ".yaml")
    }
    manifest = (root / MANIFEST).read_text()
    problems = check(
        workflows,
        manifest,
        (root / MAKEFILE).read_text(),
        (root / RULESET_DOC).read_text(),
    )
    gating = sum(parse_workflow(t).gates_queue for t in workflows.values())
    rows = [] if problems else ci_mirror.parse(manifest)
    summary = (
        f"ci-mirror-check: {len(rows)} rows match {gating} merge_group "
        f"workflows, the ruleset mirror and the Makefile"
    )
    return problems, summary


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        prog="ci_mirror_check.py",
        description=(
            f"Fail when {MANIFEST} drifts from the workflows' make targets, "
            "the required contexts, or the Makefile."
        ),
    )
    parser.add_argument(
        "--root", type=Path, default=ROOT, help="checkout root (default: this repo)"
    )
    args = parser.parse_args(argv)
    try:
        problems, summary = check_tree(args.root)
    except OSError as exc:
        print(f"ci-mirror-check: cannot read: {exc}", file=sys.stderr)
        return 2
    for line in problems:
        print(line)
    if problems:
        return 1
    print(summary)
    return 0


if __name__ == "__main__":
    sys.exit(main())
