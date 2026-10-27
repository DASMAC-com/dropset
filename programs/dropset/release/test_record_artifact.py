"""Tests for record_artifact's ELF check and provenance writer.

The fixtures are synthetic ELF64 headers, never a built program.
"""

import hashlib
import json
import sys
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))

import record_artifact  # noqa: E402


def elf(e_flags: int, body: bytes = b"\0" * 64) -> bytes:
    """A 64-byte ELF64 LE header carrying ``e_flags``, then ``body``."""
    header = bytearray(64)
    header[:4] = record_artifact.ELF_MAGIC
    header[4] = record_artifact.ELFCLASS64
    header[5] = record_artifact.ELFDATA2LSB
    off = record_artifact.E_FLAGS_OFF
    header[off : off + 4] = e_flags.to_bytes(4, "little")
    return bytes(header) + body


class ReadEFlags(unittest.TestCase):
    def test_reads_v3(self):
        self.assertEqual(record_artifact.read_e_flags(elf(0x3)), 0x3)

    def test_reads_v0(self):
        self.assertEqual(record_artifact.read_e_flags(elf(0x0)), 0x0)

    def test_rejects_bad_magic(self):
        with self.assertRaisesRegex(record_artifact.ArtifactError, "magic"):
            record_artifact.read_e_flags(b"\0" * 64)

    def test_rejects_short_file(self):
        with self.assertRaisesRegex(record_artifact.ArtifactError, "shorter"):
            record_artifact.read_e_flags(record_artifact.ELF_MAGIC)

    def test_rejects_32_bit(self):
        header = bytearray(elf(0x3))
        header[4] = 1
        with self.assertRaisesRegex(record_artifact.ArtifactError, "64-bit"):
            record_artifact.read_e_flags(bytes(header))


class Record(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.dir = Path(self.tmp.name)

    def tearDown(self):
        self.tmp.cleanup()

    def test_writes_hash_and_provenance(self):
        data = elf(0x3, b"program bytes")
        so = self.dir / "dropset_mainnet.so"
        so.write_bytes(data)
        manifest = record_artifact.record(so, "abc123", False)
        digest = hashlib.sha256(data).hexdigest()
        self.assertEqual(manifest["sha256"], digest)
        self.assertEqual(manifest["e_flags"], "0x3")
        self.assertEqual(manifest["bytes"], len(data))
        self.assertEqual(
            (self.dir / "dropset_mainnet.so.sha256").read_text(),
            f"{digest}  dropset_mainnet.so\n",
        )
        prov = json.loads((self.dir / "dropset_mainnet.so.provenance.json").read_text())
        self.assertEqual(prov, manifest)
        self.assertEqual(prov["commit"], "abc123")
        self.assertFalse(prov["dirty"])

    def test_refuses_v0_and_writes_nothing(self):
        so = self.dir / "dropset_mainnet.so"
        so.write_bytes(elf(0x0))
        with self.assertRaisesRegex(record_artifact.ArtifactError, "0x0"):
            record_artifact.record(so, "abc123", False)
        self.assertFalse((self.dir / "dropset_mainnet.so.sha256").exists())

    def test_main_exits_nonzero_on_wrong_arch(self):
        so = self.dir / "dropset_mainnet.so"
        so.write_bytes(elf(0x0))
        self.assertEqual(record_artifact.main([str(so)]), 1)


if __name__ == "__main__":
    unittest.main()
