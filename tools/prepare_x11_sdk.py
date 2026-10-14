#!/usr/bin/env python3
"""Prepare pinned official headers offline; never install or execute SDK code."""
import argparse
import hashlib
import json
from pathlib import Path, PurePosixPath
import tarfile


def prepare(archives, output):
    sources = json.loads(Path(__file__).with_name("x11-sdk-sources.json").read_text())
    if output.exists():
        raise ValueError("output must be a new directory")
    headers = {}
    for source in sources:
        archive = archives / source["archive"]
        if archive.stat().st_size > 4 * 1024 * 1024:
            raise ValueError("SDK archive size exceeded")
        if hashlib.sha256(archive.read_bytes()).hexdigest() != source["sha256"]:
            raise ValueError("SDK archive checksum mismatch: " + source["archive"])
        package = source["archive"].removesuffix(".tar.xz")
        with tarfile.open(archive, "r:xz") as tar:
            for member in tar:
                path = PurePosixPath(member.name)
                if not member.name.startswith(package + "/include/X11/") or path.suffix != ".h":
                    continue
                if path.is_absolute() or ".." in path.parts or not member.isfile():
                    raise ValueError("invalid SDK header path/type")
                if member.size > 1024 * 1024 or len(headers) >= 512:
                    raise ValueError("SDK header budget exceeded")
                relative = PurePosixPath(*path.parts[2:])
                if str(relative) in source.get("exclude_headers", []):
                    continue
                with tar.extractfile(member) as stream:
                    contents = stream.read(1024 * 1024 + 1)
                if len(contents) != member.size:
                    raise ValueError("SDK header size mismatch")
                if relative in headers and headers[relative] != contents:
                    raise ValueError("conflicting SDK headers")
                headers[relative] = contents
    if not headers:
        raise ValueError("SDK archives contain no public headers")
    # All archive/header validation finishes before writing any output.
    output.mkdir(parents=True, exist_ok=False)
    for name, contents in headers.items():
        destination = output.joinpath(*name.parts)
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_bytes(contents)
    record = {"archives": sources, "headers": {
        str(name): {"sha256": hashlib.sha256(contents).hexdigest(), "bytes": len(contents)}
        for name, contents in sorted(headers.items())}}
    (output / "sdk-manifest.json").write_text(json.dumps(record, indent=2) + "\n")
    print(f"Prepared {len(headers)} pinned public SDK headers; offline, no installation")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--archives", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    prepare(args.archives.resolve(strict=True), args.output.absolute())
