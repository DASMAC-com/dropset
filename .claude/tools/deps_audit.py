#!/usr/bin/env python3
"""Dependency advisories across the workspace — ``cargo audit`` over
``Cargo.lock`` and ``pnpm audit --prod`` over ``pnpm-lock.yaml`` — reduced to a
ranked summary. ``make deps-audit`` runs it by hand; the daily upkeep pass runs
it for its one ``deps`` line.

Usage (from the repo root):

    python3 .claude/tools/deps_audit.py [--full] [--json] [--top N] [--state PATH]
                                        [--install]
    python3 .claude/tools/deps_audit.py check-ignores

**Default output** is the count by severity and the top ``--top`` (5)
advisories, each with its crate or package, the locked version and the fixed
range. ``--full`` lists every advisory, with the dependency path pnpm reports;
``--json`` prints the structured twin, which carries the upkeep ``line``.

**Severity.** pnpm's own (critical / high / moderate / low / info). A cargo
vulnerability carries a CVSS vector, not a rating, so its rating is the CVSS 3.x
base score's band; a vector this tool cannot score reads ``unrated``. cargo's
unmaintained / unsound / yanked warnings rank last, as ``warning``.

**Standing exceptions.** ``cfg/audit.toml`` (cargo-audit's own format,
``[advisories] ignore = [...]``, RUSTSEC ids) and ``cfg/pnpm-audit.toml`` (the
same shape, GHSA ids). Every id carries a trailing ``# reason`` comment on its
own line; ``check-ignores`` exits 1 naming any that does not, and the tests run
it over the committed files. An ignored advisory still counts as ``ignored``,
and an ignore matching nothing is reported ``stale`` so the list shrinks when a
fix lands. The tool applies both lists itself, so the two audits share one rule.

**Delta.** ``--state PATH`` reads the previous run's advisory ids from PATH and
reports what is new and what resolved since — an id that merely joined an
ignore list is not counted resolved — then rewrites PATH, but only when both
audits ran. A failed scanner never resets the baseline and reports no delta,
since half the ids would read as resolved. A scanner fails when it exits
outside 0 / 1 **or** prints JSON without its findings key: pnpm reports a
registry failure as exit 1 with an ``error`` object, which would otherwise read
as an all-clear. The upkeep pass keeps PATH untracked beside its own stamp.

**Install on demand, never unattended.** ``cargo-audit`` is not a toolchain
prerequisite. Under ``--install`` (which ``make deps-audit`` passes) an absent
``cargo audit`` is built once at the pinned ``CARGO_AUDIT_VERSION`` with
``--locked``; without it — the upkeep pass — absence is a failed-scanner line
naming the make target, so no crate build script runs unwatched.

Exit status: 0 no unignored advisory, 1 advisories found, 2 a scanner failed
(the output names it; the other scanner's findings still print).

Stdlib only; a skill-tool under ``.claude/tools/``, deliberately **not** a Cargo
workspace member. Tests live in ``tests/test_deps_audit.py``.
"""

from __future__ import annotations

import argparse
import json
import math
import re
import subprocess
import sys
from datetime import datetime, timezone
from pathlib import Path
from typing import Callable

REPO_ROOT = Path(__file__).resolve().parents[2]
CARGO_IGNORES = Path("cfg/audit.toml")
PNPM_IGNORES = Path("cfg/pnpm-audit.toml")

# Ranked most-severe first; anything else sorts after `warning`.
SEVERITIES = ("critical", "high", "moderate", "low", "info", "unrated", "warning")
TOP = 5

# The cargo-audit release `--install` builds; bump it deliberately, by hand.
CARGO_AUDIT_VERSION = "0.22.2"

# cargo-audit is built from source on first use, which takes minutes.
TIMEOUT_INSTALL = 900
TIMEOUT_AUDIT = 300

Runner = Callable[[list[str], float, "str | None"], "tuple[int, str, str]"]


def real_run(cmd: list[str], timeout: float, cwd: str | None = None):
    """``(rc, stdout, stderr)``; a timeout or a missing binary is an rc."""
    try:
        proc = subprocess.run(
            cmd, capture_output=True, text=True, timeout=timeout, cwd=cwd, check=False
        )
    except subprocess.TimeoutExpired:
        return 124, "", f"timed out after {timeout:g}s"
    except OSError as e:
        return 127, "", str(e)
    return proc.returncode, proc.stdout, proc.stderr


def first_line(text: str) -> str:
    return (text.strip().splitlines() or [""])[0][:160]


# --- ignore lists ------------------------------------------------------------

_IGNORE_ENTRY = re.compile(r'^\s*"([^"]+)"\s*,?\s*(?:#\s*(.*?))?\s*$')


