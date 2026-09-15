#!/usr/bin/env python3
# cspell:word ONBUILD
"""Guard the Dockerfiles' stage graph: every Rust stage inherits the pin layer.

The failure this exists for, measured live on 2026-09-15 while the operator
brought up a demo. ``rust:1-bookworm`` floats and had moved to rustc 1.98.1;
the workspace pins 1.98.0 *exactly*. Every Rust stage received the pin file
only with the whole-tree ``COPY . .``, so the first ``cargo`` invocation after
it made rustup download the full pinned toolchain — five components, several
hundred megabytes — inside a layer whose parent was that COPY. Any source
change invalidated it, so the download was re-paid on **every**
rebuild-after-commit, once per image. Two multi-minute stalls on the demo path.

Fixing the layer order once does not keep it fixed, so this guard asserts the
structure rather than any timing (a timing canary would be flaky, and the
structure is the real invariant). Per Dockerfile that builds Rust at all:

1. **One stage provides the pin** — it copies ``rust-toolchain.toml`` alone and
   resolves it, so the download lands in a layer keyed only on that file and
   invalidates only on a deliberate pin bump.
2. **Nothing broad precedes the resolve** — no whole-tree COPY in that stage or
   any ancestor before it, which is exactly what made the layer source-keyed.
3. **The resolve is that stage's first Rust command**, so nothing compiles on
   the image's own floating compiler.
4. **Every Rust stage descends from the pin stage**, so cook and build share
   one compiler. Compiled artifacts are keyed by compiler version, so a cook
   stage on a different one leaves much of its output invalid at build time —
   measured here at 144 crates recompiled against 71 once the compilers match.
   Note the honest size of that: it is under a second on this workspace, so
   this rule is a correctness tidy-up, not the saving. Rule 2 is the saving.
5. **No stage names a version** — agreement with ``rust-toolchain.toml`` is by
   construction, never by duplication. The base image's major-only tag is the
   rustup bootstrap and stays allowed; an ``x.y`` tag or a version handed to
   rustup is not.

Matching **zero** Rust stages across every Dockerfile is itself a failure: it
means the discovery broke (a renamed file, a new instruction spelling), and a
guard that silently checks nothing is worse than no guard.

The parse is deliberately small and local — comments dropped, continuations
joined, ``FROM``/``RUN``/``COPY``/``WORKDIR`` read positionally. It is not a
Dockerfile interpreter: no ``ARG`` substitution, no heredocs. Heredocs are
absent from all five of this repo's Dockerfiles (checked). ``ARG`` is not — the
Node-only ``explorer.Dockerfile`` interpolates one into its ``FROM`` — and that
is harmless here only because the file has no Rust stage, so an unexpanded
``${NODE_VERSION}`` never reaches a decision. A full parser would be a
dependency this guard does not earn.
"""

from __future__ import annotations

import argparse
import os
import re
import sys
from typing import NamedTuple

# The pin file, and the only thing the pin layer is allowed to be keyed on.
TOOLCHAIN_FILE = "rust-toolchain.toml"

# Trees a Dockerfile never lives in, and which are expensive to walk (`target`
# alone is multi-GB, and `.claude/worktrees` holds a `target` per checkout).
SKIP_DIRS = frozenset(
    {
        ".git",
        ".next",
        ".pnpm-store",
        ".turbo",
        "dist",
        "node_modules",
        "target",
        "worktrees",
    }
)

_ROOT_MARKERS = ("Cargo.toml", "pnpm-workspace.yaml")

# A command that touches the Rust toolchain at image-build time. `cargo-chef`
# is reached as `cargo chef`, so the `cargo` branch already covers it.
_RUST_CMD_RE = re.compile(r"(?<![\w./-])(cargo|rustc|rustup)(?![\w-])")

# The subset that COMPILES, and so is the thing that must not run before the
# pin resolves. `rustup` itself downloads no crates and is deliberately absent:
# `RUN rustup --version` ahead of the pin COPY is harmless, and flagging it
# blocked a correct Dockerfile with the message "runs cargo before resolving
# the pin", which it does not.
_CARGO_CMD_RE = re.compile(r"(?<![\w./-])(cargo|rustc)(?![\w-])")

