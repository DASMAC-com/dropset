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

import io
import os
import tempfile
import unittest
from contextlib import redirect_stderr, redirect_stdout

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

    def test_the_two_regex_boundaries_are_pinned_separately(self) -> None:
        # One fixture cannot pin both halves: `/var/cargo-cache` is rejected by
        # the lookbehind AND the lookahead, so deleting either alone leaves it
        # passing. These two fail on exactly one boundary each.
        lookbehind = ds.parse("FROM debian:x AS b\nRUN ./cargo build\n")
        lookahead = ds.parse("FROM debian:x AS b\nRUN cargo-deny check\n")
        self.assertFalse(lookbehind[0].is_rust)
        self.assertFalse(lookahead[0].is_rust)

    def test_rustc_also_marks_a_stage_rust(self) -> None:
        text = "FROM rust:1-bookworm AS solo\nCOPY . .\nRUN rustc --version\n"
        self.assertEqual(rust_count(text), 1)

    def test_a_rust_looking_file_with_no_rust_stage_is_reported(self) -> None:
        # The backstop for the detector itself: the zero-Rust alarm is
        # repo-global, so a single file whose cargo call the regex misses would
        # otherwise pass in silence while the known images keep the total
        # non-zero. Here the build runs through a wrapper script.
        text = (
            "FROM rust:1-bookworm AS builder\n"
            "WORKDIR /app\n"
            "COPY . .\n"
            "RUN ./scripts/build.sh\n"
        )
        self.assertEqual(rust_count(text), 0)
        found = problems(text)
        self.assertTrue(
            any("nothing here was checked" in line for line in found),
            f"a Rust-looking file was silently unchecked: {found}",
        )

    def test_a_complaint_names_a_line(self) -> None:
        # The line number is the only thing pointing an operator at the stage
        # that failed, and every other assertion here matches on message text.
        found = problems(BROKEN)
        self.assertTrue(found)
        for line in found:
            self.assertRegex(line, r"^f\.Dockerfile:\d+: ")


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

    def test_copying_a_different_single_file_is_not_the_pin(self) -> None:
        # The check is on the BASENAME, not merely on "exactly one source": a
        # stage that copies some other single file and then resolves would key
        # the download on that file instead of on the pin.
        text = (
            "FROM rust:1-bookworm AS chef\n"
            "WORKDIR /app\n"
            "COPY Cargo.lock ./\n"
            "RUN rustup toolchain install\n"
        )
        self.assertIsNone(ds.provides_pin(ds.parse(text)[0]))

    def test_rustup_show_is_not_the_resolve(self) -> None:
        # This test previously asserted the opposite. `rustup show` was accepted
        # as a resolve on the assumption that it installs the active toolchain —
        # never verified, and rustup 1.28 stopped auto-installing on `show`, so
        # such a stage would resolve nothing and leave the download on the build
        # path. Requiring the install command fails closed.
        text = (
            "FROM rust:1-bookworm AS chef\n"
            "WORKDIR /app\n"
            "COPY rust-toolchain.toml ./\n"
            "RUN rustup show\n"
            "RUN cargo build\n"
        )
        self.assertIsNone(ds.provides_pin(ds.parse(text)[0]))

    def test_installing_a_named_toolchain_is_not_the_resolve(self) -> None:
        # The hole the adversarial pass found: an argument to
        # `rustup toolchain install` overrides the pin file, and a channel name
        # carries no digits for the version scan to catch — so this read as a
        # valid pin layer while installing the wrong compiler.
        for spelling in ("stable", "nightly", "1.98.0"):
            with self.subTest(spelling=spelling):
                text = (
                    "FROM rust:1-bookworm AS chef\n"
                    "WORKDIR /app\n"
                    "COPY rust-toolchain.toml ./\n"
                    f"RUN rustup toolchain install {spelling}\n"
                    "RUN cargo build\n"
                )
                self.assertIsNone(ds.provides_pin(ds.parse(text)[0]))

    def test_flags_on_the_resolve_are_still_the_resolve(self) -> None:
        # The bound on the rule above: a flag is not a toolchain name.
        text = (
            "FROM rust:1-bookworm AS chef\n"
            "WORKDIR /app\n"
            "COPY rust-toolchain.toml ./\n"
            "RUN rustup toolchain install --profile minimal\n"
            "RUN cargo build\n"
        )
        self.assertEqual(problems(text), [])

    def test_a_narrow_context_copy_before_the_resolve_disqualifies(self) -> None:
        # `COPY src /app` is not "broad", and keys the pin layer on the source
        # tree exactly as `COPY . .` would. Enumerating broad spellings left
        # this open while reading as complete.
        text = (
            "FROM rust:1-bookworm AS chef\n"
            "WORKDIR /app\n"
            "COPY src /app\n"
            "COPY rust-toolchain.toml ./\n"
            "RUN rustup toolchain install\n"
        )
        self.assertIsNone(ds.provides_pin(ds.parse(text)[0]))

    def test_a_bare_rustup_call_is_not_the_resolve(self) -> None:
        # Narrowness matters as much as breadth: `rustup --version` installs
        # nothing, so a stage whose only post-copy rustup call is that one has
        # no pin layer and the download stays on the build path.
        text = (
            "FROM rust:1-bookworm AS chef\n"
            "WORKDIR /app\n"
            "COPY rust-toolchain.toml ./\n"
            "RUN rustup --version\n"
            "RUN cargo build\n"
        )
        self.assertIsNone(ds.provides_pin(ds.parse(text)[0]))
        self.assertIn("does not inherit", " ".join(problems(text)))

    def test_a_json_form_broad_copy_disqualifies(self) -> None:
        # `COPY [".", "."]` is a whole-context copy in Docker's exec form. A
        # naive whitespace split leaves the brackets attached, so it matched
        # nothing in the broad-source set and silently failed to disqualify.
        text = (
            "FROM rust:1-bookworm AS chef\n"
            'WORKDIR /app\nCOPY [".", "."]\n'
            "COPY rust-toolchain.toml ./\n"
            "RUN rustup toolchain install\n"
        )
        self.assertIsNone(ds.provides_pin(ds.parse(text)[0]))


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
        # Guard the anchor. Every `.replace` fixture whose expected result is
        # `[]` is silently vacuous if the anchor drifts: the replace no-ops,
        # `text is FIXED`, and the assertion reduces to one another test
        # already makes — so the two-hop chain walk would go untested while
        # this still passed.
        self.assertNotEqual(text, FIXED)
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

    def test_a_broad_copy_in_an_ANCESTOR_is_caught(self) -> None:
        # The bypass that shipped in the first draft of this guard, and the
        # sharpest possible regression test: every per-stage rule is satisfied
        # — `chef` copies the pin alone and resolves it, and both later stages
        # descend from `chef` — yet the pin layer's parent is a whole-tree COPY
        # in `base`, so the download is source-keyed and re-paid on every
        # commit. That is the exact measured bug, and it passed the guard.
        text = (
            "FROM rust:1-bookworm AS base\n"
            "WORKDIR /app\n"
            "COPY . .\n"
            "\n"
            "FROM base AS chef\n"
            "COPY rust-toolchain.toml ./\n"
            "RUN rustup toolchain install \\\n"
            "    && cargo install cargo-chef --locked\n"
        )
        found = problems(text)
        self.assertTrue(
            any("ancestor 'base'" in line for line in found),
            f"an ancestor's broad COPY was not reported: {found}",
        )

    def test_a_redundant_resolve_below_a_valid_pin_is_not_reported(self) -> None:
        # The ancestor rule's own false positive. `tools` inherits chef's pin
        # layer through `deps`, so its defensive repeat of the resolve downloads
        # nothing — the toolchain is already in the inherited layer. Reporting
        # that it "re-pays the download" would be a false statement as well as a
        # false positive, on a hook where one blocks every commit in the repo.
        text = (
            "FROM rust:1-bookworm AS chef\n"
            "WORKDIR /app\n"
            "COPY rust-toolchain.toml ./\n"
            "RUN rustup toolchain install\n"
            "\n"
            "FROM chef AS deps\n"
            "COPY . .\n"
            "RUN cargo build --release\n"
            "\n"
            "FROM deps AS tools\n"
            "COPY rust-toolchain.toml ./\n"
            "RUN rustup toolchain install\n"
            "RUN cargo install --path tools\n"
        )
        self.assertEqual(problems(text), [])

    def test_a_forward_stage_reference_is_not_credited_with_a_pin(self) -> None:
        # Docker resolves a `FROM` name only against stages declared earlier,
        # so this `builder` builds on the base image and inherits nothing.
        text = (
            "FROM rust:1-bookworm AS builder\n"
            "COPY . .\n"
            "RUN cargo build --release\n"
            "\n"
            "FROM rust:1-bookworm AS chef\n"
            "WORKDIR /app\n"
            "COPY rust-toolchain.toml ./\n"
            "RUN rustup toolchain install\n"
        )
        found = problems(text)
        self.assertTrue(
            any("'builder'" in line and "does not inherit" in line for line in found),
            f"a forward reference was credited with a later stage's pin: {found}",
        )


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
        self.assertNotEqual(text, FIXED)
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
        # Discriminating on purpose. With a relative `./` target both sides of
        # the comparison derive from the same inherited value, so dropping
        # ancestor inheritance would collapse both to "/" and still compare
        # equal — passing for the wrong reason. An ABSOLUTE copy target and no
        # local WORKDIR means this passes only if inheritance really works.
        text = (
            "FROM rust:1-bookworm AS base\n"
            "WORKDIR /app\n"
            "\n"
            "FROM base AS chef\n"
            "COPY rust-toolchain.toml /app/rust-toolchain.toml\n"
            "RUN rustup toolchain install\n"
            "RUN cargo build\n"
        )
        self.assertEqual(problems(text), [])

    def test_a_rustup_call_before_the_pin_copy_is_not_a_cargo_call(self) -> None:
        # rustup compiles nothing, so this is a legitimate Dockerfile. Flagging
        # it produced the message "runs cargo before resolving the pin" about a
        # stage that runs no cargo — a false positive with a wrong diagnosis.
        text = (
            "FROM rust:1-bookworm AS chef\n"
            "WORKDIR /app\n"
            "RUN rustup --version\n"
            "COPY rust-toolchain.toml ./\n"
            "RUN rustup toolchain install \\\n"
            "    && cargo install cargo-chef --locked\n"
        )
        self.assertEqual(problems(text), [])

    def test_resolving_below_the_copy_directory_is_accepted(self) -> None:
        # rustup searches the working directory AND its parents for the
        # override file (verified against rustup 1.29.0), so a resolve in a
        # subdirectory of the copy target reads the pin correctly. An equality
        # check rejected this legitimate shape.
        text = (
            "FROM rust:1-bookworm AS chef\n"
            "WORKDIR /app\n"
            "COPY rust-toolchain.toml ./\n"
            "WORKDIR /app/crates\n"
            "RUN rustup toolchain install\n"
            "RUN cargo build\n"
        )
        self.assertEqual(problems(text), [])

    def test_resolving_above_the_copy_directory_is_still_caught(self) -> None:
        # The bound on the rule above: upward search does not help when the
        # resolve runs in a PARENT of the directory the file landed in.
        text = (
            "FROM rust:1-bookworm AS chef\n"
            "WORKDIR /app/crates\n"
            "COPY rust-toolchain.toml ./\n"
            "WORKDIR /app\n"
            "RUN rustup toolchain install\n"
            "RUN cargo build\n"
        )
        found = problems(text)
        self.assertTrue(any("reads no override" in line for line in found), found)


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

    def test_a_registry_port_does_not_hide_the_tag(self) -> None:
        # `partition(":")` on the whole reference splits at the registry's
        # port, reads the repo as the registry host, and skips the check.
        text = FIXED.replace(
            "rust:1-bookworm", "registry.example:5000/rust:1.98-bookworm"
        )
        self.assertNotEqual(text, FIXED)
        found = problems(text)
        self.assertTrue(any("base image tag" in line for line in found), found)

    def test_env_rustup_toolchain_is_caught(self) -> None:
        # It overrides the pin file for every later command in the stage, so it
        # names a version as surely as a rustup argument does — and the version
        # scan reads only `RUN`, which is how it escaped. Fails OPEN.
        text = FIXED.replace(
            "WORKDIR /app\n", "WORKDIR /app\nENV RUSTUP_TOOLCHAIN=1.98.1\n"
        )
        self.assertNotEqual(text, FIXED)
        found = problems(text)
        self.assertTrue(any("RUSTUP_TOOLCHAIN" in line for line in found), found)

    def test_a_pinned_rustup_download_url_is_not_a_compiler_pin(self) -> None:
        # A bare `"rustup" in segment` test also matches a URL and a tarball
        # name, blocking a stage that bootstraps rustup itself with a
        # misdiagnosis of a download path.
        text = (
            "FROM debian:bookworm-slim AS chef\n"
            "WORKDIR /app\n"
            "COPY rust-toolchain.toml ./\n"
            "RUN curl -sSf https://static.rust-lang.org/rustup/1.28.1/init.sh "
            "-o init.sh\n"
            "RUN rustup toolchain install\n"
            "RUN cargo build\n"
        )
        self.assertEqual(problems(text), [])

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


