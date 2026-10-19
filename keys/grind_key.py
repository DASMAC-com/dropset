#!/usr/bin/env python3
# cspell:word keygen
"""Grind one mainnet vanity keypair into a Phantom-importable text file.

Runs ``solana-keygen grind --starts-with <prefix>:1`` (case-sensitive)
in an output directory outside every checkout, then converts the keygen
JSON array to ``<pubkey>.key``: mode 600, its entire contents the
base58 encoding of the 64-byte secret keypair, the form Phantom's
import and export dialogs use. The keygen JSON is deleted, so no other
key-file form is left behind.

Prints only the public key, the output path and the byte count; the
secret is never printed or logged. See ``keys/README.md`` → "Mainnet
keys" for the verification and storage steps.
"""

import argparse
import json
import os
import subprocess
import sys
from pathlib import Path

# cspell:disable-next-line
ALPHABET = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"
DEFAULT_OUT_DIR = Path.home() / "Desktop" / "dropset-vanity"
KEYPAIR_LEN = 64


def b58encode(data: bytes) -> str:
    """Bitcoin-alphabet base58, one leading ``1`` per leading zero byte."""
    n = int.from_bytes(data, "big")
    out = ""
    while n:
        n, r = divmod(n, 58)
        out = ALPHABET[r] + out
    pad = len(data) - len(data.lstrip(b"\0"))
    return "1" * pad + out


def write_key_file(out_dir: Path, keypair: bytes) -> Path:
    """Write ``<pubkey>.key`` holding the base58 keypair; return its path.

    Refuses to overwrite an existing file, and creates it at mode 600
    so the secret is never readable by another user, even briefly.
    """
    if len(keypair) != KEYPAIR_LEN:
        raise ValueError(f"keypair is {len(keypair)} bytes, want {KEYPAIR_LEN}")
    path = out_dir / f"{b58encode(keypair[32:])}.key"
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(fd, "w") as f:
        f.write(b58encode(keypair))
    return path


def grind(prefix: str, out_dir: Path) -> Path:
    """Run the keygen grind in ``out_dir``; return the new JSON's path."""
    before = set(out_dir.glob("*.json"))
    subprocess.run(
        [
            "solana-keygen",
            "grind",
            "--starts-with",
            f"{prefix}:1",
            "--no-bip39-passphrase",
        ],
        cwd=out_dir,
        check=True,
        stdout=subprocess.DEVNULL,
    )
    new = set(out_dir.glob("*.json")) - before
    if len(new) != 1:
        raise RuntimeError(f"expected one new keypair JSON, found {len(new)}")
    return new.pop()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("prefix", help="case-sensitive base58 prefix")
    parser.add_argument(
        "--out-dir",
        type=Path,
        default=DEFAULT_OUT_DIR,
        help=f"output directory (default: {DEFAULT_OUT_DIR})",
    )
    args = parser.parse_args()
    if not args.prefix or any(c not in ALPHABET for c in args.prefix):
        parser.error(f"prefix {args.prefix!r} is not base58")

    os.umask(0o077)
    args.out_dir.mkdir(parents=True, exist_ok=True, mode=0o700)
    json_path = grind(args.prefix, args.out_dir)
    try:
        keypair = bytes(json.loads(json_path.read_text()))
        if f"{b58encode(keypair[32:])}.json" != json_path.name:
            raise RuntimeError(f"{json_path.name}: public half mismatch")
        key_path = write_key_file(args.out_dir, keypair)
    finally:
        json_path.unlink()

    print(f"pubkey: {key_path.stem}")
    print(f"path:   {key_path}")
    print(f"bytes:  {key_path.stat().st_size}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
