"""Tests for check_ort_pin.py. Run with: python3 -m unittest discover -s packaging/flatpak -p 'test_*.py'"""

import hashlib
import io
import tarfile
import tempfile
import unittest
from pathlib import Path

import check_ort_pin as pin

URL = "https://cdn.pyke.io/0/pyke:ort-rs/ms@1.31.0/x86_64-unknown-linux-gnu.tar.lzma2"
HASH = "02aac0d3b4927026292088986d7a377c657c88761b1c068be217a67b35447678"
OTHER_HASH = "28903773a7a8fcde725f3f599d406692e947a0c1356e3d39d6dae41d6452f959"

DIST = (
    "target\tfeatures\turl\thash\n"
    "# comments and other targets are skipped\n"
    f"x86_64-pc-windows-msvc\tnone\thttps://cdn.pyke.io/0/pyke:ort-rs/ms@1.31.0/x86_64-pc-windows-msvc.tar.lzma2\t{'1' * 64}\n"
    f"x86_64-unknown-linux-gnu\tnone\t{URL}\t{HASH}\n"
    f"x86_64-unknown-linux-gnu\twebgpu\thttps://cdn.pyke.io/0/pyke:ort-rs/ms@1.31.0/x86_64-unknown-linux-gnu+webgpu.tar.lzma2\t{OTHER_HASH}\n"
    "\n"
)

MANIFEST = f"""modules:
  - name: onnxruntime
    buildsystem: simple
    sources:
      - type: file
        url: {URL}
        sha256: {HASH}
        dest-filename: onnxruntime.tar.lzma2
  - name: repeat-that
    sources:
      - type: file
        url: https://example.org/other.tar.gz
        sha256: {'f' * 64}
"""


def crate(version, dist=DIST):
    """The bytes of `ort-sys-<version>.crate`, a gzipped tar, holding only the file the check reads."""
    out = io.BytesIO()
    with tarfile.open(fileobj=out, mode="w:gz") as tar:
        data = dist.encode()
        info = tarfile.TarInfo(f"ort-sys-{version}/build/download/dist.tsv")
        info.size = len(data)
        tar.addfile(info, io.BytesIO(data))
    return out.getvalue()


def lock(version="2.0.0-rc.15", checksum="0" * 64, extra=""):
    return f"""version = 4

[[package]]
name = "ort"
version = "{version}"

[[package]]
name = "ort-sys"
version = "{version}"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "{checksum}"
{extra}"""


class DistTests(unittest.TestCase):
    def test_the_plain_build_for_x64_linux_is_the_one_wanted(self):
        self.assertEqual(pin.linux_dist(DIST), (URL, HASH))

    def test_a_table_without_that_row_fails(self):
        with self.assertRaises(pin.PinError):
            pin.linux_dist("target\tfeatures\turl\thash\n")


class LockTests(unittest.TestCase):
    def test_the_version_and_checksum_of_ort_sys_come_from_the_lock(self):
        self.assertEqual(pin.ort_sys_from_lock(lock(checksum="a" * 64)), ("2.0.0-rc.15", "a" * 64))

    def test_a_lock_without_ort_sys_fails(self):
        with self.assertRaises(pin.PinError):
            pin.ort_sys_from_lock('version = 4\n\n[[package]]\nname = "ort"\nversion = "1.0.0"\n')


class ManifestTests(unittest.TestCase):
    def test_the_pinned_url_and_hash_are_read_from_the_manifest(self):
        self.assertEqual(pin.pinned_in_manifest(MANIFEST), (URL, HASH))

    def test_a_manifest_without_the_prebuilt_fails(self):
        with self.assertRaises(pin.PinError):
            pin.pinned_in_manifest("modules: []\n")

    def test_two_prebuilts_fail(self):
        with self.assertRaises(pin.PinError):
            pin.pinned_in_manifest(MANIFEST + MANIFEST)


class CheckTests(unittest.TestCase):
    def setUp(self):
        self.archive = crate("2.0.0-rc.15")
        self.checksum = hashlib.sha256(self.archive).hexdigest()

    def check(self, manifest=MANIFEST, lock_text=None, archive=None):
        with tempfile.TemporaryDirectory() as folder:
            folder = Path(folder)
            (folder / "manifest.yml").write_text(manifest, encoding="utf-8")
            (folder / "Cargo.lock").write_text(lock_text or lock(checksum=self.checksum), encoding="utf-8")
            (folder / "ort-sys.crate").write_bytes(self.archive if archive is None else archive)
            return pin.check(folder / "Cargo.lock", folder / "manifest.yml", crate_file=folder / "ort-sys.crate")

    def test_a_manifest_that_matches_the_lock_is_fine(self):
        self.assertEqual(self.check(), "2.0.0-rc.15")

    def test_a_url_of_another_ort_version_fails(self):
        manifest = MANIFEST.replace("ms@1.31.0", "ms@1.30.0")
        with self.assertRaises(pin.PinError) as caught:
            self.check(manifest=manifest)
        self.assertIn("ms@1.30.0", str(caught.exception))
        self.assertIn("ms@1.31.0", str(caught.exception))

    def test_a_hash_that_differs_fails(self):
        with self.assertRaises(pin.PinError) as caught:
            self.check(manifest=MANIFEST.replace(f"sha256: {HASH}", f"sha256: {OTHER_HASH}"))
        self.assertIn(HASH, str(caught.exception))

    def test_a_crate_that_does_not_match_the_locks_checksum_fails(self):
        with self.assertRaises(pin.PinError) as caught:
            self.check(lock_text=lock(checksum="0" * 64))
        self.assertIn("checksum", str(caught.exception))

    def test_a_crate_without_the_table_fails(self):
        archive = crate("2.0.0-rc.15", dist="")
        self.checksum = hashlib.sha256(archive).hexdigest()
        with self.assertRaises(pin.PinError):
            self.check(archive=archive, lock_text=lock(checksum=self.checksum))


if __name__ == "__main__":
    unittest.main()