class TestCli(unittest.TestCase):
    """The entry point `cfg/pre-commit-lint.yml` actually invokes.

    Everything else here drives `check_file` / `check` directly, so `main()`
    was the one surface with no coverage at all — argparse wiring, `--root`,
    path normalization, `--show`, and exit-code propagation. A `main()` that
    returned 0 unconditionally passed the whole suite.
    """

    def write(self, root: str, rel: str, body: str) -> None:
        full = os.path.join(root, rel)
        os.makedirs(os.path.dirname(full), exist_ok=True)
        with open(full, "w", encoding="utf-8") as handle:
            handle.write(body)

    def test_exit_zero_on_a_clean_root(self) -> None:
        with tempfile.TemporaryDirectory() as root:
            self.write(root, "a.Dockerfile", FIXED)
            with redirect_stderr(io.StringIO()):
                self.assertEqual(ds.main(["--root", root]), 0)

    def test_exit_one_and_report_on_a_broken_root(self) -> None:
        with tempfile.TemporaryDirectory() as root:
            self.write(root, "a.Dockerfile", BROKEN)
            err = io.StringIO()
            with redirect_stderr(err):
                self.assertEqual(ds.main(["--root", root]), 1)
        self.assertIn("does not inherit", err.getvalue())

    def test_show_prints_the_graph_and_exits_zero(self) -> None:
        with tempfile.TemporaryDirectory() as root:
            self.write(root, "a.Dockerfile", BROKEN)
            out = io.StringIO()
            with redirect_stdout(out):
                # `--show` is an inspection view, so it reports even on a tree
                # the guard would reject.
                self.assertEqual(ds.main(["--root", root, "--show"]), 0)
        printed = out.getvalue()
        self.assertIn("a.Dockerfile", printed)
        self.assertIn("NO PIN", printed)

    def test_an_explicit_path_suppresses_the_zero_rust_alarm(self) -> None:
        with tempfile.TemporaryDirectory() as root:
            self.write(root, "web.Dockerfile", NODE_ONLY)
            target = os.path.join(root, "web.Dockerfile")
            with redirect_stderr(io.StringIO()):
                self.assertEqual(ds.main(["--root", root, target]), 0)


