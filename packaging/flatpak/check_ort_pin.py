#!/usr/bin/env python3
"""Checks that the prebuilt ONNX Runtime in the Flatpak manifest is the one the `ort` crate in Cargo.lock wants.

The Flatpak build has no network, so it can't let `ort-sys` download its ONNX Runtime the way a normal build does.
The manifest pins that download instead (URL and sha256) and points `ORT_LIB_LOCATION` at it. Nothing ties the pin to
Cargo.lock, though: after an `ort` bump the old archive would still link, and the speech detector would then fail at run
time with a mismatch of ONNX Runtime versions. This script is that tie. It reads the `ort-sys` version and checksum from
Cargo.lock, takes that crate from crates.io (checked against the lock's checksum), and compares the download that
`ort-sys` would have made for x86_64 Linux with the manifest's.

Usage: check_ort_pin.py [--lock Cargo.lock] [--manifest FILE] [--crate-file FILE]
  --crate-file is the `ort-sys-<version>.crate` to use instead of downloading it (tests).

On a mismatch it prints what to put in the manifest and exits 1. Unit tests: test_check_ort_pin.py.
"""

import argparse
import hashlib
import io
import re
import sys
import tarfile
import tomllib
import urllib.request
from pathlib import Path

TARGET = "x86_64-unknown-linux-gnu"
CRATE_URL = "https://static.crates.io/crates/ort-sys/ort-sys-{version}.crate"
# The download is a `file` source of the manifest with the pyke.io URL, whatever else is on the lines around it.
PREBUILT = re.compile(r"^\s*(?:- )?url:\s*(?P<url>https://cdn\.pyke\.io/\S+)\s*\n\s*sha256:\s*(?P<hash>[0-9a-f]{64})\s*$", re.M)


class PinError(Exception):
    """The pin is wrong, or can't be checked."""


def ort_sys_from_lock(text):
    """(version, checksum) of the `ort-sys` package in Cargo.lock."""
    for package in tomllib.loads(text).get("package", []):
        if package.get("name") == "ort-sys":
            if "checksum" not in package:
                raise PinError("ort-sys in Cargo.lock has no checksum: is it not from crates.io?")
            return package["version"], package["checksum"]
    raise PinError("Cargo.lock has no ort-sys package")


def linux_dist(table):
    """(URL, sha256) of the plain x86_64 Linux build in the `dist.tsv` of `ort-sys`."""
    for row in table.splitlines():
        if not row or row.startswith("#"):
            continue
        columns = row.split("\t")
        if len(columns) == 4 and columns[0] == TARGET and columns[1] == "none":
            return columns[2], columns[3]
    raise PinError(f"the dist.tsv of ort-sys has no build for {TARGET} without features")


def pinned_in_manifest(text):
    """(URL, sha256) of the prebuilt ONNX Runtime the manifest downloads."""
    found = [(match["url"], match["hash"]) for match in PREBUILT.finditer(text)]
    if len(found) != 1:
        raise PinError(f"the manifest should download one prebuilt ONNX Runtime from cdn.pyke.io, it has {len(found)}")
    return found[0]


def dist_of_crate(archive, version):
    """The text of `build/download/dist.tsv` inside the gzipped tar `archive` of ort-sys `version`."""
    name = f"ort-sys-{version}/build/download/dist.tsv"
    with tarfile.open(fileobj=io.BytesIO(archive), mode="r:gz") as tar:
        try:
            member = tar.extractfile(name)
        except KeyError:
            member = None
        if member is None:
            raise PinError(f"the ort-sys {version} crate has no {name}")
        return member.read().decode("utf-8")


def fetch_crate(version):
    with urllib.request.urlopen(CRATE_URL.format(version=version), timeout=60) as response:  # noqa: S310 (fixed https URL)
        return response.read()


def check(lock, manifest, crate_file=None):
    """Raises PinError unless the manifest pins what ort-sys wants. Returns the ort-sys version."""
    version, checksum = ort_sys_from_lock(Path(lock).read_text(encoding="utf-8"))
    archive = Path(crate_file).read_bytes() if crate_file else fetch_crate(version)
    if hashlib.sha256(archive).hexdigest() != checksum:
        raise PinError(f"the ort-sys {version} crate does not match the checksum in Cargo.lock")
    wanted_url, wanted_hash = linux_dist(dist_of_crate(archive, version))
    url, digest = pinned_in_manifest(Path(manifest).read_text(encoding="utf-8"))
    if (url, digest) != (wanted_url, wanted_hash):
        raise PinError(
            f"the manifest pins the wrong ONNX Runtime for ort-sys {version}.\n"
            f"  manifest: url {url}\n            sha256 {digest}\n"
            f"  wanted:   url {wanted_url}\n            sha256 {wanted_hash}\n"
            "Change the `onnxruntime` source in the manifest to the wanted ones."
        )
    return version


def main(argv=None):
    here = Path(__file__).resolve().parent
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--lock", default=here.parent.parent / "Cargo.lock", type=Path)
    parser.add_argument("--manifest", default=here / "me.roboroads.RepeatThat.yml", type=Path)
    parser.add_argument("--crate-file", type=Path)
    args = parser.parse_args(argv)
    try:
        version = check(args.lock, args.manifest, args.crate_file)
    except (PinError, OSError, tarfile.TarError, tomllib.TOMLDecodeError) as error:
        print(f"::error::{error}", file=sys.stderr)
        return 1
    print(f"ok: the manifest pins the ONNX Runtime that ort-sys {version} wants")
    return 0


if __name__ == "__main__":
    sys.exit(main())
