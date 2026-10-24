# cspell:word keygen
"""Tests for grind_key's base58 codec, key-file writers and converter.

The fixtures are the committed localnet throwaway ``AAAA.json`` and the
RFC 8032 test vector, never a real key.
"""

import base64
import json
import shutil
import stat
import struct
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))

import grind_key  # noqa: E402

FIXTURE = HERE / "AAAA.json"
FIXTURE_PUBKEY = "AAAAz3pYUMwhX1bsEtPx9LSWYbpRM8qrFaQgmKVX6oiV"

# RFC 8032 section 7.1, TEST 1: the secret seed and its public key.
RFC8032_SEED = bytes.fromhex(
    "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60"
)
RFC8032_PUBKEY = bytes.fromhex(
    "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a"
)
RFC8032_SSH_BLOB = (
    "AAAAC3NzaC1lZDI1NTE5AAAAINdamAGCsQq31Uv+08lkBzoO4XLz2qYjJa8CGmj3B1Ea"
)

b58decode = grind_key.b58decode


def read_string(buf: bytes, at: int) -> tuple[bytes, int]:
    (n,) = struct.unpack_from(">I", buf, at)
    return buf[at + 4 : at + 4 + n], at + 4 + n


def parse_openssh(text: str) -> tuple[bytes, bytes, bytes]:
    """Return (outer public key, inner public key, 64-byte private field).

    An independent reader of the ``openssh-key-v1`` layout (PROTOCOL.key
    in OpenSSH), so the round trip checks the container, not just the
    writer against itself.
    """
    lines = text.strip().splitlines()
    assert lines[0] == grind_key.PEM_BEGIN
    assert lines[-1] == grind_key.PEM_END
    assert all(len(line) <= 70 for line in lines[1:-1])
    buf = base64.b64decode("".join(lines[1:-1]))
    magic = b"openssh-key-v1\0"
    assert buf.startswith(magic)
    at = len(magic)
    for want in (b"none", b"none", b""):
        got, at = read_string(buf, at)
        assert got == want, (got, want)
    (key_count,) = struct.unpack_from(">I", buf, at)
    assert key_count == 1
    outer, at = read_string(buf, at + 4)
    _, k = read_string(outer, 0)
    outer_pub, _ = read_string(outer, k)
    private, at = read_string(buf, at)
    assert at == len(buf)
    assert len(private) % 8 == 0
    assert private[0:4] == private[4:8]
    key_type, p = read_string(private, 8)
    assert key_type == b"ssh-ed25519"
    inner_pub, p = read_string(private, p)
    secret, p = read_string(private, p)
    _comment, p = read_string(private, p)
    assert private[p:] == bytes(range(1, 1 + len(private) - p))
    return outer_pub, inner_pub, secret


class B58EncodeTest(unittest.TestCase):
    def test_known_vectors(self):
        self.assertEqual(grind_key.b58encode(b""), "")
        self.assertEqual(grind_key.b58encode(b"hello world"), "StV1DL6CwTryKyV")
        self.assertEqual(grind_key.b58encode(b"\0\0\x01"), "112")
        self.assertEqual(grind_key.b58encode(b"\0"), "1")

    def test_fixture_public_half_is_its_address(self):
        keypair = bytes(json.loads(FIXTURE.read_text()))
        self.assertEqual(grind_key.b58encode(keypair[32:]), FIXTURE_PUBKEY)


class WriteKeyFileTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.out_dir = Path(self.tmp.name)
        self.keypair = bytes(json.loads(FIXTURE.read_text()))

    def tearDown(self):
        self.tmp.cleanup()

    def test_name_contents_and_mode(self):
        path = grind_key.write_key_file(self.out_dir, self.keypair)
        self.assertEqual(path.name, f"{FIXTURE_PUBKEY}.txt")
        contents = path.read_text()
        self.assertNotIn("\n", contents)
        self.assertEqual(b58decode(contents), self.keypair)
        self.assertEqual(stat.S_IMODE(path.stat().st_mode), 0o600)
        self.assertEqual([p.name for p in self.out_dir.iterdir()], [path.name])

    def test_refuses_to_overwrite(self):
        grind_key.write_key_file(self.out_dir, self.keypair)
        with self.assertRaises(FileExistsError):
            grind_key.write_key_file(self.out_dir, self.keypair)

    def test_rejects_wrong_length(self):
        with self.assertRaises(ValueError):
            grind_key.write_key_file(self.out_dir, self.keypair[:32])


class ParseKeypairTest(unittest.TestCase):
    def setUp(self):
        self.keypair = bytes(json.loads(FIXTURE.read_text()))

    def test_every_form_decodes_to_the_same_bytes(self):
        for text in (
            FIXTURE.read_text(),
            self.keypair.hex(),
            self.keypair.hex().upper() + "\n",
            grind_key.b58encode(self.keypair),
        ):
            self.assertEqual(grind_key.parse_keypair(text), self.keypair)

    def test_rejects_wrong_length_and_junk(self):
        for text in (self.keypair[:32].hex(), "[1, 2, 3]", "not-a-key!"):
            with self.assertRaises(ValueError):
                grind_key.parse_keypair(text)