# A loose mention of the toolchain, used only for the per-file backstop below.
# Deliberately wider than `_RUST_CMD_RE`: its job is to notice a file that
# looks Rust-shaped while the precise detector found nothing in it.
_RUST_HINT_RE = re.compile(r"(?i)(cargo|rustc|rustup|\brust\b|rust:)")

# The two sane spellings of "resolve whatever rust-toolchain.toml says". Both
# read the override file in the working directory and neither names a version:
# `rustup toolchain install` with no argument installs the active toolchain
# (rustup >= 1.28), and `rustup show` reports it.
_RESOLVE_RE = re.compile(r"\brustup\s+(?:toolchain\s+install|show)\b")

# A literal compiler version, in a base image tag or on a rustup command line.
_VERSION_RE = re.compile(r"\b\d+\.\d+(?:\.\d+)?\b")

# `cargo +1.98.0 build` / `rustc +stable` — a per-command toolchain override,
# which is the other way to disagree with the pin file.
_PLUS_TOOLCHAIN_RE = re.compile(r"(?<![\w./-])(?:cargo|rustc)\s+\+\S+")

# Sources that pull in the whole context rather than a named file.
_BROAD_SOURCES = frozenset({".", "./", "/", "*", "**", "./*"})

# Shell operators that separate one command from the next inside a RUN.
_SEGMENT_RE = re.compile(r"&&|\|\||;|\|")


class Instruction(NamedTuple):
    """One logical Dockerfile instruction, continuations already joined."""

    keyword: str  # upper-cased
    args: str
    line: int  # 1-indexed, the line the instruction starts on

    @property
    def is_rust(self) -> bool:
        """Whether this instruction runs cargo, rustc or rustup."""
        return self.keyword == "RUN" and bool(_RUST_CMD_RE.search(self.args))

    @property
    def is_cargo(self) -> bool:
        """Whether this instruction COMPILES — cargo or rustc, never rustup."""
        return self.keyword == "RUN" and bool(_CARGO_CMD_RE.search(self.args))

    @property
    def is_resolve(self) -> bool:
        """Whether this instruction resolves the pin file's toolchain."""
        return self.keyword == "RUN" and bool(_RESOLVE_RE.search(self.args))


class Stage(NamedTuple):
    """One ``FROM`` block."""

    name: str | None  # the `AS` name, lower-cased (Docker folds case)
    base: str  # raw base reference: an image, or an earlier stage's name
    line: int
    instructions: list[Instruction]

    @property
    def label(self) -> str:
        """How to name this stage in a message."""
        return self.name or f"<unnamed stage at line {self.line}>"

    @property
    def is_rust(self) -> bool:
        return any(inst.is_rust for inst in self.instructions)


def _operands(args: str) -> list[str]:
    """The operands of a ``COPY``/``ADD``, flags dropped, JSON form unwrapped.

    Docker accepts an exec/JSON form — ``COPY [".", "."]`` — and a naive
    whitespace split leaves the brackets and quotes attached, so ``".",``
    matches nothing in ``_BROAD_SOURCES`` and a whole-context copy in that
    spelling silently fails to disqualify a pin stage. That is the same
    false-negative class as an unchecked ancestor, so it is normalized here
    rather than documented as out of scope.
    """
    body = args.strip()
    if body.startswith("["):
        body = body.strip("[]")
        parts = [part.strip().strip('"').strip("'") for part in body.split(",")]
        return [part for part in parts if part]
    return [tok for tok in body.split() if not tok.startswith("--")]


def copy_sources(args: str) -> list[str]:
    """The source operands of a ``COPY``/``ADD``, flags and target dropped."""
    tokens = _operands(args)
    return tokens[:-1] if len(tokens) > 1 else []


def copy_target(args: str) -> str | None:
    """The target operand of a ``COPY``/``ADD``."""
    tokens = _operands(args)
    return tokens[-1] if len(tokens) > 1 else None