def parse_ignores(text: str) -> tuple[dict[str, str], list[str]]:
    """``({id: reason}, [ids with no reason])`` from an ``ignore = [...]``
    array written one id per line, each with a trailing ``# reason``."""
    reasons: dict[str, str] = {}
    unreasoned: list[str] = []
    inside = False
    for line in text.splitlines():
        stripped = line.strip()
        if not inside:
            if re.match(r"^ignore\s*=\s*\[", stripped):
                inside = True
                stripped = stripped.split("[", 1)[1].strip()
            else:
                continue
        if stripped.startswith("]"):
            inside = False
            continue
        if not stripped or stripped.startswith("#"):
            continue
        match = _IGNORE_ENTRY.match(stripped)
        if not match:
            # An inline `ignore = ["A", "B"]` hides which reason is whose.
            unreasoned.append(stripped)
            continue
        ident, reason = match.group(1), (match.group(2) or "").strip()
        if reason:
            reasons[ident] = reason
        else:
            unreasoned.append(ident)
    return reasons, unreasoned


def load_ignores(root: Path, rel: Path) -> tuple[dict[str, str], list[str]]:
    try:
        return parse_ignores((root / rel).read_text("utf-8"))
    except FileNotFoundError:
        return {}, []


# --- CVSS --------------------------------------------------------------------

_CVSS3 = {
    "AV": {"N": 0.85, "A": 0.62, "L": 0.55, "P": 0.2},
    "AC": {"L": 0.77, "H": 0.44},
    "UI": {"N": 0.85, "R": 0.62},
    "CIA": {"H": 0.56, "L": 0.22, "N": 0.0},
}


def _roundup(value: float) -> float:
    scaled = round(value * 100000)
    if scaled % 10000 == 0:
        return scaled / 100000.0
    return (math.floor(scaled / 10000) + 1) / 10.0


def cvss3_score(vector: str) -> float | None:
    """The CVSS 3.x base score of ``vector``, or None when it is not one."""
    parts = vector.split("/")
    if not parts or not parts[0].startswith("CVSS:3"):
        return None
    try:
        m = dict(p.split(":", 1) for p in parts[1:])
        changed = m["S"] == "C"
        pr = {"N": 0.85, "L": 0.68 if changed else 0.62, "H": 0.5 if changed else 0.27}
        cia = [_CVSS3["CIA"][m[k]] for k in ("C", "I", "A")]
        exploit = (
            8.22
            * _CVSS3["AV"][m["AV"]]
            * _CVSS3["AC"][m["AC"]]
            * pr[m["PR"]]
            * _CVSS3["UI"][m["UI"]]
        )
    except (KeyError, ValueError):
        return None
    iss = 1 - (1 - cia[0]) * (1 - cia[1]) * (1 - cia[2])
    if changed:
        impact = 7.52 * (iss - 0.029) - 3.25 * (iss - 0.02) ** 15
    else:
        impact = 6.42 * iss
    if impact <= 0:
        return 0.0
    total = (impact + exploit) * (1.08 if changed else 1.0)
    return _roundup(min(total, 10.0))


def band(score: float | None) -> str:
    if score is None:
        return "unrated"
    if score >= 9.0:
        return "critical"
    if score >= 7.0:
        return "high"
    if score >= 4.0:
        return "moderate"
    if score > 0:
        return "low"
    return "info"


# --- parsing -----------------------------------------------------------------


def parse_cargo(data: dict) -> list[dict]:
    """Findings from ``cargo audit --json``: its vulnerabilities, then its
    unmaintained / unsound / yanked warnings."""
    found: list[dict] = []
    for vuln in (data.get("vulnerabilities") or {}).get("list") or []:
        adv = vuln.get("advisory") or {}
        pkg = vuln.get("package") or {}
        found.append(
            {
                "source": "cargo",
                "id": adv.get("id", "?"),
                "package": pkg.get("name", adv.get("package", "?")),
                "version": pkg.get("version", "?"),
                "severity": band(cvss3_score(adv.get("cvss") or "")),
                "fixed": ", ".join((vuln.get("versions") or {}).get("patched") or [])
                or "no fix",
                "title": adv.get("title", ""),
                "paths": [],
            }
        )
    for kind, warnings in (data.get("warnings") or {}).items():
        for warn in warnings or []:
            adv = warn.get("advisory") or {}
            pkg = warn.get("package") or {}
            name, version = pkg.get("name", "?"), pkg.get("version", "?")
            found.append(
                {
                    "source": "cargo",
                    # A yanked crate has no advisory, so no id of its own.
                    "id": adv.get("id") or f"{kind}:{name}@{version}",
                    "package": name,
                    "version": version,
                    "severity": "warning",
                    "fixed": ", ".join(
                        (warn.get("versions") or {}).get("patched") or []
                    )
                    or "no fix",
                    "title": f"{kind}: {adv.get('title') or 'yanked from crates.io'}",
                    "paths": [],
                }
            )
    return found


