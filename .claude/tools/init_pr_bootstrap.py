#!/usr/bin/env python3
"""``init-pr`` bootstrap in one run — every deterministic act of the skill's
opening, performed in order, ending in one compact manifest.

The skill used to spend a turn per act: the credential check, the merged-PR
pre-check, the branch helper, the fetch, the frontend install, the rename, the
rebase, the bootstrap commit, the push, the draft PR, the notification mute and
the Linear In Progress move. None of those turns held a judgment, so they are
one tool call now, and the session's first substantive turn starts from a
finished bootstrap.

What stays the skill's is only what needs judgment, and the tool **stops before
any mutation** to hand it back:

* a dead ``gh`` credential (``auth``);
* a **merged** PR already on the issue (``merged_check``) — a contradiction the
  operator adjudicates; ``--allow-merged`` proceeds once they have;
* an invalid tag (``branch``), read from `init_pr_branch.py`, which this tool
  drives rather than duplicates.

A signer that cannot sign is **advisory, never a stop** (``docs/conventions/
commits-and-prs.md`` → "Signing defers to the merge gate"): a verdict of
``external-signer-missing`` / ``agent-locked``, or a ``LOCKED`` answer from
`signing_probe.py` (which proves the signer unlocked, not merely wired), adds a
note and runs the rebase and the bootstrap commit with ``--no-gpg-sign``. A
signed commit that fails anyway falls back to an unsigned one the same way.
``facts.signing_deferred`` says whether **this run** deferred — a re-run that
finds an unsigned bootstrap commit already made reads ``false`` — so the
branch-wide answer is `resign_branch.py count`, and `resign_branch.py` signs
the branch at the merge gate.

After the first mutation a failure still stops the run, and the manifest names
what completed and what did not. Every step is **idempotent** — the rename and
rebase are no-ops when done, the commit is skipped when the branch already
carries it, an open PR on the branch is reused — so the remedy for any stop is
to fix the cause and re-run the same command, which resumes rather than redoes.

Best-effort steps never stop the run, matching the skill's old prose: the
frontend install (``install``), the notification mute (``mute``), the Linear
state move (``linear_state``) and the spec spill (``spec``). Each reports
``failed`` with its error instead.

Usage::

    python3 .claude/tools/init_pr_bootstrap.py --tag 1386 \
      --spec-out <scratchpad>/spec.md

Prints JSON::

    {
      "tag": "eng-1386",
      "branch": "eng-1386",
      "pr": {"number": 596, "url": "https://github.com/…/pull/596"},
      "stopped_at": null,        // the step that stopped the run, or null
      "steps": [{"step": "auth", "status": "ok", "detail": "…"}, …],
      "remaining": [],           // steps never reached because of the stop
      "notes": ["…"],            // e.g. a missing `notifications` scope
      "facts": {"program_so": "absent", "signing": "external-signer",
                "signing_deferred": false, …},
      "spec_path": "<scratchpad>/spec.md"
    }

``status`` is ``ok`` / ``skipped`` (already done, or not needed) / ``failed``
(best-effort, run continued) / ``stopped`` (the run ended here). Exit 0 when
``stopped_at`` is null, else 1.

The draft PR is opened with ``gh pr create`` rather than the GitHub MCP, a
documented carve-out (``docs/conventions/github-mcp.md``): an MCP call cannot
run inside a tool, and putting it back in the skill would restore the turn this
tool exists to remove.

Stdlib only. This is a Python skill-tool under ``.claude/tools/`` — deliberately
**not** a Cargo workspace member (see ``CLAUDE.md`` → "Skill tooling").
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
import tempfile
from collections.abc import Callable
from pathlib import Path

from init_pr_branch import normalize_tag
from signing_probe import TIMEOUT_SECONDS as PROBE_TIMEOUT

REPO = "DASMAC-com/dropset"
TOOLS = Path(__file__).resolve().parent

STEPS = (
    "auth",
    "merged_check",
    "branch",
    "fetch",
    "rename",
    "rebase",
    "install",
    "commit",
    "push",
    "pr",
    "mute",
    "linear_state",
    "spec",
)

# Steps whose failure is reported as `failed` and never stops the run.
BEST_EFFORT = frozenset({"install", "mute", "linear_state", "spec"})

# Signing verdicts from `init_pr_branch.py` that defer signing: the run goes on
# unsigned, with the remedy in a note.
SIGNING_DEFERRALS = {
    "external-signer-missing": "the configured signer resolves nowhere; "
    "reinstall it or fix gpg.ssh.program",
    "agent-locked": "the ssh agent holds no keys; unlock the 1Password app",
}

MUTE_QUERY = (
    "mutation($id: ID!) { updateSubscription(input: "
    "{ subscribableId: $id, state: IGNORED }) "
    "{ subscribable { viewerSubscription } } }"
)

PR_NUMBER_RE = re.compile(r"/pull/(\d+)")

Runner = Callable[[list[str], int], "tuple[int, str, str]"]


class Stop(Exception):
    """Raised by a step to end the run; the message is the step's detail."""