def is_broad_copy(inst: Instruction) -> bool:
    """Whether an instruction copies the whole build context."""
    if inst.keyword not in ("COPY", "ADD"):
        return False
    # A `COPY --from=<stage>` moves an artifact between stages; it never
    # brings the build context in, so it cannot make a layer source-keyed.
    if "--from=" in inst.args:
        return False
    return any(src in _BROAD_SOURCES for src in copy_sources(inst.args))


def copies_toolchain_alone(inst: Instruction) -> bool:
    """Whether an instruction copies the pin file and nothing else.

    ``COPY`` only, while :func:`is_broad_copy` also accepts ``ADD`` — so
    ``ADD rust-toolchain.toml ./`` reads as "no pin provider" and the file is
    rejected. That asymmetry fails closed, and is deliberate: ``ADD`` also
    fetches URLs and unpacks archives, so it is the wrong instruction for a
    layer whose whole purpose is a stable cache key.
    """
    if inst.keyword != "COPY" or "--from=" in inst.args:
        return False
    sources = copy_sources(inst.args)
    return len(sources) == 1 and os.path.basename(sources[0]) == TOOLCHAIN_FILE


def rust_before_resolve(inst: Instruction) -> bool:
    """Whether a cargo/rustc call precedes the resolve *inside* one ``RUN``.

    The rule this backs also has to hold within a single shell line, because
    consolidating two ``RUN``s is the obvious response to hadolint's DL3059 —
    and `cargo install … && rustup toolchain install` reintroduces exactly the
    unpinned compile the separate-instruction check would have caught. Another
    ``rustup`` before the resolve is fine: rustup itself downloads nothing.
    """
    resolve = _RESOLVE_RE.search(inst.args)
    if resolve is None:
        return False
    match = _CARGO_CMD_RE.search(inst.args)
    return match is not None and match.start() < resolve.start()


def parse(text: str) -> list[Stage]:
    """Parse a Dockerfile into its stages.

    Comment lines are dropped first and backslash continuations joined after,
    which is the order Docker itself uses — a comment *inside* a continuation
    is a comment, not part of the command.
    """
    stages: list[Stage] = []
    pending: list[str] = []
    start = 0

    def flush() -> None:
        if not pending:
            return
        joined = " ".join(part.strip() for part in pending).strip()
        pending.clear()
        if not joined:
            return
        head, _, rest = joined.partition(" ")
        keyword = head.upper()
        inst = Instruction(keyword=keyword, args=rest.strip(), line=start)
        if keyword == "FROM":
            stages.append(_new_stage(inst))
        elif stages:
            stages[-1].instructions.append(inst)
        # An instruction before the first FROM (a global ARG) belongs to no
        # stage and is dropped: nothing here reads one.

    for number, raw in enumerate(text.splitlines(), start=1):
        line = raw.rstrip("\n")
        if not line.strip() or line.lstrip().startswith("#"):
            continue
        if not pending:
            start = number
        if line.rstrip().endswith("\\"):
            pending.append(line.rstrip()[:-1])
            continue
        pending.append(line)
        flush()
    flush()
    return stages


def _new_stage(inst: Instruction) -> Stage:
    """Build a ``Stage`` from a parsed ``FROM`` instruction."""
    tokens = [tok for tok in inst.args.split() if not tok.startswith("--")]
    base = tokens[0] if tokens else ""
    name: str | None = None
    for index, token in enumerate(tokens):
        if token.upper() == "AS" and index + 1 < len(tokens):
            name = tokens[index + 1].lower()
            break
    return Stage(name=name, base=base, line=inst.line, instructions=[])


def provides_pin(stage: Stage) -> int | None:
    """Index of the resolve instruction, if this stage provides the pin.

    Requires the pin file to be copied *alone* and resolved afterwards. A
    broad COPY before the resolve disqualifies the stage: that is the original
    bug, where the layer carrying the download was keyed on the whole tree.
    """
    copied_at: int | None = None
    for index, inst in enumerate(stage.instructions):
        if is_broad_copy(inst):
            return None
        if copied_at is None and copies_toolchain_alone(inst):
            copied_at = index
            continue
        if copied_at is not None and inst.is_resolve:
            return index
    return None


