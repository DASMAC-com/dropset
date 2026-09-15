#!/usr/bin/env python3
"""Unit tests for ``dockerfile_stages.py`` (stdlib ``unittest``; no pytest).

Three kinds of test, and the third is the one that would have caught the bug.

The **parser** tests pin the small Dockerfile reader, because every rule rests
on it: a missed continuation splits one ``RUN`` into two, and a stage whose
instructions land in the wrong bucket makes the guard pass on anything.

The **rule** tests drive ``check_file`` over embedded fixtures, one per way the
invariant can break — including the *exact* pre-fix shape of this repo's four
Rust Dockerfiles, which the guard must reject. A guard nobody has watched fail
is a guard that might only ever pass.

The **repo** tests assert against this checkout's own Dockerfiles. Two of them
reach outside the guard on purpose: the pin file has to exist for the new
``COPY`` to resolve, and the root ``.dockerignore`` must not exclude it —
otherwise every image build fails at that COPY, which is a failure mode this
guard's own parse cannot see (it reads Dockerfiles, not the build context).
"""

from __future__ import annotations

import os
import tempfile
import unittest

import docker_context as dc
import dockerfile_stages as ds

# The four Rust Dockerfiles all shared this shape before ENG-1523: the pin file
# arrives only with `COPY . .`, so the planner's first cargo call downloads the
# whole pinned toolchain into a layer keyed on the source tree.
BROKEN = """\
FROM rust:1-bookworm AS chef
RUN cargo install cargo-chef --locked
WORKDIR /app

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json
COPY . .
RUN cargo build --release

FROM debian:bookworm-slim AS runtime
COPY --from=builder /app/target/release/thing /usr/local/bin/thing
CMD ["thing"]
"""

# The shape this guard exists to hold: the pin resolves in the chef stage, in a
# layer keyed only on the pin file, and both later stages descend from it.
FIXED = """\
FROM rust:1-bookworm AS chef
WORKDIR /app
COPY rust-toolchain.toml ./
RUN rustup toolchain install \\
    && cargo install cargo-chef --locked

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json
COPY . .
RUN cargo build --release

FROM debian:bookworm-slim AS runtime
COPY --from=builder /app/target/release/thing /usr/local/bin/thing
CMD ["thing"]
"""

NODE_ONLY = """\
ARG NODE_VERSION=22
FROM node:${NODE_VERSION}-bookworm-slim AS build
WORKDIR /app
COPY . .
RUN npm ci && npm run build

FROM node:${NODE_VERSION}-bookworm-slim AS run
COPY --from=build /app/dist /app/dist
CMD ["node", "/app/dist/main.js"]
"""


def problems(text: str) -> list[str]:
    """The guard's complaints about one Dockerfile body."""
    return ds.check_file("f.Dockerfile", text)[0]


def rust_count(text: str) -> int:
    """How many Rust stages the guard sees in one Dockerfile body."""
    return ds.check_file("f.Dockerfile", text)[1]


class TestParse(unittest.TestCase):
    def test_joins_continuations(self) -> None:
        # A split RUN read as two instructions would let a cargo call hide in
        # the tail of a line the guard thought it had already classified.
        stages = ds.parse(
            "FROM rust:1-bookworm AS b\n"
            "RUN cargo chef cook --release \\\n"
            "    --recipe-path recipe.json\n"
        )
        self.assertEqual(len(stages[0].instructions), 1)
        self.assertEqual(
            stages[0].instructions[0].args,
            "cargo chef cook --release --recipe-path recipe.json",
        )

    def test_drops_comment_inside_continuation(self) -> None:
        # Docker strips comments before joining continuations, so a comment
        # line in the middle of one is a comment — not part of the command.
        stages = ds.parse(
            "FROM rust:1-bookworm AS b\n"
            "RUN cargo build \\\n"
            "# hadolint ignore=DL3008\n"
            "    --release\n"
        )
        self.assertEqual(stages[0].instructions[0].args, "cargo build --release")

    def test_stage_names_fold_case(self) -> None:
        stages = ds.parse("FROM rust:1-bookworm As Chef\nRUN cargo --version\n")
        self.assertEqual(stages[0].name, "chef")

    def test_reports_instruction_lines(self) -> None:
        stages = ds.parse("# note\n\nFROM rust:1 AS b\nWORKDIR /app\nRUN cargo b\n")
        self.assertEqual(stages[0].line, 3)
        self.assertEqual(stages[0].instructions[-1].line, 5)

    def test_instruction_before_first_from_is_dropped(self) -> None:
        stages = ds.parse(NODE_ONLY)
        self.assertEqual([stage.name for stage in stages], ["build", "run"])