def parse_pnpm(data: dict) -> list[dict]:
    """Findings from ``pnpm audit --json``, one per advisory, keyed by its GHSA
    id (stable across registries, unlike npm's numeric key)."""
    found: list[dict] = []
    for key, adv in (data.get("advisories") or {}).items():
        findings = adv.get("findings") or []
        versions = sorted({f.get("version", "?") for f in findings})
        paths = [p for f in findings for p in f.get("paths") or []]
        found.append(
            {
                "source": "pnpm",
                "id": adv.get("github_advisory_id") or str(key),
                "package": adv.get("module_name", "?"),
                "version": ", ".join(versions) or "?",
                "severity": adv.get("severity") or "unrated",
                "fixed": adv.get("patched_versions") or "no fix",
                "title": adv.get("title", ""),
                "paths": paths,
            }
        )
    return found


def rank(findings: list[dict]) -> list[dict]:
    order = {s: i for i, s in enumerate(SEVERITIES)}
    return sorted(
        findings,
        key=lambda f: (order.get(f["severity"], len(SEVERITIES)), f["id"]),
    )


# --- running -----------------------------------------------------------------


def _json_of(out: str) -> dict:
    data = json.loads(out)
    if not isinstance(data, dict):
        raise ValueError("audit printed JSON that is not an object")
    return data


def _findings_of(out: str, key: str) -> dict:
    """The audit's JSON, refused unless it carries its findings ``key``: exit 1
    is also how pnpm reports its OWN failure, as ``{"error": ...}``, and reading
    that as zero findings would be a silent all-clear."""
    data = _json_of(out)
    error = data.get("error")
    if error is not None or not isinstance(data.get(key), dict):
        detail = error.get("message") if isinstance(error, dict) else error
        reason = f": {first_line(str(detail))}" if detail is not None else ""
        raise ValueError(f"no {key!r} in the audit's JSON{reason}")
    return data


def audit_cargo(run: Runner, root: Path, install: bool = False) -> dict:
    """``cargo audit --json``; with ``install``, building the pinned cargo-audit
    first if it is absent. Exit 1 means advisories found, not failure."""
    rc, _out, _err = run(["cargo", "audit", "--version"], 60, str(root))
    if rc != 0:
        if not install:
            raise RuntimeError("cargo-audit absent; `make deps-audit` installs it")
        rc, _out, err = run(
            [
                "cargo",
                "install",
                "cargo-audit",
                "--version",
                CARGO_AUDIT_VERSION,
                "--locked",
            ],
            TIMEOUT_INSTALL,
            str(root),
        )
        if rc != 0:
            raise RuntimeError(f"cargo-audit install failed: {first_line(err)}")
    rc, out, err = run(["cargo", "audit", "--json"], TIMEOUT_AUDIT, str(root))
    if rc not in (0, 1):
        raise RuntimeError(f"cargo audit exited {rc}: {first_line(err)}")
    return _findings_of(out, "vulnerabilities")


def audit_pnpm(run: Runner, root: Path, install: bool = False) -> dict:
    """``pnpm audit --prod --json`` at the workspace root, which covers every
    workspace package. Exit 1 means advisories found — or pnpm's own failure,
    which ``_findings_of`` tells apart."""
    rc, out, err = run(["pnpm", "audit", "--prod", "--json"], TIMEOUT_AUDIT, str(root))
    if rc not in (0, 1):
        raise RuntimeError(f"pnpm audit exited {rc}: {first_line(err or out)}")
    return _findings_of(out, "advisories")


# --- summary -----------------------------------------------------------------


def read_state(path: Path) -> list[str] | None:
    try:
        ids = json.loads(path.read_text("utf-8")).get("ids")
    except (OSError, ValueError, AttributeError):
        return None
    if not isinstance(ids, list):
        return None
    return [i for i in ids if isinstance(i, str)]


def write_state(path: Path, ids: list[str], now: datetime) -> None:
    path.write_text(
        json.dumps({"ran_at": now.isoformat(), "ids": ids}) + "\n", encoding="utf-8"
    )


def summarize(
    findings: list[dict],
    ignores: dict[str, str],
    previous: list[str] | None,
    errors: list[str],
    top: int = TOP,
) -> dict:
    live = rank([f for f in findings if f["id"] not in ignores])
    seen = {f["id"] for f in findings}
    counts = {s: 0 for s in SEVERITIES}
    for f in live:
        counts[f["severity"]] = counts.get(f["severity"], 0) + 1
    ids = sorted({f["id"] for f in live})
    # No delta over a partial run: the failed scanner's ids would all read as
    # resolved. An id that only joined an ignore list did not resolve either.
    delta = None
    if previous is not None and not errors:
        delta = {
            "new": sorted(set(ids) - set(previous)),
            "resolved": sorted(set(previous) - set(ids) - set(ignores)),
        }
    summary = {
        "total": len(live),
        "counts": {s: n for s, n in counts.items() if n},
        "top": live[:top],
        "findings": live,
        "ids": ids,
        "ignored": sorted(i for i in ignores if i in seen),
        # A failed scanner matched none of its own ignores; calling them stale
        # would ask for exceptions that are still needed to be deleted.
        "stale_ignores": [] if errors else sorted(i for i in ignores if i not in seen),
        "delta": delta,
        "errors": errors,
    }
    summary["line"] = upkeep_line(summary)
    return summary