def chain(stage: Stage, by_name: dict[str, Stage]) -> list[Stage]:
    """``stage`` and its ancestors, nearest first, following ``FROM`` names.

    A name resolves to a stage only if that stage is declared **earlier**;
    Docker treats a forward reference as an image name, so crediting a stage
    with a pin declared below it would be a false pass.
    """
    result: list[Stage] = []
    seen: set[str] = set()
    current: Stage | None = stage
    while current is not None:
        result.append(current)
        parent = by_name.get(current.base.lower())
        if parent is not None and parent.line >= current.line:
            parent = None
        # A stage whose name is its own base, or a cycle: stop rather than
        # loop. Docker would reject it; this guard just declines to hang.
        if parent is None or (parent.name or "") in seen:
            break
        seen.add(parent.name or "")
        current = parent
    return result


def workdir_at(ancestry: list[Stage], stop: Instruction | None) -> str | None:
    """The WORKDIR in effect at ``stop``, searching outward through ancestors.

    ``ancestry`` is nearest-first, so it is walked in reverse: an outer
    stage's WORKDIR is inherited, and a nearer one overrides it.
    """
    current: str | None = None
    for stage in reversed(ancestry):
        for inst in stage.instructions:
            if stop is not None and inst is stop:
                return current
            if inst.keyword == "WORKDIR" and inst.args:
                current = inst.args.split()[0]
    return current


def _resolve_dir(target: str, workdir: str | None) -> str:
    """The directory a COPY target names, relative paths joined onto WORKDIR."""
    cleaned = target
    if os.path.basename(cleaned) == TOOLCHAIN_FILE:
        cleaned = os.path.dirname(cleaned) or "."
    if not cleaned.startswith("/"):
        cleaned = os.path.join(workdir or "/", cleaned)
    return os.path.normpath(cleaned)


def _version_problems(
    path: str, stages: list[Stage], by_name: dict[str, Stage]
) -> list[str]:
    """Every place a stage names a compiler version instead of inheriting it."""
    problems: list[str] = []
    for stage in stages:
        # The base image's tag is the rustup bootstrap, so a major-only tag
        # (`rust:1-bookworm`) is fine. An `x.y` tag is a second pin that can
        # silently disagree with the file.
        if stage.base.lower() not in by_name:
            # Split the tag off the LAST path component: a registry with a
            # port (`registry.example:5000/rust:1.98-bookworm`) puts a colon before the
            # image name, so partitioning the whole reference reads the
            # registry as the repo and skips the check entirely.
            last = stage.base.rsplit("/", 1)[-1]
            repo, _, tag = last.partition(":")
            repo = repo.lower()
            if repo == "rust" and _VERSION_RE.search(tag):
                problems.append(
                    f"{path}:{stage.line}: stage '{stage.label}' pins the "
                    f"compiler in its base image tag ('{stage.base}'). The "
                    f"tag is only the rustup bootstrap — {TOOLCHAIN_FILE} is "
                    "the pin, and two of them drift apart silently."
                )
        for inst in stage.instructions:
            if inst.keyword != "RUN":
                continue
            # Per shell command, not per instruction: a consolidated RUN can
            # legitimately pin an unrelated tool's version beside the resolve
            # (`… && cargo install cargo-chef --version 0.1.68`), and reading
            # that as a compiler pin is a false positive with a confidently
            # wrong diagnosis attached.
            # `\brustup\s` matches the COMMAND, not the string: a bare
            # substring test also fires on `rustup-init-1.28.1` and on a
            # pinned download URL like `…/rustup/1.28.1/…`, blocking a stage
            # that bootstraps rustup itself with a misdiagnosis of a URL.
            rustup_versions = [
                segment
                for segment in _SEGMENT_RE.split(inst.args)
                if re.search(r"\brustup\s", segment) and _VERSION_RE.search(segment)
            ]
            if rustup_versions:
                problems.append(
                    f"{path}:{inst.line}: stage '{stage.label}' hands rustup a "
                    f"literal version. Resolve {TOOLCHAIN_FILE} instead "
                    "(`rustup toolchain install`, no argument) so the pin has "
                    "one home."
                )
            match = _PLUS_TOOLCHAIN_RE.search(inst.args)
            if match:
                problems.append(
                    f"{path}:{inst.line}: stage '{stage.label}' overrides the "
                    f"toolchain per-command ('{match.group(0)}'), bypassing "
                    f"{TOOLCHAIN_FILE}."
                )
    return problems