class TestRustDetection(unittest.TestCase):
    def test_cargo_run_marks_a_stage_rust(self) -> None:
        self.assertEqual(rust_count(FIXED), 3)

    def test_node_dockerfile_has_no_rust_stages(self) -> None:
        self.assertEqual(rust_count(NODE_ONLY), 0)
        self.assertEqual(problems(NODE_ONLY), [])

    def test_copy_only_stage_is_not_rust(self) -> None:
        # The runtime stage copies a compiled binary out of the builder. It
        # runs no cargo, so demanding it inherit the pin layer would force a
        # Rust base image into a thin Debian runtime.
        stages = ds.parse(FIXED)
        runtime = next(stage for stage in stages if stage.name == "runtime")
        self.assertFalse(runtime.is_rust)

    def test_a_word_containing_cargo_is_not_a_cargo_call(self) -> None:
        stages = ds.parse(
            "FROM debian:bookworm-slim AS b\nRUN mkdir -p /var/cargo-cache\n"
        )
        self.assertFalse(stages[0].is_rust)


class TestPinProvider(unittest.TestCase):
    def test_recognizes_the_pin_stage(self) -> None:
        stages = ds.parse(FIXED)
        self.assertIsNotNone(ds.provides_pin(stages[0]))

    def test_broad_copy_before_the_resolve_disqualifies(self) -> None:
        # This is the original bug in miniature: the resolve is present, but
        # its layer's parent is the whole tree, so any source change re-pays
        # the download. The guard must not accept it.
        text = (
            "FROM rust:1-bookworm AS chef\n"
            "WORKDIR /app\n"
            "COPY . .\n"
            "RUN rustup toolchain install\n"
        )
        self.assertIsNone(ds.provides_pin(ds.parse(text)[0]))
        self.assertIn("does not inherit", " ".join(problems(text)))

    def test_resolve_before_the_copy_disqualifies(self) -> None:
        # rustup reads the override file at invocation time, so resolving
        # first installs the image default and pins nothing.
        text = (
            "FROM rust:1-bookworm AS chef\n"
            "WORKDIR /app\n"
            "RUN rustup toolchain install\n"
            "COPY rust-toolchain.toml ./\n"
        )
        self.assertIsNone(ds.provides_pin(ds.parse(text)[0]))

    def test_copying_the_pin_file_alongside_others_disqualifies(self) -> None:
        # `COPY rust-toolchain.toml Cargo.lock ./` keys the layer on Cargo.lock
        # too, so a dependency bump re-pays the toolchain download.
        text = (
            "FROM rust:1-bookworm AS chef\n"
            "WORKDIR /app\n"
            "COPY rust-toolchain.toml Cargo.lock ./\n"
            "RUN rustup toolchain install\n"
        )
        self.assertIsNone(ds.provides_pin(ds.parse(text)[0]))

    def test_copy_from_another_stage_is_not_a_broad_copy(self) -> None:
        # `COPY --from=planner` moves an artifact between stages and never
        # brings the build context in, so it cannot make a layer source-keyed.
        stages = ds.parse(FIXED)
        builder = next(stage for stage in stages if stage.name == "builder")
        self.assertFalse(ds.is_broad_copy(builder.instructions[0]))


class TestInheritance(unittest.TestCase):
    def test_the_broken_shape_is_rejected_at_every_rust_stage(self) -> None:
        found = problems(BROKEN)
        self.assertEqual(len(found), 3)
        for stage in ("chef", "planner", "builder"):
            self.assertTrue(
                any(f"'{stage}'" in line for line in found),
                f"no complaint named stage {stage}: {found}",
            )

    def test_the_fixed_shape_passes(self) -> None:
        self.assertEqual(problems(FIXED), [])

    def test_pin_inherited_through_a_grandparent(self) -> None:
        text = FIXED.replace("FROM chef AS builder", "FROM planner AS builder")
        self.assertEqual(problems(text), [])

    def test_a_rust_stage_off_the_chain_is_caught(self) -> None:
        # The realistic regression once the pin layer exists: someone adds a
        # stage straight from the base image, and it downloads the toolchain
        # again on a source-keyed layer.
        text = FIXED + (
            "\nFROM rust:1-bookworm AS tools\n"
            "COPY . .\n"
            "RUN cargo build --release --bin extra\n"
        )
        found = problems(text)
        self.assertEqual(len(found), 1)
        self.assertIn("'tools'", found[0])

    def test_a_self_referential_base_does_not_hang(self) -> None:
        text = "FROM chef AS chef\nCOPY . .\nRUN cargo build\n"
        self.assertEqual(len(problems(text)), 1)


