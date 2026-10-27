#!/usr/bin/env python3
"""Check a built program ELF and record its provenance beside it.

Run by ``make program-mainnet`` on the artifact it has just built:

1. Asserts the ELF is sBPF v3 by reading ``e_flags`` straight from the
   header (the 4-byte field at offset ``0x30`` that ``llvm-readelf -h``
   prints as ``Flags``; ``docs/program-build.md`` → "Reading an ELF"),
   so no platform-tools binary has to be on ``PATH``. A wrong-arch build
   fails here instead of at deploy.
2. Writes ``<artifact>.sha256`` in ``sha256sum`` format, so
   ``sha256sum -c`` (or ``shasum -a 256 -c``) re-checks it.
3. Writes ``<artifact>.provenance.json``: the hash, the ``e_flags``
   value, the byte size, and the git commit the build ran at, with a
   ``dirty`` flag when the tree had uncommitted changes or untracked,
   non-ignored files.

It never carries an *expected* hash: it records what was built, so an
upgrade changes nothing here. Tying the on-chain bytes to the recorded
hash is the deploy ceremony's dump-and-compare.
"""

import argparse
import hashlib
import json
import subprocess
import sys
from pathlib import Path

ELF_MAGIC = b"\x7fELF"
ELFCLASS64 = 2
ELFDATA2LSB = 1
E_FLAGS_OFF = 0x30
SBPF_V3 = 0x3


class ArtifactError(Exception):
    """The artifact is not an sBPF v3 ELF."""


def read_e_flags(header: bytes) -> int:
    """Return ``e_flags`` from a 64-bit little-endian ELF header."""
    if len(header) < E_FLAGS_OFF + 4:
        raise ArtifactError("file is shorter than an ELF64 header")
    if header[:4] != ELF_MAGIC:
        raise ArtifactError("not an ELF file (bad magic)")
    if header[4] != ELFCLASS64 or header[5] != ELFDATA2LSB:
        raise ArtifactError("not a 64-bit little-endian ELF")
    return int.from_bytes(header[E_FLAGS_OFF : E_FLAGS_OFF + 4], "little")


def git_state(cwd: Path) -> tuple[str, bool]:
    """Return ``(commit, dirty)``; ``("unknown", True)`` outside git."""
    try:
        commit = subprocess.run(
            ["git", "rev-parse", "HEAD"],
            cwd=cwd,
            capture_output=True,
            text=True,
            check=True,
        ).stdout.strip()
        status = subprocess.run(
            # Untracked files count (an untracked .cargo/config.toml or .s
            # can change the build); ignored ones, target/ included, don't.
            ["git", "status", "--porcelain"],
            cwd=cwd,
            capture_output=True,
            text=True,
            check=True,
        ).stdout
    except (OSError, subprocess.CalledProcessError):
        return "unknown", True
    return commit, bool(status.strip())


def record(artifact: Path, commit: str, dirty: bool) -> dict:
    """Check ``artifact`` and write its ``.sha256`` and provenance files.

    A previous run's files are removed first, so a failed check never
    leaves an earlier build's hash beside a newer artifact.
    """
    sha_path = artifact.with_name(artifact.name + ".sha256")
    prov_path = artifact.with_name(artifact.name + ".provenance.json")
    sha_path.unlink(missing_ok=True)
    prov_path.unlink(missing_ok=True)
    data = artifact.read_bytes()
    e_flags = read_e_flags(data[:64])
    if e_flags != SBPF_V3:
        raise ArtifactError(f"e_flags is {e_flags:#x}, expected {SBPF_V3:#x} (sBPF v3)")
    digest = hashlib.sha256(data).hexdigest()
    manifest = {
        "artifact": artifact.name,
        "sha256": digest,
        "e_flags": f"{e_flags:#x}",
        "bytes": len(data),
        "commit": commit,
        "dirty": dirty,
    }
    sha_path.write_text(f"{digest}  {artifact.name}\n")
    prov_path.write_text(json.dumps(manifest, indent=2) + "\n")
    return manifest


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("artifact", type=Path)
    args = parser.parse_args(argv)
    # The checkout this script lives in is the source tree being built,
    # wherever the artifact (or a shared target dir) sits.
    commit, dirty = git_state(Path(__file__).resolve().parent)
    try:
        manifest = record(args.artifact, commit, dirty)
    except (OSError, ArtifactError) as e:
        print(f"record-artifact: {args.artifact}: {e}", file=sys.stderr)
        return 1
    print(json.dumps(manifest))
    if dirty:
        print(
            "record-artifact: WARNING: built from a dirty tree; this hash"
            " does not reproduce from the commit alone",
            file=sys.stderr,
        )
    return 0


if __name__ == "__main__":
    sys.exit(main())