def check_file(path: str, text: str) -> tuple[list[str], int]:
    """Check one Dockerfile. Returns its problems and its Rust stage count."""
    stages = parse(text)
    by_name = {stage.name: stage for stage in stages if stage.name}
    rust_stages = [stage for stage in stages if stage.is_rust]
    if not rust_stages:
        # Backstop for the detector itself. The zero-Rust alarm in `check()` is
        # repo-GLOBAL, so while the four known images keep the total non-zero,
        # a fifth file whose cargo invocation this regex misses — a wrapper
        # script, a path-qualified binary, an `ONBUILD` — would pass in total
        # silence. A file that looks Rust-shaped and yields no Rust stage is
        # therefore reported per file. Comments are already dropped by `parse`,
        # so an incidental mention in prose cannot trip this.
        hint_line: int | None = None
        for stage in stages:
            if _RUST_HINT_RE.search(stage.base):
                hint_line = stage.line
                break
            for inst in stage.instructions:
                if _RUST_HINT_RE.search(inst.args):
                    hint_line = inst.line
                    break
            if hint_line is not None:
                break
        if hint_line is not None:
            return [
                f"{path}:{hint_line}: this Dockerfile mentions the Rust "
                "toolchain, but the guard found no Rust build stage in it — so "
                "nothing in this file was checked. Either the detection missed "
                "a cargo invocation (a wrapper script, a path-qualified "
                "binary, an ONBUILD) or the mention is incidental."
            ], 0
        return [], 0

    problems = _version_problems(path, stages, by_name)

    # Rule 1 and 4: every Rust stage reaches a pin provider through its
    # `FROM` chain — itself included, since the provider builds Rust too.
    for stage in rust_stages:
        provider = None
        for ancestor in chain(stage, by_name):
            if provides_pin(ancestor) is not None:
                provider = ancestor
                break
        if provider is None:
            problems.append(
                f"{path}:{stage.line}: Rust stage '{stage.label}' does not "
                f"inherit a pinned-toolchain layer. Copy {TOOLCHAIN_FILE} "
                "alone and run `rustup toolchain install` in this stage or an "
                "ancestor, before any broad COPY — otherwise the first cargo "
                "invocation downloads the whole pinned toolchain in a layer "
                "keyed on the source tree, and every commit re-pays it."
            )

    # Rules 2 and 3 are about the provider's own instruction order, so they are
    # checked once per provider rather than once per descendant.
    for provider in stages:
        resolve_index = provides_pin(provider)
        if resolve_index is None:
            continue
        resolve = provider.instructions[resolve_index]
        ancestry = chain(provider, by_name)

        # Anything cargo-shaped before the resolve compiles on the base
        # image's own floating compiler.
        # Rule 2 reaches ANCESTORS, not just this stage. An ancestor's
        # whole-tree COPY runs before this stage exists, so it keys the pin
        # layer on the source tree exactly as one in the same stage would — and
        # `provides_pin` scans a single stage, so it cannot see it. Without
        # this, a two-stage shape (`base` doing `COPY . .`, `chef` resolving the
        # pin) reproduced the measured bug and passed the guard.
        for ancestor in ancestry[1:]:
            broad = next(
                (inst for inst in ancestor.instructions if is_broad_copy(inst)), None
            )
            if broad is not None:
                problems.append(
                    f"{path}:{broad.line}: stage '{provider.label}' resolves "
                    f"the pin, but its ancestor '{ancestor.label}' copies the "
                    "whole build context first — so the pin layer is keyed on "
                    "the source tree anyway and every commit re-pays the "
                    "download. Move the broad COPY into a descendant stage."
                )
                break

        earlier = [
            inst for inst in provider.instructions[:resolve_index] if inst.is_cargo
        ]
        if earlier or rust_before_resolve(resolve):
            offender = earlier[0] if earlier else resolve
            problems.append(
                f"{path}:{offender.line}: stage '{provider.label}' runs "
                "cargo before resolving the pin, so that layer compiles on "
                "the base image's floating compiler. Move the "
                f"{TOOLCHAIN_FILE} COPY and `rustup toolchain install` above "
                "it."
            )

        # The resolve only reads the pin file if it runs where the file landed.
        copies = [
            inst
            for inst in provider.instructions[:resolve_index]
            if copies_toolchain_alone(inst)
        ]
        if copies:
            run_dir = os.path.normpath(workdir_at(ancestry, resolve) or "/")
            copy_dirs = [
                _resolve_dir(copy_target(copy.args) or ".", workdir_at(ancestry, copy))
                for copy in copies
            ]
            # At or BELOW, and ANY of the copies. Two corrections to a stricter
            # earlier form: rustup searches the working directory *and its
            # parents* for the override file (verified — resolving from
            # `db-schema/` picked up the repo-root pin), so a resolve in a
            # subdirectory of the copy target is correct; and `provides_pin`
            # accepts the FIRST toolchain copy while a later one may be the one
            # that lands, so adjudicating only the last produced a false
            # positive when a stage copied it twice.
            if not any(
                run_dir == copy_dir or run_dir.startswith(copy_dir.rstrip("/") + "/")
                for copy_dir in copy_dirs
            ):
                problems.append(
                    f"{path}:{resolve.line}: stage '{provider.label}' resolves "
                    f"the toolchain in {run_dir}, which is neither "
                    f"{' nor '.join(copy_dirs)} nor below it, so rustup reads "
                    "no override there and the download moves back onto the "
                    "build path."
                )

    return problems, len(rust_stages)