class TestOrderWithinThePinStage(unittest.TestCase):
    def test_cargo_before_the_resolve_is_caught(self) -> None:
        # cargo-chef compiled before the pin resolves builds on the base
        # image's floating compiler — the mismatch that silently invalidates
        # the cooked dependency cache at build time.
        text = FIXED.replace(
            "COPY rust-toolchain.toml ./",
            "RUN cargo install cargo-chef --locked\nCOPY rust-toolchain.toml ./",
        )
        found = problems(text)
        self.assertTrue(any("before resolving the pin" in line for line in found))

    def test_cargo_before_the_resolve_inside_one_run_is_caught(self) -> None:
        # The regression consolidating two RUNs invites: hadolint's DL3059
        # asks for one instruction, and swapping the two halves of the `&&`
        # puts the cargo-chef compile back on the floating compiler while the
        # instruction-level order still looks right.
        text = FIXED.replace(
            "RUN rustup toolchain install \\\n    && cargo install cargo-chef --locked",
            "RUN cargo install cargo-chef --locked \\\n    && rustup toolchain install",
        )
        self.assertIn("RUN cargo install", text)
        found = problems(text)
        self.assertTrue(
            any("before resolving the pin" in line for line in found), found
        )

    def test_another_rustup_call_before_the_resolve_is_fine(self) -> None:
        # rustup itself downloads no toolchain, so this is not the bug.
        text = FIXED.replace(
            "RUN rustup toolchain install \\",
            "RUN rustup --version \\\n    && rustup toolchain install \\",
        )
        self.assertEqual(problems(text), [])

    def test_resolve_outside_the_copy_directory_is_caught(self) -> None:
        text = (
            "FROM rust:1-bookworm AS chef\n"
            "WORKDIR /app\n"
            "COPY rust-toolchain.toml ./\n"
            "WORKDIR /src\n"
            "RUN rustup toolchain install\n"
            "RUN cargo build\n"
        )
        found = problems(text)
        self.assertTrue(any("reads no override" in line for line in found), found)

    def test_absolute_copy_target_matching_the_workdir_passes(self) -> None:
        text = (
            "FROM rust:1-bookworm AS chef\n"
            "WORKDIR /app\n"
            "COPY rust-toolchain.toml /app/rust-toolchain.toml\n"
            "RUN rustup toolchain install\n"
            "RUN cargo build\n"
        )
        self.assertEqual(problems(text), [])

    def test_workdir_inherited_from_the_parent_stage(self) -> None:
        text = (
            "FROM rust:1-bookworm AS base\n"
            "WORKDIR /app\n"
            "\n"
            "FROM base AS chef\n"
            "COPY rust-toolchain.toml ./\n"
            "RUN rustup toolchain install\n"
            "RUN cargo build\n"
        )
        self.assertEqual(problems(text), [])


class TestVersionDuplication(unittest.TestCase):
    def test_version_in_the_base_tag_is_caught(self) -> None:
        text = FIXED.replace("rust:1-bookworm", "rust:1.98.0-bookworm")
        found = problems(text)
        self.assertTrue(any("base image tag" in line for line in found), found)

    def test_major_only_base_tag_is_allowed(self) -> None:
        # The tag is the rustup bootstrap, not the pin. Rejecting it would
        # leave nothing to install rustup from.
        self.assertEqual(problems(FIXED), [])

    def test_version_handed_to_rustup_is_caught(self) -> None:
        text = FIXED.replace(
            "RUN rustup toolchain install", "RUN rustup toolchain install 1.98.0"
        )
        found = problems(text)
        self.assertTrue(any("literal version" in line for line in found), found)

    def test_per_command_toolchain_override_is_caught(self) -> None:
        text = FIXED.replace("RUN cargo build --release", "RUN cargo +nightly build")
        found = problems(text)
        self.assertTrue(any("per-command" in line for line in found), found)

    def test_a_version_elsewhere_is_not_a_rust_pin(self) -> None:
        # Only the rustup *command* is scanned for a version, not the whole
        # instruction. Since the resolve shares one consolidated RUN with the
        # cargo-chef install, an instruction-wide scan reads a pinned tool
        # version as a second compiler pin — a false positive arriving with a
        # confidently wrong diagnosis. Caught by this test, live.
        text = FIXED.replace(
            "&& cargo install cargo-chef --locked",
            "&& cargo install cargo-chef --locked --version 0.1.68",
        )
        self.assertIn("--version 0.1.68", text)
        self.assertEqual(problems(text), [])