class OpenSshTest(unittest.TestCase):
    def setUp(self):
        self.keypair = RFC8032_SEED + RFC8032_PUBKEY

    def test_public_line_matches_the_rfc_vector(self):
        self.assertEqual(
            grind_key.ssh_public_line(RFC8032_PUBKEY, "c"),
            f"ssh-ed25519 {RFC8032_SSH_BLOB} c",
        )

    def test_round_trip_keeps_the_public_bytes(self):
        text = grind_key.openssh_private_key(self.keypair, "c", check=b"\1\2\3\4")
        outer_pub, inner_pub, secret = parse_openssh(text)
        self.assertEqual(outer_pub, RFC8032_PUBKEY)
        self.assertEqual(inner_pub, RFC8032_PUBKEY)
        self.assertEqual(secret, self.keypair)

    def test_padding_holds_for_every_comment_length(self):
        for n in range(9):
            parse_openssh(grind_key.openssh_private_key(self.keypair, "x" * n))

    @unittest.skipUnless(shutil.which("ssh-keygen"), "ssh-keygen not on PATH")
    def test_ssh_keygen_reads_the_container(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = grind_key.write_ssh_key_file(Path(tmp), self.keypair)
            out = subprocess.run(
                ["ssh-keygen", "-y", "-f", str(path)],
                check=True,
                capture_output=True,
                text=True,
            ).stdout
        self.assertEqual(out.split()[:2], ["ssh-ed25519", RFC8032_SSH_BLOB])


class WriteSshKeyFileTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.out_dir = Path(self.tmp.name)
        self.keypair = bytes(json.loads(FIXTURE.read_text()))

    def tearDown(self):
        self.tmp.cleanup()

    def test_name_contents_and_mode(self):
        path = grind_key.write_ssh_key_file(self.out_dir, self.keypair)
        self.assertEqual(path.name, f"{FIXTURE_PUBKEY}.key")
        self.assertEqual(parse_openssh(path.read_text())[2], self.keypair)
        self.assertEqual(stat.S_IMODE(path.stat().st_mode), 0o600)

    def test_refuses_to_overwrite(self):
        grind_key.write_ssh_key_file(self.out_dir, self.keypair)
        with self.assertRaises(FileExistsError):
            grind_key.write_ssh_key_file(self.out_dir, self.keypair)


class GrindTest(unittest.TestCase):
    """grind() with the keygen replaced by a stub that drops JSON files."""

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.out_dir = Path(self.tmp.name)

    def tearDown(self):
        self.tmp.cleanup()

    def stub_keygen(self, names):
        def run(argv, cwd, **_):
            self.assertEqual(
                argv[:4], ["solana-keygen", "grind", "--starts-with", "eBook:1"]
            )
            self.assertNotIn("--ignore-case", argv)
            for name in names:
                (Path(cwd) / name).write_text(FIXTURE.read_text())

        return mock.patch.object(grind_key.subprocess, "run", run)

    def test_returns_the_one_new_json(self):
        (self.out_dir / "old.json").write_text("[]")
        with self.stub_keygen([f"{FIXTURE_PUBKEY}.json"]):
            path = grind_key.grind("eBook", self.out_dir)
        self.assertEqual(path.name, f"{FIXTURE_PUBKEY}.json")

    def test_several_new_keypair_files_are_all_deleted(self):
        with self.stub_keygen(["a.json", "b.json"]):
            with self.assertRaises(RuntimeError):
                grind_key.grind("eBook", self.out_dir)
        self.assertEqual(list(self.out_dir.iterdir()), [])

    def test_cmd_grind_leaves_both_forms_and_no_json(self):
        args = mock.Mock(prefix="eBook", out_dir=self.out_dir)
        with self.stub_keygen([f"{FIXTURE_PUBKEY}.json"]):
            paths = grind_key.cmd_grind(args)
        self.assertEqual(
            [p.name for p in paths], [f"{FIXTURE_PUBKEY}.txt", f"{FIXTURE_PUBKEY}.key"]
        )
        self.assertEqual(sorted(self.out_dir.iterdir()), sorted(paths))


class ConvertTest(unittest.TestCase):
    def test_writes_the_key_beside_the_input(self):
        with tempfile.TemporaryDirectory() as tmp:
            src = Path(tmp) / "in.hex"
            src.write_text(bytes(json.loads(FIXTURE.read_text())).hex())
            args = mock.Mock(keypair=str(src), out_dir=None)
            [path] = grind_key.cmd_convert(args)
            self.assertEqual(path, src.resolve().parent / f"{FIXTURE_PUBKEY}.key")
            self.assertTrue(path.exists())


if __name__ == "__main__":
    unittest.main()