def default_runner(cmd: list[str], timeout: int) -> tuple[int, str, str]:
    """Run ``cmd``, returning ``(returncode, stdout, stderr)``; never raises."""
    try:
        done = subprocess.run(
            cmd,
            capture_output=True,
            text=True,
            errors="replace",
            timeout=timeout,
            check=False,
        )
    except (OSError, subprocess.TimeoutExpired) as exc:
        return 1, "", str(exc)
    return done.returncode, done.stdout, done.stderr


def _tail(text: str, limit: int = 300) -> str:
    text = text.strip()
    return text if len(text) <= limit else "…" + text[-limit:]


class Bootstrap:
    def __init__(
        self,
        tag: str,
        spec_out: str | None,
        allow_merged: bool,
        run: Runner = default_runner,
        log_dir: str | None = None,
    ):
        self.raw_tag = tag
        self.spec_out = spec_out
        self.allow_merged = allow_merged
        self.run = run
        self.log_dir = Path(log_dir or tempfile.gettempdir()) / "claude-init-pr"
        self.tag: str | None = None
        self.branch: str | None = None
        self.helper: dict = {}
        self.pr: dict | None = None
        self.notes: list[str] = []
        self.steps: list[dict] = []
        self.signing_deferred = False

    # -- plumbing -----------------------------------------------------------

    def _ok(self, cmd: list[str], timeout: int = 120) -> str:
        """Run a command that must succeed, raising `Stop` with its stderr."""
        code, out, err = self.run(cmd, timeout)
        if code != 0:
            raise Stop(f"`{' '.join(cmd[:3])}` failed: {_tail(err) or f'exit {code}'}")
        return out

    @property
    def scope(self) -> str:
        return (self.tag or "").upper()

    @property
    def subject(self) -> str:
        return f"chore({self.scope}): Bootstrap the worktree"

    def _defer_signing(self, reason: str) -> None:
        if not self.signing_deferred:
            self.notes.append(
                f"signing deferred ({reason}); committing unsigned — the merge "
                "gate signs the branch with resign_branch.py"
            )
        self.signing_deferred = True

    # -- steps --------------------------------------------------------------

    def step_auth(self) -> tuple[str, str]:
        code, out, err = self.run(["gh", "auth", "status"], 30)
        text = out + err
        if code != 0:
            raise Stop(f"no valid gh credential; re-authenticate ({_tail(text)})")
        if "notifications" not in text:
            self.notes.append(
                "gh token lacks the `notifications` scope, so the mute step will "
                "fail; one-time fix: gh auth refresh -h github.com -s notifications"
            )
        return "ok", "credential valid"

    def step_merged_check(self) -> tuple[str, str]:
        self.tag = normalize_tag(self.raw_tag)
        if self.tag is None:
            raise Stop(f"invalid tag {self.raw_tag!r}; want ### or eng-###")
        out = self._ok(
            [
                "gh", "pr", "list", "--repo", REPO, "--search", self.scope,
                "--state", "all", "--json", "number,title,state,headRefName,url",
            ]
        )  # fmt: skip
        # `--search` is full-text, so a PR that merely mentions the tag is a
        # hit; only one on this branch or scoped to this issue counts.
        prs = [
            p
            for p in json.loads(out or "[]")
            if p.get("headRefName") == self.tag
            or f"({self.scope})" in (p.get("title") or "")
        ]
        merged = [p for p in prs if p.get("state") == "MERGED"]
        closed = [p for p in prs if p.get("state") == "CLOSED"]
        for p in prs:
            if p.get("state") == "OPEN" and p.get("headRefName") == self.tag:
                self.pr = {"number": p["number"], "url": p.get("url")}
        if closed:
            self.notes.append(
                "closed-not-merged PR(s) on this issue: "
                + ", ".join(f"#{p['number']}" for p in closed)
            )
        if merged and not self.allow_merged:
            nums = ", ".join(f"#{p['number']}" for p in merged)
            raise Stop(
                f"{self.scope} already merged as {nums}; the issue looks "
                "mis-stated or mis-queued — ask, then re-run with --allow-merged "
                "only for genuine follow-up scope"
            )
        if merged:
            return "ok", "merged PR(s) present; proceeding under --allow-merged"
        return "ok", f"{len(prs)} PR(s) found, none merged"

    def step_branch(self) -> tuple[str, str]:
        code, out, err = self.run(
            [
                sys.executable, str(TOOLS / "init_pr_branch.py"),
                "--tag", self.raw_tag, "--link-env",
            ],
            60,
        )  # fmt: skip
        try:
            self.helper = json.loads(out)
        except json.JSONDecodeError:
            raise Stop(f"init_pr_branch.py gave no JSON: {_tail(err)}")
        if not isinstance(self.helper, dict):
            self.helper = {}
            raise Stop(f"init_pr_branch.py gave no JSON object: {_tail(out)}")
        if not self.helper.get("tag_valid"):
            raise Stop(f"invalid tag {self.raw_tag!r}; want ### or eng-###")
        signing = self.helper.get("signing")
        if signing in SIGNING_DEFERRALS:
            self._defer_signing(f"signing is {signing}: {SIGNING_DEFERRALS[signing]}")
        else:
            # The verdict proves the signer is wired, not unlocked; a dangling
            # signed commit proves it before the rebase replays anything.
            # Outlast the probe's own unlock window (one per git call it
            # makes), so a slow unlock reports its LOCKED line, not our timeout.
            code, out, err = self.run(
                [sys.executable, str(TOOLS / "signing_probe.py")],
                2 * PROBE_TIMEOUT + 20,
            )
            if code == 1 and "LOCKED" in err + out:
                self._defer_signing(_tail(err or out, 200))
            elif code != 0:
                # Exit 2 is git failing, not the signer; any other non-zero
                # without the LOCKED line is the probe crashing or hanging.
                raise Stop(_tail(err or out) or f"signing_probe.py exit {code}")
        self.branch = self.helper.get("normalized_branch")
        if not self.branch:
            raise Stop("init_pr_branch.py reported no normalized_branch")
        for key in ("env_link", "secrets_env_link"):
            if self.helper.get(key) == "failed":
                self.notes.append(f"{key} failed; copy that file by hand")
        return "ok", f"signing {signing}"

    def step_fetch(self) -> tuple[str, str]:
        self._ok(["git", "fetch", "origin", "main"])
        return "ok", "origin/main updated"

    def step_rename(self) -> tuple[str, str]:
        if not self.helper.get("rename_needed"):
            return "skipped", "branch already normalized"
        current = self.helper["current_branch"]
        self._ok(["git", "branch", "-m", current, self.branch])
        return "ok", f"{current} -> {self.branch}"

    def step_rebase(self) -> tuple[str, str]:
        cmd = ["git", "rebase", "origin/main"]
        if self.signing_deferred:
            cmd.append("--no-gpg-sign")
        code, out, err = self.run(cmd, 120)
        if code != 0:
            self.run(["git", "rebase", "--abort"], 60)
            raise Stop(
                "rebase onto origin/main failed (a conflict, or a dirty tree) and "
                f"any rebase in progress was aborted; resolve by hand, never here "
                f"({_tail(err or out)})"
            )
        return "ok", _tail(out, 120) or "rebased"

    def step_install(self) -> tuple[str, str]:
        if self.helper.get("frontend_node_modules") != "absent":
            return "skipped", f"node_modules {self.helper.get('frontend_node_modules')}"
        # The helper resolves the `npx --yes pnpm@<pin>` fallback when `pnpm`
        # is off PATH; an older helper without the field means plain `pnpm`.
        pnpm = self.helper.get("pnpm_command", "pnpm")
        if not pnpm:
            return "failed", "neither pnpm nor npx is on PATH"
        code, out, err = self.run([*pnpm.split(), "--dir", "frontend", "install"], 900)
        self.log_dir.mkdir(mode=0o700, parents=True, exist_ok=True)
        log = self.log_dir / f"pnpm-install-{self.tag}.log"
        log.write_text(out + err, encoding="utf-8")
        via = "" if pnpm == "pnpm" else f" via `{pnpm}`"
        if code != 0:
            return "failed", f"pnpm install{via} exit {code}; log: {log}"
        return "ok", f"installed{via}; log: {log}"

    def step_commit(self) -> tuple[str, str]:
        out = self._ok(["git", "log", "--format=%s", "origin/main..HEAD"])
        if self.subject in out.splitlines():
            return "skipped", "bootstrap commit already on the branch"
        commit = ["git", "commit", "--allow-empty", "-m", self.subject]
        signed_err = None
        if not self.signing_deferred:
            code, _, signed_err = self.run([*commit[:3], "-S", *commit[3:]], 120)
            if code == 0:
                return "ok", self.subject
        code, _, err = self.run([*commit[:3], "--no-gpg-sign", *commit[3:]], 120)
        if code != 0:
            raise Stop(
                f"commit failed even unsigned — nothing is pushed yet ({_tail(err)})"
            )
        if signed_err is not None:
            # Only signing separates the two attempts, so the signed one's
            # failure was the signer (`failed to fill whole buffer` is a
            # locked 1Password app).
            self._defer_signing(f"signed commit failed: {_tail(signed_err, 160)}")
        return "ok", f"{self.subject} (unsigned)"

    def step_push(self) -> tuple[str, str]:
        code, _, err = self.run(["git", "push", "-u", "origin", self.branch], 120)
        if code == 0:
            return "ok", f"origin/{self.branch}"
        # Rejected. On a resumed run the rebase may have replayed an
        # already-pushed bootstrap commit onto a newer main; replace the remote
        # branch only when that commit is ALL it carries, with the lease pinned
        # to the sha just read — never a bare lease, whose shared tracking ref
        # may predate another session's push.
        out = self._ok(["git", "ls-remote", "origin", f"refs/heads/{self.branch}"])
        sha = out.split()[0] if out.strip() else ""
        if not sha:
            raise Stop(f"push rejected: {_tail(err) or f'exit {code}'}")
        self._ok(["git", "fetch", "origin", self.branch])
        theirs = self._ok(["git", "log", "--format=%s", f"origin/main..{sha}"])
        if any(s != self.subject for s in theirs.splitlines()):
            raise Stop(
                f"origin/{self.branch} carries commits beyond the bootstrap "
                "commit; reconcile by hand, never force here"
            )
        self._ok(
            [
                "git", "push", f"--force-with-lease={self.branch}:{sha}",
                "-u", "origin", self.branch,
            ]
        )  # fmt: skip
        return "ok", f"origin/{self.branch} (replaced its rebased bootstrap commit)"

    def step_pr(self) -> tuple[str, str]:
        if not self.pr:
            # Authoritative by head ref: the search index behind
            # `merged_check` can lag a PR created moments ago.
            out = self._ok(
                [
                    "gh", "pr", "list", "--repo", REPO, "--head", self.branch,
                    "--state", "open", "--json", "number,url,headRefName,state",
                ]
            )  # fmt: skip
            for p in json.loads(out or "[]"):
                if p.get("state") == "OPEN" and p.get("headRefName") == self.branch:
                    self.pr = {"number": p["number"], "url": p.get("url")}
        if self.pr:
            return "skipped", f"reusing open PR #{self.pr['number']}"
        out = self._ok(
            [
                "gh", "pr", "create", "--repo", REPO, "--draft",
                "--base", "main", "--head", self.branch,
                "--title", self.subject, "--body", "",
            ]
        )  # fmt: skip
        url = out.strip().splitlines()[-1] if out.strip() else ""
        m = PR_NUMBER_RE.search(url)
        if not m:
            raise Stop(f"gh pr create printed no PR URL: {_tail(out)}")
        self.pr = {"number": int(m.group(1)), "url": url}
        return "ok", url

    def step_mute(self) -> tuple[str, str]:
        number = str(self.pr["number"])
        code, out, err = self.run(
            ["gh", "pr", "view", number, "--repo", REPO, "--json", "id"], 60
        )
        if code != 0:
            return "failed", f"node-id read failed: {_tail(err)}"
        node = json.loads(out).get("id", "")
        code, out, err = self.run(
            ["gh", "api", "graphql", "-F", f"id={node}", "-f", f"query={MUTE_QUERY}"],
            60,
        )
        if code != 0 or "UNSUBSCRIBED" not in out:
            return "failed", _tail(err or out) or f"exit {code}"
        return "ok", "UNSUBSCRIBED"

    def step_linear_state(self) -> tuple[str, str]:
        code, out, err = self.run(
            [
                sys.executable, str(TOOLS / "linear_patch.py"),
                "state", self.scope, "--state", "In Progress",
            ],
            60,
        )  # fmt: skip
        if code != 0:
            return "failed", _tail(err or out) or f"exit {code}"
        return "ok", _tail(out, 160)

    def step_spec(self) -> tuple[str, str]:
        if not self.spec_out:
            return "skipped", "no --spec-out"
        code, out, err = self.run(
            [
                sys.executable, str(TOOLS / "linear_patch.py"),
                "read", self.scope, "--out", self.spec_out,
            ],
            60,
        )  # fmt: skip
        if code != 0:
            return "failed", _tail(err or out) or f"exit {code}"
        return "ok", _tail(out, 160)

    # -- driver -------------------------------------------------------------

    def execute(self) -> dict:
        stopped_at = None
        for name in STEPS:
            try:
                status, detail = getattr(self, f"step_{name}")()
            except Stop as exc:
                status, detail, stopped_at = "stopped", str(exc), name
            except Exception as exc:  # noqa: BLE001 — the manifest must print
                # An unexpected reply (non-JSON, a write error) is reported, not
                # raised: a traceback would lose the record of what completed.
                detail = f"{type(exc).__name__}: {exc}"
                if name in BEST_EFFORT:
                    status = "failed"
                else:
                    status, stopped_at = "stopped", name
            self.steps.append({"step": name, "status": status, "detail": detail})
            if stopped_at:
                break
        done = {s["step"] for s in self.steps}
        facts = {
            k: self.helper.get(k)
            for k in (
                "env_link",
                "secrets_env_link",
                "frontend_node_modules",
                "pnpm",
                "pnpm_command",
                "program_so",
                "signing",
            )
        }
        facts["signing_deferred"] = self.signing_deferred
        return {
            "tag": self.tag,
            "branch": self.branch,
            "pr": self.pr,
            "stopped_at": stopped_at,
            "steps": self.steps,
            "remaining": [s for s in STEPS if s not in done],
            "notes": self.notes,
            "facts": facts,
            "spec_path": self.spec_out
            if any(s["step"] == "spec" and s["status"] == "ok" for s in self.steps)
            else None,
        }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        prog="init_pr_bootstrap.py",
        description="Run the whole init-pr bootstrap and print one manifest.",
    )
    parser.add_argument("--tag", required=True, help="### or eng-###")
    parser.add_argument(
        "--spec-out", help="write the Linear issue body here (zero-echo read)"
    )
    parser.add_argument(
        "--allow-merged",
        action="store_true",
        help="proceed past a merged PR on the issue — only after the operator "
        "confirmed genuine follow-up scope",
    )
    args = parser.parse_args(argv)
    result = Bootstrap(args.tag, args.spec_out, args.allow_merged).execute()
    print(json.dumps(result, indent=2))
    return 0 if result["stopped_at"] is None else 1


if __name__ == "__main__":
    raise SystemExit(main())