class TestCheckDriver(unittest.TestCase):
    def write(self, root: str, rel: str, body: str) -> None:
        full = os.path.join(root, rel)
        os.makedirs(os.path.dirname(full), exist_ok=True)
        with open(full, "w", encoding="utf-8") as handle:
            handle.write(body)

    def test_zero_rust_stages_on_a_full_scan_fails_loudly(self) -> None:
        # The failure a silent guard would hide: the parse stops recognizing
        # cargo, every file "passes", and nothing is checked any more.
        with tempfile.TemporaryDirectory() as root:
            self.write(root, "web.Dockerfile", NODE_ONLY)
            code, lines = ds.check(root)
        self.assertEqual(code, 1)
        self.assertTrue(any("zero Rust build stages" in line for line in lines))

    def test_scoped_paths_suppress_the_zero_rust_alarm(self) -> None:
        # A caller naming one Node-only file asks about that file; it is not
        # claiming the repo has no Rust images.
        with tempfile.TemporaryDirectory() as root:
            self.write(root, "web.Dockerfile", NODE_ONLY)
            self.assertEqual(ds.check(root, ["web.Dockerfile"]), (0, []))

    def test_no_dockerfiles_at_all_fails(self) -> None:
        with tempfile.TemporaryDirectory() as root:
            code, lines = ds.check(root)
        self.assertEqual(code, 1)
        self.assertTrue(any("no Dockerfiles" in line for line in lines))

    def test_unreadable_file_fails_closed(self) -> None:
        # Named explicitly, because discovery only ever yields real files: the
        # path a caller passes is the one that can be a directory or a dead
        # symlink. An unreadable Dockerfile means the guard verified nothing
        # about it, so it fails rather than skipping.
        with tempfile.TemporaryDirectory() as root:
            os.makedirs(os.path.join(root, "a.Dockerfile"))
            code, lines = ds.check(root, ["a.Dockerfile"])
        self.assertEqual(code, 1)
        self.assertTrue(any("cannot read" in line for line in lines), lines)

    def test_discovery_skips_the_fat_trees(self) -> None:
        # `target` holds a Dockerfile-shaped path in a vendored crate now and
        # then, and `.claude/worktrees` carries one `target` per checkout.
        with tempfile.TemporaryDirectory() as root:
            self.write(root, "infra/a.Dockerfile", FIXED)
            self.write(root, "target/vendor/Dockerfile", BROKEN)
            self.write(root, "node_modules/x/Dockerfile", BROKEN)
            self.assertEqual(ds.discover(root), [os.path.join("infra", "a.Dockerfile")])
            self.assertEqual(ds.check(root), (0, []))

    def test_finds_both_dockerfile_spellings(self) -> None:
        self.assertTrue(ds.is_dockerfile("Dockerfile"))
        self.assertTrue(ds.is_dockerfile("collectors.Dockerfile"))
        self.assertFalse(ds.is_dockerfile("Dockerfile.md"))
        self.assertFalse(ds.is_dockerfile("docker-compose.yml"))


class TestThisCheckout(unittest.TestCase):
    """The guard, and two of its preconditions, against the real repo."""

    @classmethod
    def setUpClass(cls) -> None:
        cls.root = ds.find_root(os.path.dirname(os.path.abspath(__file__)))

    def test_the_repo_passes(self) -> None:
        code, lines = ds.check(self.root)
        self.assertEqual(code, 0, "\n".join(lines))

    def test_every_rust_dockerfile_provides_a_pin_stage(self) -> None:
        # Counted rather than reasoned about: a Rust image added without the
        # pin layer should fail here as well as in the guard.
        seen = 0
        for rel in ds.discover(self.root):
            with open(os.path.join(self.root, rel), encoding="utf-8") as handle:
                stages = ds.parse(handle.read())
            if not any(stage.is_rust for stage in stages):
                continue
            seen += 1
            self.assertTrue(
                any(ds.provides_pin(stage) is not None for stage in stages),
                f"{rel} builds Rust but provides no pinned-toolchain layer",
            )
        self.assertGreaterEqual(seen, 4, "expected at least the four Rust images")

    def test_the_pin_file_exists(self) -> None:
        # Every Rust image now COPYs it by name, so a rename would break the
        # builds at that COPY rather than at a cargo call.
        self.assertTrue(
            os.path.exists(os.path.join(self.root, ds.TOOLCHAIN_FILE)),
            f"{ds.TOOLCHAIN_FILE} is what every Rust image's pin layer copies",
        )

    def test_the_pin_file_survives_the_dockerignore(self) -> None:
        # The failure this catches is invisible to the guard's own parse: an
        # over-broad ignore pattern would make `COPY rust-toolchain.toml ./`
        # fail in every Rust image, with no Dockerfile at fault.
        with open(os.path.join(self.root, ".dockerignore"), encoding="utf-8") as handle:
            rules = dc.parse_dockerignore(handle.read())
        self.assertFalse(
            dc.is_ignored(ds.TOOLCHAIN_FILE, rules),
            f"{ds.TOOLCHAIN_FILE} is excluded from the build context, so the "
            "pin layer's COPY cannot resolve",
        )


if __name__ == "__main__":
    unittest.main()
