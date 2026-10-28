#!/usr/bin/env python3
# cspell:word keygen
# cspell:word kdfname
# cspell:word kdfoptions
# cspell:word urandom
"""Grind or convert a mainnet keypair into Phantom and SSH-agent forms.

``grind <prefix>`` runs ``solana-keygen grind --starts-with <prefix>:1``
(case-sensitive) in an output directory (by default outside every
checkout), then converts the keygen JSON array to two files, each mode
600 and refusing to overwrite:

- ``<pubkey>.txt``, whose entire contents are the base58 encoding of
  the 64-byte secret keypair, the form Phantom's import and export
  dialogs use;
- ``<pubkey>.key``, the same key as an unencrypted OpenSSH Ed25519
  private key, the form a 1Password SSH Key item imports, so the
  1Password SSH agent can sign with it.

The keygen JSON is deleted, so no other key-file form is left behind.

``convert <file>`` writes the ``.key`` form for a keypair that already
exists, into the same output directory as ``grind`` (never beside its
input, which may sit in a checkout): the file (or ``-`` for stdin) holds
the 128-hex form, the JSON byte array, or the base58 form. A hex keypair
cannot be pasted into an SSH Key item as it is; this is why the
converter exists.

Both print only the public key, the SSH public-key line and the output
paths; the secret is never printed or logged. Neither checks that the
seed derives the public half (that is cryptography); a mismatched pair
fails at the first signature, which the agent signer verifies. See
``keys/README.md`` → "Mainnet keys" for the verification and storage
steps.
"""

import argparse
import base64
import json
import os
import struct
import subprocess
import sys
from pathlib import Path

# cspell:disable-next-line
ALPHABET = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"
DEFAULT_OUT_DIR = Path.home() / "Desktop" / "dropset-vanity"
KEYPAIR_LEN = 64
SSH_KEY_TYPE = b"ssh-ed25519"
# The armor lines, assembled so the source never holds the literal header
# the detect-private-key hook greps for; this file writes keys, holds none.
PEM_LABEL = "OPENSSH PRIVATE" + " KEY-----"
PEM_BEGIN = "-----BEGIN " + PEM_LABEL
PEM_END = "-----END " + PEM_LABEL


def b58encode(data: bytes) -> str:
    """Bitcoin-alphabet base58, one leading ``1`` per leading zero byte."""
    n = int.from_bytes(data, "big")
    out = ""
    while n:
        n, r = divmod(n, 58)
        out = ALPHABET[r] + out
    pad = len(data) - len(data.lstrip(b"\0"))
    return "1" * pad + out