def counts_text(counts: dict[str, int]) -> str:
    return ", ".join(f"{n} {s}" for s, n in counts.items()) or "none"


def upkeep_line(summary: dict) -> str:
    """The upkeep report's one line: advisories by severity and the delta."""
    text = f"{summary['total']} advisories ({counts_text(summary['counts'])})"
    delta = summary["delta"]
    if summary["errors"]:
        text += ", no delta over a partial run"
    elif delta is None:
        text += ", no previous run"
    else:
        text += (
            f", {len(delta['new'])} new / {len(delta['resolved'])} resolved since"
            " the last run"
        )
        if delta["new"]:
            text += f" (new: {', '.join(delta['new'][:3])})"
    if summary["ignored"]:
        text += f"; {len(summary['ignored'])} ignored"
    if summary["stale_ignores"]:
        text += f"; stale ignores: {', '.join(summary['stale_ignores'])}"
    if summary["errors"]:
        text += f"; FAILED: {'; '.join(summary['errors'])}"
    return text


def finding_line(f: dict) -> str:
    return (
        f"{f['severity']:<9} {f['id']:<20} {f['package']} {f['version']}"
        f" -> {f['fixed']}  ({f['source']}) {f['title']}"
    )


def render(summary: dict, full: bool) -> str:
    lines = [f"deps-audit: {summary['line']}"]
    for f in summary["findings"] if full else summary["top"]:
        lines.append(f"  {finding_line(f)}")
        if full and f["paths"]:
            lines.append(f"    via {f['paths'][0]}")
    hidden = summary["total"] - len(summary["top"])
    if not full and hidden > 0:
        lines.append(f"  … {hidden} more (--full lists them)")
    return "\n".join(lines)


# --- CLI ---------------------------------------------------------------------


def check_ignores(root: Path) -> int:
    bad = []
    for rel in (CARGO_IGNORES, PNPM_IGNORES):
        _reasons, unreasoned = load_ignores(root, rel)
        bad += [f"{rel}: {ident}" for ident in unreasoned]
    for line in bad:
        print(f"ignore without a reason: {line}")
    return 1 if bad else 0


def run_audit(
    root: Path,
    run: Runner = real_run,
    state: Path | None = None,
    now: datetime | None = None,
    top: int = TOP,
    install: bool = False,
) -> dict:
    findings: list[dict] = []
    errors: list[str] = []
    for name, audit, parse in (
        ("cargo", audit_cargo, parse_cargo),
        ("pnpm", audit_pnpm, parse_pnpm),
    ):
        try:
            findings += parse(audit(run, root, install))
        # An unexpected JSON shape is one scanner's failure, not the tool's.
        except (RuntimeError, ValueError, AttributeError, TypeError) as e:
            errors.append(f"{name}: {first_line(str(e))}")
    ignores = {
        **load_ignores(root, CARGO_IGNORES)[0],
        **load_ignores(root, PNPM_IGNORES)[0],
    }
    previous = read_state(state) if state else None
    summary = summarize(findings, ignores, previous, errors, top)
    if state and not errors:
        write_state(state, summary["ids"], now or datetime.now(timezone.utc))
    return summary


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("command", nargs="?", choices=("check-ignores",))
    parser.add_argument("--full", action="store_true", help="list every advisory")
    parser.add_argument("--json", action="store_true", help="print the JSON twin")
    parser.add_argument("--top", type=int, default=TOP)
    parser.add_argument("--state", type=Path, help="previous-run ids, for the delta")
    parser.add_argument(
        "--install", action="store_true", help="build cargo-audit if it is absent"
    )
    parser.add_argument("--root", type=Path, default=REPO_ROOT)
    args = parser.parse_args(argv)
    if args.command == "check-ignores":
        return check_ignores(args.root)
    summary = run_audit(args.root, state=args.state, top=args.top, install=args.install)
    if args.json:
        if not args.full:
            summary.pop("findings")
        print(json.dumps(summary, indent=2))
    else:
        print(render(summary, args.full))
    if summary["errors"]:
        return 2
    return 1 if summary["total"] else 0


if __name__ == "__main__":
    sys.exit(main())
