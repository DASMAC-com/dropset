# cspell:word keygen
"""Tests for grind_key's base58 encoder and key-file writer.

The fixture is the committed localnet throwaway ``AAAA.json``, never a
real key.
"""

import json
import stat
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


def b58decode(s: str) -> bytes:
    n = 0
    for c in s:
        n = n * 58 + grind_key.ALPHABET.index(c)
    body = n.to_bytes((n.bit_length() + 7) // 8, "big")
    pad = len(s) - len(s.lstrip("1"))
    return b"\0" * pad + body


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


if __name__ == "__main__":
    unittest.main()