class TestThisCheckout(unittest.TestCase):
    """The guard, and two of its preconditions, against the real repo."""

    @classmethod
    def setUpClass(cls) -> None:
        cls.root = ds.find_root(os.path.dirname(os.path.abspath(__file__)))

    def test_find_root_resolves_to_this_checkout(self) -> None:
        # Worktrees live UNDER the base checkout
        # (`<base>/.claude/worktrees/<tag>/`), so an upward walk that missed the
        # worktree's own marker files would resolve to the base repo — and every
        # assertion in this class would then be made against a different tree
        # while still reporting green.
        self.assertTrue(
            os.path.abspath(__file__).startswith(self.root + os.sep),
            f"find_root returned {self.root}, which does not contain this test",
        )

    def test_the_pin_layer_is_identical_across_the_rust_images(self) -> None:
        # The measured win — ONE shared pin layer for all four images, so the
        # toolchain downloads once per machine rather than once per image —
        # holds only while the layer's cache key matches. Every per-file rule
        # would still pass if one image drifted to
        # `COPY rust-toolchain.toml /app/`, silently turning one download into
        # four, so the cross-file identity needs its own assertion.
        #
        # The BASE IMAGE is part of the compared tuple, and leaving it out was
        # the gap the cross-check found: one image moving to
        # `FROM rust:1-slim` keeps every instruction identical while splitting
        # the layer four ways — exactly the regression this test exists for.
        #
        # What it deliberately does NOT assert, so the name promises no more
        # than it checks: the comparison is on the PARSED form, which collapses
        # inside a continued `RUN` where Docker's cache key would not. A
        # re-indented continuation still passes here.
        keys: dict[str, tuple[object, ...]] = {}
        for rel in ds.discover(self.root):
            with open(os.path.join(self.root, rel), encoding="utf-8") as handle:
                stages = ds.parse(handle.read())
            for stage in stages:
                index = ds.provides_pin(stage)
                if index is None:
                    continue
                # Keyed per stage rather than per file: a file carrying two pin
                # stages would otherwise record only the last.
                keys[f"{rel}:{stage.label}"] = (
                    stage.base,
                    tuple(
                        (inst.keyword, inst.args)
                        for inst in stage.instructions[: index + 1]
                    ),
                )
        self.assertGreaterEqual(len(keys), 4, f"expected four pin stages: {keys}")
        self.assertEqual(
            len(set(keys.values())),
            1,
            f"the pin layer diverges across images: {keys}",
        )

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