class NoCheckoutRoot(Exception):
    """No checkout root above the starting directory."""


def find_root(start: str) -> str:
    """Walk up from ``start`` to the checkout root, by marker file."""
    current = os.path.abspath(start)
    while True:
        if all(
            os.path.exists(os.path.join(current, marker)) for marker in _ROOT_MARKERS
        ):
            return current
        parent = os.path.dirname(current)
        if parent == current:
            raise NoCheckoutRoot(
                f"no checkout root above {start} "
                f"(looked for {' + '.join(_ROOT_MARKERS)})"
            )
        current = parent


def is_dockerfile(name: str) -> bool:
    """Whether a filename is a Dockerfile, in either spelling this repo uses."""
    return name == "Dockerfile" or name.endswith(".Dockerfile")


def discover(root: str) -> list[str]:
    """Every Dockerfile under ``root``, repo-relative and sorted."""
    found: list[str] = []
    for current, dirs, files in os.walk(root):
        dirs[:] = sorted(d for d in dirs if d not in SKIP_DIRS)
        for name in files:
            if is_dockerfile(name):
                found.append(os.path.relpath(os.path.join(current, name), root))
    return sorted(found)


def check(root: str, paths: list[str] | None = None) -> tuple[int, list[str]]:
    """Run the guard. Returns an exit code and the lines to report.

    With ``paths``, only those files are read and the zero-Rust alarm is off:
    a caller naming one Node-only Dockerfile is asking about that file, not
    claiming the repo has no Rust images.
    """
    scoped = bool(paths)
    targets = paths if paths else discover(root)
    if not targets:
        return 1, [
            "dockerfile-stages: found no Dockerfiles to check under "
            f"{root}. This guard verifies the Rust images' layer order, so "
            "finding nothing means the discovery broke, not that the repo is "
            "clean."
        ]

    problems: list[str] = []
    rust_stages = 0
    for rel in targets:
        full = rel if os.path.isabs(rel) else os.path.join(root, rel)
        try:
            with open(full, encoding="utf-8") as handle:
                text = handle.read()
        except (OSError, UnicodeDecodeError) as err:
            # Fails closed, like the sibling guards: an unreadable Dockerfile
            # means this guard verified nothing about it.
            problems.append(
                f"dockerfile-stages: cannot read {rel} ({type(err).__name__}: {err})."
            )
            continue
        file_problems, count = check_file(rel, text)
        problems.extend(file_problems)
        rust_stages += count

    if rust_stages == 0 and not scoped:
        problems.append(
            "dockerfile-stages: matched zero Rust build stages across "
            f"{len(targets)} Dockerfile(s). Every Rust image in "
            "infra/localnet/ builds with cargo, so this means the stage parse "
            "or the cargo detection broke — a guard that checks nothing is "
            "worse than no guard. Fix the parse in "
            ".claude/tools/dockerfile_stages.py."
        )

    if problems:
        return 1, [
            "dockerfile-stages: the Rust images' pinned-toolchain layer "
            "invariant is broken.",
            "",
            *problems,
            "",
            "Why this is a guard: the pin file arriving with `COPY . .` put a "
            "several-hundred-megabyte toolchain download in a source-keyed "
            "layer, so every rebuild-after-commit re-paid it (measured "
            "2026-09-15, two multi-minute stalls on the demo path).",
        ]
    # Silent on success: this hook is `always_run`, so a line here would print
    # on every commit. `--show` is how you ask for the graph.
    return 0, []