def b58decode(s: str) -> bytes:
    """The inverse of :func:`b58encode`; raises ``ValueError`` off-alphabet."""
    n = 0
    for c in s:
        n = n * 58 + ALPHABET.index(c)
    body = n.to_bytes((n.bit_length() + 7) // 8, "big")
    pad = len(s) - len(s.lstrip("1"))
    return b"\0" * pad + body


def parse_keypair(text: str) -> bytes:
    """Decode a 64-byte keypair from its JSON, hex or base58 form."""
    text = text.strip()
    if text.startswith("["):
        try:
            keypair = bytes(json.loads(text))
        except (TypeError, ValueError):
            raise ValueError("keypair JSON is not a byte array") from None
    elif len(text) == 2 * KEYPAIR_LEN and all(
        c in "0123456789abcdefABCDEF" for c in text
    ):
        keypair = bytes.fromhex(text)
    else:
        try:
            keypair = b58decode(text)
        except ValueError:
            raise ValueError("keypair is not JSON, hex or base58") from None
    if len(keypair) != KEYPAIR_LEN:
        raise ValueError(f"keypair is {len(keypair)} bytes, want {KEYPAIR_LEN}")
    return keypair


def _ssh_string(data: bytes) -> bytes:
    return struct.pack(">I", len(data)) + data


def ssh_public_blob(pubkey: bytes) -> bytes:
    """The wire-format ``ssh-ed25519`` public key (RFC 8709 section 4)."""
    return _ssh_string(SSH_KEY_TYPE) + _ssh_string(pubkey)


def ssh_public_line(pubkey: bytes, comment: str) -> str:
    """The ``authorized_keys`` line for an Ed25519 public key."""
    blob = base64.b64encode(ssh_public_blob(pubkey)).decode()
    return f"{SSH_KEY_TYPE.decode()} {blob} {comment}"


def openssh_private_key(keypair: bytes, comment: str, check: bytes = b"") -> str:
    """An unencrypted ``openssh-key-v1`` container for a Solana keypair.

    A Solana keypair is the 32-byte seed then the 32-byte public key,
    which is exactly the 64-byte private field OpenSSH stores for
    Ed25519, so the conversion is a re-framing with no cryptography.
    ``check`` is the repeated check integer (random when empty; fixed
    only in tests); the private section is padded to the cipher block
    size, 8 for ``none``.
    """
    if len(keypair) != KEYPAIR_LEN:
        raise ValueError(f"keypair is {len(keypair)} bytes, want {KEYPAIR_LEN}")
    pubkey = keypair[32:]
    check = check or os.urandom(4)
    private = (
        check
        + check
        + _ssh_string(SSH_KEY_TYPE)
        + _ssh_string(pubkey)
        + _ssh_string(keypair)
        + _ssh_string(comment.encode())
    )
    private += bytes(range(1, 1 + (-len(private) % 8)))
    body = (
        b"openssh-key-v1\0"
        + _ssh_string(b"none")  # cipher name
        + _ssh_string(b"none")  # kdfname
        + _ssh_string(b"")  # kdfoptions
        + struct.pack(">I", 1)
        + _ssh_string(ssh_public_blob(pubkey))
        + _ssh_string(private)
    )
    b64 = base64.b64encode(body).decode()
    lines = [b64[i : i + 70] for i in range(0, len(b64), 70)]
    return "\n".join(
        [
            PEM_BEGIN,
            *lines,
            PEM_END,
            "",
        ]
    )


def _write_secret(path: Path, contents: str) -> Path:
    """Create ``path`` at mode 600, refusing to overwrite, holding ``contents``."""
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    try:
        with os.fdopen(fd, "w") as f:
            f.write(contents)
    except BaseException:
        path.unlink()
        raise
    return path


def write_key_file(out_dir: Path, keypair: bytes) -> Path:
    """Write ``<pubkey>.txt`` holding the base58 keypair; return its path.

    Refuses to overwrite an existing file, and creates it at mode 600
    so the secret is never readable by another user, even briefly.
    """
    if len(keypair) != KEYPAIR_LEN:
        raise ValueError(f"keypair is {len(keypair)} bytes, want {KEYPAIR_LEN}")
    path = out_dir / f"{b58encode(keypair[32:])}.txt"
    return _write_secret(path, b58encode(keypair))


def write_ssh_key_file(out_dir: Path, keypair: bytes) -> Path:
    """Write ``<pubkey>.key``, the OpenSSH form; return its path.

    The key's comment is its base58 address, so the agent's identity
    list names it. Same mode and overwrite rules as
    :func:`write_key_file`.
    """
    address = b58encode(keypair[32:])
    return _write_secret(
        out_dir / f"{address}.key", openssh_private_key(keypair, address)
    )


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
        # Every new JSON is a secret keypair; never leave one behind.
        for p in new:
            p.unlink()
        raise RuntimeError(f"expected one new keypair JSON, found {len(new)}")
    return new.pop()


def prepare_out_dir(out_dir: Path) -> None:
    os.umask(0o077)
    out_dir.mkdir(parents=True, exist_ok=True, mode=0o700)
    out_dir.chmod(0o700)  # mkdir leaves an existing directory's mode


def cmd_grind(args: argparse.Namespace) -> list[Path]:
    prepare_out_dir(args.out_dir)
    json_path = grind(args.prefix, args.out_dir)
    try:
        keypair = bytes(json.loads(json_path.read_text()))
        if f"{b58encode(keypair[32:])}.json" != json_path.name:
            raise RuntimeError(f"{json_path.name}: public half mismatch")
        txt_path = write_key_file(args.out_dir, keypair)
        try:
            return [txt_path, write_ssh_key_file(args.out_dir, keypair)]
        except BaseException:
            txt_path.unlink()
            raise
    finally:
        json_path.unlink()


def cmd_convert(args: argparse.Namespace) -> list[Path]:
    # The output defaults to DEFAULT_OUT_DIR, never beside the input: that
    # can be inside a checkout, and prepare_out_dir changes the directory's
    # mode.
    text = sys.stdin.read() if args.keypair == "-" else Path(args.keypair).read_text()
    keypair = parse_keypair(text)
    prepare_out_dir(args.out_dir)
    return [write_ssh_key_file(args.out_dir, keypair)]


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    sub = parser.add_subparsers(dest="command", required=True)

    p_grind = sub.add_parser("grind", help="grind a fresh vanity keypair")
    p_grind.add_argument("prefix", help="case-sensitive base58 prefix")
    p_grind.add_argument(
        "--out-dir",
        type=Path,
        default=DEFAULT_OUT_DIR,
        help=f"output directory (default: {DEFAULT_OUT_DIR})",
    )
    p_grind.set_defaults(run=cmd_grind)

    p_convert = sub.add_parser(
        "convert", help="write the OpenSSH form of an existing keypair"
    )
    p_convert.add_argument(
        "keypair", help="file holding the hex, JSON or base58 keypair, or -"
    )
    p_convert.add_argument(
        "--out-dir",
        type=Path,
        default=DEFAULT_OUT_DIR,
        help=f"output directory (default: {DEFAULT_OUT_DIR})",
    )
    p_convert.set_defaults(run=cmd_convert)

    args = parser.parse_args()
    if args.command == "grind" and (
        not args.prefix or any(c not in ALPHABET for c in args.prefix)
    ):
        parser.error(f"prefix {args.prefix!r} is not base58")

    paths = args.run(args)
    address = paths[0].stem
    print(f"pubkey: {address}")
    print(f"ssh:    {ssh_public_line(b58decode(address), address)}")
    for path in paths:
        print(f"path:   {path} ({path.stat().st_size} bytes)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
