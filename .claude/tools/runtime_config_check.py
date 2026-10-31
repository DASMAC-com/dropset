#!/usr/bin/env python3
"""Check the untracked runtime config against its committed manifest.

The runtime config is the operator's shell profile: the model tiers, the
1Password coordinates, the Linear ids and the machine's fleet knobs. It is
never committed (model ids and secret coordinates are ruled out of the repo),
so its SHAPE is: ``.claude/shell/runtime-config.manifest.json`` holds one row
per variable the shell init or a tool reads — its kind, whether it is required,
the repo default where one exists, the verbs that need it and the convention
anchor — and ``.claude/shell/runtime-config.example.zsh`` is a placeholder
copy of the config itself.

``--static`` checks the manifest against the code, reading only committed
files, so CI enforces it (a lint hook beside ``skill-size``):

* every ``DS_*`` / ``LINEAR_*`` / ``CARGO_*`` name read in the shell init (a
  name on a non-comment line) or under ``.claude/tools/`` or ``.claude/hooks/``
  (a quoted literal, tests excluded) is a manifest row or a listed
  ``external`` name;
* every row is read somewhere — a stale row fails as an undocumented read does;
* a knob or toggle row's ``default`` equals every literal default the code
  applies to it (``${NAME:-d}`` in the shell, ``("NAME", d)`` in a tool), and
  a row with no default has none in the code;
* every row the config sets is named in the example, so it cannot drift
  either.

A read through an indirection the scan cannot see (a name built at runtime)
fails open on that one variable; ``--live`` then lists it as unknown when it
appears in the environment.

``--live`` reads the environment it is handed: each required row ``set`` or
``MISSING``, each optional knob at its default or ``pinned: NAME=value
(default d)``, and every ``DS_*`` / ``LINEAR_*`` name the manifest does not
know. A row's ``required_unless`` map (``{"DS_CARGO_POOL": "0"}``) makes it
optional while another variable holds that value. The value of a model id, a
secret reference or a Linear id is never printed, only ``set`` — and such a
row may carry no default, so no ``pinned:`` line can print one either. A Python process sees only EXPORTED variables, so a profile's plain
assignments reach it only through a caller that hands them over — the
``_ds_upkeep`` shell helper does, for the upkeep pass's ``config`` step.
``--names`` prints the row names that helper passes, one per line.

Exit 0 clean, 1 drift (static) or a missing required row (live), 2 on an
unreadable or malformed manifest. Standard library only; a skill-tool, not a
Cargo workspace member. Tests live in ``tests/test_runtime_config_check.py``.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import sys
from dataclasses import dataclass, field
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
MANIFEST = Path(".claude/shell/runtime-config.manifest.json")
EXAMPLE = Path(".claude/shell/runtime-config.example.zsh")
SHELL_FILES = (Path(".claude/shell/init.zsh"),)
TOOLS_DIR = Path(".claude/tools")
#: Python read the same way as the tools: a quoted name is a read.
PYTHON_DIRS = (TOOLS_DIR, Path(".claude/hooks"))

KINDS = (
    "knob",
    "launcher-set",
    "linear-id",
    "model-id",
    "retired",
    "secret-ref",
    "toggle",
)
#: Kinds whose value is never printed, only whether it is set.
OPAQUE = ("linear-id", "model-id", "secret-ref")
#: Kinds the config never sets: the launcher sets them for one child, or the
#: shell names them only to warn that they are retired.
NOT_CONFIG = ("launcher-set", "retired")
#: Kinds whose repo default the static mode holds against the code's.
DEFAULTED = ("knob", "toggle")

PREFIX = r"(?:DS|LINEAR|CARGO)_[A-Z0-9_]*[A-Z0-9]"
NAME_RE = re.compile(rf"(?<![A-Za-z0-9_])({PREFIX})(?![A-Za-z0-9_])")
QUOTED_RE = re.compile(rf"""["']({PREFIX})["']""")
SHELL_DEFAULT_RE = re.compile(rf"\$\{{({PREFIX}):-([^}}]*)\}}")
TOOL_DEFAULT_RE = re.compile(
    rf"""\(\s*["']({PREFIX})["']\s*,\s*(\d+(?:\.\d+)?|"[^"]*"|'[^']*')\s*[,)]"""
)


@dataclass
class Reads:
    """Where each name is read, and the literal defaults applied to it."""

    sites: dict[str, list[str]] = field(default_factory=dict)
    defaults: dict[str, set[str]] = field(default_factory=dict)

    def add(self, name: str, where: str) -> None:
        self.sites.setdefault(name, []).append(where)

    def default(self, name: str, value: str) -> None:
        self.defaults.setdefault(name, set()).add(value.strip("\"'"))


class ManifestError(Exception):
    pass


def load_manifest(path: Path) -> dict:
    try:
        data = json.loads(path.read_text())
    except (OSError, ValueError) as e:
        raise ManifestError(f"cannot read {path}: {e}") from None
    rows = data.get("variables")
    if not isinstance(rows, list) or not isinstance(data.get("external"), dict):
        raise ManifestError(f"{path}: needs a `variables` list and `external` map")
    seen: set[str] = set()
    for row in rows:
        name = row.get("name")
        if row.get("kind") not in KINDS:
            raise ManifestError(f"{name}: kind {row.get('kind')!r} not in {KINDS}")
        if not isinstance(row.get("required"), bool):
            raise ManifestError(f"{name}: `required` must be true or false")
        if row.get("required") and row.get("default") is not None:
            raise ManifestError(f"{name}: a required row carries no default")
        # A default is printed beside a pinned value, so an opaque kind with one
        # would put its value in the report.
        if row.get("kind") in OPAQUE and row.get("default") is not None:
            raise ManifestError(f"{name}: a {row['kind']} row carries no default")
        unless = row.get("required_unless", {})
        # String values only: the environment holds strings, so a JSON 0
        # would never match and the requirement would silently never lift.
        if (
            not isinstance(unless, dict)
            or (unless and not row.get("required"))
            or not all(isinstance(v, str) for v in unless.values())
        ):
            raise ManifestError(
                f'{name}: `required_unless` is a {{NAME: "value"}} map on a '
                "required row"
            )
        if name in seen:
            raise ManifestError(f"{name}: listed twice")
        seen.add(name)
    for row in rows:
        for key in row.get("required_unless", {}):
            if key not in seen:
                raise ManifestError(
                    f"{row['name']}: `required_unless` names {key}, not a row"
                )
    return data


def scan(root: Path) -> Reads:
    reads = Reads()
    for rel in SHELL_FILES:
        path = root / rel
        if not path.exists():
            continue
        for n, line in enumerate(path.read_text().splitlines(), 1):
            if line.lstrip().startswith("#"):
                continue
            for name in NAME_RE.findall(line):
                reads.add(name, f"{rel}:{n}")
            for name, value in SHELL_DEFAULT_RE.findall(line):
                reads.default(name, value)
    for directory in PYTHON_DIRS:
        for path in sorted((root / directory).glob("*.py")):
            rel = path.relative_to(root)
            for n, line in enumerate(path.read_text().splitlines(), 1):
                for name in QUOTED_RE.findall(line):
                    reads.add(name, f"{rel}:{n}")
                for name, value in TOOL_DEFAULT_RE.findall(line):
                    reads.default(name, value)
    return reads


def static_problems(manifest: dict, reads: Reads, example: str = "") -> list[str]:
    rows = {r["name"]: r for r in manifest["variables"]}
    external = manifest["external"]
    problems: list[str] = []
    named = set(NAME_RE.findall(example))
    for name, row in sorted(rows.items()):
        if row["kind"] not in NOT_CONFIG and name not in named:
            problems.append(f"example gap: {name} is not in {EXAMPLE}")
    for name in sorted(reads.sites):
        if name not in rows and name not in external:
            problems.append(f"undocumented read: {name} ({reads.sites[name][0]})")
    for name in sorted({*rows, *external}):
        if name not in reads.sites:
            problems.append(f"stale row: {name} is read nowhere")
    for name, row in sorted(rows.items()):
        # Only a knob or toggle has a default to drift; an empty fallback is
        # "unset" spelled out, and another kind's fallback is a display word
        # (`${DS_MODEL_BACKGROUND:-<unset>}`), not a value the code runs on.
        if row["kind"] not in DEFAULTED:
            continue
        code = reads.defaults.get(name, set()) - {""}
        want = row.get("default")
        if want is None and code:
            problems.append(
                f"default drift: {name} has none in the manifest, "
                f"the code applies {sorted(code)}"
            )
        elif want is not None and code and code != {str(want)}:
            problems.append(
                f"default drift: {name} manifest {want!r}, code {sorted(code)}"
            )
    return problems


def live_rows(manifest: dict, env) -> tuple[list[str], list[str], list[str]]:
    """``(missing, lines, unknown)``: the required rows unset, one line per
    config row, and the prefixed names in `env` the manifest does not know."""
    missing: list[str] = []
    lines: list[str] = []
    known = {r["name"] for r in manifest["variables"]} | set(manifest["external"])
    for row in manifest["variables"]:
        name, kind = row["name"], row["kind"]
        if kind in NOT_CONFIG:
            continue
        value = env.get(name, "")
        default = row.get("default")
        # Required unless another variable switches its consumer off — the
        # cargo pair while `DS_CARGO_POOL=0`, as the launch refusal reads it.
        unless = row.get("required_unless", {})
        required = row["required"] and not any(
            env.get(k, "") == v for k, v in unless.items()
        )
        if not value:
            if required:
                missing.append(name)
                lines.append(f"MISSING {name}")
            elif default is not None:
                lines.append(f"{name} at default ({default})")
            else:
                lines.append(f"{name} unset")
        elif kind in OPAQUE:
            # Checked first: an opaque value is never printed, pinned or not.
            lines.append(f"{name} set")
        elif default is not None and value != str(default):
            lines.append(f"pinned: {name}={value} (default {default})")
        else:
            lines.append(f"{name}={value}")
    # CARGO_* is left out here: cargo reads many of its own (CARGO_INCREMENTAL,
    # CARGO_PROFILE_*), which an operator may export for reasons of their own.
    unknown = sorted(
        k
        for k in env
        if NAME_RE.fullmatch(k)
        and not k.startswith("CARGO_")
        and k not in known
        and env.get(k)
    )
    return missing, lines, unknown


def live_summary(missing: list[str], lines: list[str], unknown: list[str]) -> str:
    """One report line, leading with any missing required row."""
    pinned = sum(1 for line in lines if line.startswith("pinned:"))
    unset = sum(1 for line in lines if line.endswith(" unset"))
    parts = []
    if missing:
        parts.append(f"{len(missing)} required missing ({', '.join(missing)})")
    settled = len(lines) - len(missing) - unset
    parts.append(f"{settled} of {len(lines)} rows set or defaulted")
    if pinned:
        parts.append(f"{pinned} pinned")
    if unknown:
        parts.append(f"{len(unknown)} unknown ({', '.join(unknown)})")
    return "config: " + "; ".join(parts)


def main(argv: list[str] | None = None, env=None, root: Path = REPO) -> int:
    p = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    mode = p.add_mutually_exclusive_group(required=True)
    mode.add_argument("--static", action="store_true", help="manifest vs code")
    mode.add_argument("--live", action="store_true", help="manifest vs env")
    mode.add_argument("--names", action="store_true", help="the config rows")
    p.add_argument("--json", action="store_true", help="--live as JSON")
    args = p.parse_args(argv)
    try:
        manifest = load_manifest(root / MANIFEST)
    except ManifestError as e:
        print(f"runtime_config_check: {e}", file=sys.stderr)
        return 2
    if args.names:
        for row in manifest["variables"]:
            if row["kind"] not in NOT_CONFIG:
                print(row["name"])
        return 0
    if args.static:
        example = root / EXAMPLE
        text = example.read_text() if example.exists() else ""
        problems = static_problems(manifest, scan(root), text)
        for line in problems:
            print(line)
        if not problems:
            print(
                f"runtime config manifest: {len(manifest['variables'])} rows, in sync"
            )
        return 1 if problems else 0
    missing, lines, unknown = live_rows(manifest, os.environ if env is None else env)
    line = live_summary(missing, lines, unknown)
    if args.json:
        out = {"line": line, "missing": missing, "rows": lines, "unknown": unknown}
        print(json.dumps(out))
    else:
        print(line)
        for row in lines:
            print(f"  {row}")
        for name in unknown:
            print(f"  unknown: {name}")
    return 1 if missing else 0


if __name__ == "__main__":
    sys.exit(main())
