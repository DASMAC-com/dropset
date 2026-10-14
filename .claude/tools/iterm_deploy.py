#!/usr/bin/env python3
"""Point the live iTerm integration at the committed scripts, by symlink.

The tab-color hooks are wired user-wide to `~/.claude/scripts/iterm-*`, and the
reorderer runs from iTerm's `AutoLaunch/` folder. Both used to be COPIES, made
once by hand, so nothing committed afterwards ever reached the running
integration: a measured deploy was three months and several fixes behind
`main`, and the operator saw the flicker a merged fix had already removed.
This makes them symlinks into the base checkout instead — the same shape as
`.claude/shell/init.zsh`, where the local config holds only a pointer and the
code is whatever `main` has. A fast-forward of the base checkout is then the
whole deploy.

Usage:
  iterm_deploy.py            # report each target; exit 1 if any is not linked
  iterm_deploy.py --apply    # replace copies with links, then restart monitors

`--apply` never deletes a copy: it moves it into a timestamped backup directory
beside it. A stray symlink is only a pointer and is simply replaced.
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
import time
from pathlib import Path

from init_pr_branch import parse_base_repo

SCRIPTS_DIR = Path.home() / ".claude" / "scripts"
AUTOLAUNCH_DIR = (
    Path.home()
    / "Library"
    / "Application Support"
    / "iTerm2"
    / "Scripts"
    / "AutoLaunch"
)
REORDERER = "iterm-reorder.py"
RESTART = "iterm-restart-monitors.sh"
#: States that already run the committed code, so nothing is left to deploy.
LINKED_STATES = ("linked", "is-source")


def base_checkout() -> Path | None:
    """The worktree with `main` checked out — never this worktree's branch, whose
    scripts would vanish with the worktree."""
    out = subprocess.run(
        ["git", "worktree", "list", "--porcelain"],
        capture_output=True,
        text=True,
        check=False,
    ).stdout
    found = parse_base_repo(out)
    return Path(found) if found else None


def targets(source: Path, scripts_dir: Path, autolaunch_dir: Path) -> list[tuple]:
    """``(link_path, source_file)`` pairs for every committed `iterm-*` script.

    Test files are not deployed: they are not part of the integration. The
    AutoLaunch entry is included only where that folder exists, since its
    absence means the reorderer was never set up here, not that it is stale.
    """
    pairs = [
        (scripts_dir / path.name, path)
        for path in sorted(source.glob("iterm-*"))
        if path.is_file()
    ]
    if autolaunch_dir.is_dir() and (source / REORDERER).is_file():
        pairs.append((autolaunch_dir / REORDERER, source / REORDERER))
    return pairs


def status(link: Path, source: Path) -> str:
    """`linked` / `wrong-link` / `stale-copy` / `missing` / `is-source`.

    `is-source` is a target that IS the committed file — the deploy directory
    (or a parent of it) is itself a link into the checkout, or `--scripts-dir`
    names the source. Its leaf is no symlink, so it would otherwise read as a
    stale copy, and "backing it up" would move the committed file out of the
    repo and leave a link pointing at itself.
    """
    if link.is_symlink():
        return "linked" if link.resolve() == source.resolve() else "wrong-link"
    if link.exists():
        if link.resolve() == source.resolve():
            return "is-source"
        return "stale-copy"
    return "missing"


def apply(pairs: list[tuple], stamp: str) -> list[str]:
    """Link every target that is not linked. Returns the backup paths written.

    An `is-source` target is left untouched: that layout already runs the
    committed code, which is all a link would buy.
    """
    backups = []
    for link, source in pairs:
        state = status(link, source)
        if state in ("linked", "is-source"):
            continue
        if state == "stale-copy":
            backup_dir = link.parent / f".pre-symlink-{stamp}"
            backup_dir.mkdir(exist_ok=True)
            link.rename(backup_dir / link.name)
            backups.append(str(backup_dir / link.name))
        elif state == "wrong-link":
            link.unlink()
        link.parent.mkdir(parents=True, exist_ok=True)
        link.symlink_to(source.resolve())
    return backups


def orphans(source: Path, scripts_dir: Path) -> list[str]:
    """Deployed `iterm-*` files with no committed counterpart — an older script
    family, reported for the operator to remove rather than touched here."""
    if not scripts_dir.is_dir():
        return []
    committed = {path.name for path in source.glob("iterm-*")}
    return sorted(
        p.name for p in scripts_dir.glob("iterm-*") if p.name not in committed
    )


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--apply", action="store_true")
    parser.add_argument("--source", type=Path, help="default: the base checkout")
    parser.add_argument("--scripts-dir", type=Path, default=SCRIPTS_DIR)
    parser.add_argument("--autolaunch-dir", type=Path, default=AUTOLAUNCH_DIR)
    args = parser.parse_args(argv)

    if args.source is None:
        base = base_checkout()
        if base is None:
            print("iterm_deploy: no worktree has main checked out", file=sys.stderr)
            return 2
        args.source = base / ".claude" / "scripts"

    pairs = targets(args.source, args.scripts_dir, args.autolaunch_dir)
    if not pairs:
        # An empty set would pass the all-linked test vacuously.
        print(f"iterm_deploy: no iterm-* scripts under {args.source}", file=sys.stderr)
        return 2
    report: dict = {"source": str(args.source)}
    if args.apply:
        # Restarts are gated on what this run CHANGED, so a repeat --apply on a
        # linked tree stops nothing — the reorderer especially, which the
        # operator would then have to start again by hand for no reason.
        changed = [
            link for link, src in pairs if status(link, src) not in LINKED_STATES
        ]
        report["backups"] = apply(pairs, time.strftime("%Y%m%d-%H%M%S"))
        # The monitors are long-lived and keep running the loop they started
        # with. Stopping them is safe because the painter restarts a missing
        # monitor on its session's next hook (see `ensure_monitor`).
        restart = args.scripts_dir / RESTART
        if (
            any(link.parent == args.scripts_dir for link in changed)
            and restart.exists()
        ):
            done = subprocess.run(
                [str(restart)], capture_output=True, text=True, check=False
            )
            report["monitors"] = (
                done.stdout.strip() or done.stderr.strip() or f"exit {done.returncode}"
            )
        if args.autolaunch_dir / REORDERER in changed:
            # The running reorderer is the old code too. iTerm does not
            # relaunch an AutoLaunch script that exits, so it is stopped here
            # and started again by hand — one menu click, where restarting
            # iTerm would cost every tab. The pattern is anchored on the
            # AutoLaunch path so an editor or pager holding the file is spared.
            stopped = subprocess.run(
                ["pkill", "-f", f"AutoLaunch/{REORDERER.replace('.', '[.]')}"],
                check=False,
            )
            report["reorderer_stopped"] = stopped.returncode == 0
            report["next"] = (
                f"start the reorderer: iTerm2 Scripts menu > AutoLaunch > {REORDERER}"
            )
    report["targets"] = {str(link): status(link, src) for link, src in pairs}
    report["orphans"] = orphans(args.source, args.scripts_dir)
    print(json.dumps(report, indent=2))
    return 0 if all(s in LINKED_STATES for s in report["targets"].values()) else 1


if __name__ == "__main__":
    sys.exit(main())