def render(root: str, paths: list[str] | None = None) -> list[str]:
    """The stage graph, as the guard sees it — the inspection view."""
    lines: list[str] = []
    for rel in paths if paths else discover(root):
        full = rel if os.path.isabs(rel) else os.path.join(root, rel)
        try:
            with open(full, encoding="utf-8") as handle:
                stages = parse(handle.read())
        except (OSError, UnicodeDecodeError) as err:
            lines.append(f"{rel}: unreadable ({type(err).__name__})")
            continue
        by_name = {stage.name: stage for stage in stages if stage.name}
        rust = sum(1 for stage in stages if stage.is_rust)
        lines.append(f"{rel}  ({len(stages)} stages, {rust} rust)")
        for stage in stages:
            marks = []
            if stage.is_rust:
                marks.append("rust")
            if provides_pin(stage) is not None:
                marks.append("pin")
            elif stage.is_rust:
                ancestry = chain(stage, by_name)
                provider = next(
                    (anc for anc in ancestry if provides_pin(anc) is not None), None
                )
                marks.append(f"pin<-{provider.label}" if provider else "NO PIN")
            suffix = f"  [{', '.join(marks)}]" if marks else ""
            lines.append(f"  {stage.label} <- {stage.base}{suffix}")
    return lines


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        prog="dockerfile_stages.py",
        description=(
            "Guard the Dockerfiles' stage graph: assert every Rust build "
            "stage inherits a pinned-toolchain layer keyed only on "
            f"{TOOLCHAIN_FILE}, that nothing compiles before the pin "
            "resolves, and that no stage names a compiler version."
        ),
    )
    parser.add_argument(
        "paths",
        nargs="*",
        help="Dockerfiles to check (default: every one under the root)",
    )
    parser.add_argument(
        "--root",
        default=None,
        help="checkout root (default: the root above the cwd)",
    )
    parser.add_argument(
        "--show",
        action="store_true",
        help="print the stage graph and its pin inheritance, then exit 0",
    )
    args = parser.parse_args(argv)

    try:
        root = args.root or find_root(os.getcwd())
    except NoCheckoutRoot as err:
        print(f"dockerfile-stages: {err}", file=sys.stderr)
        return 1

    paths = [os.path.relpath(os.path.abspath(p), root) for p in args.paths]
    if args.show:
        for line in render(root, paths or None):
            print(line)
        return 0

    code, lines = check(root, paths or None)
    if lines:
        print("\n".join(lines), file=sys.stderr)
    return code


if __name__ == "__main__":
    sys.exit(main())
