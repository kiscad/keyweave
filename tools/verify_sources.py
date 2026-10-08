#!/usr/bin/env python3
"""Check source/fixture provenance without modifying the upstream repository."""
import hashlib
import json
from pathlib import Path
import subprocess

PROJECT = Path(__file__).resolve().parents[1]
manifest = json.loads((PROJECT / 'docs/source-manifest.json').read_text())
source = (PROJECT / 'docs' / manifest['source_root']).resolve()
head = subprocess.check_output(['git', '-C', str(source), 'rev-parse', 'HEAD'], text=True).strip()
if head != manifest['head']:
    raise SystemExit(f"Source HEAD differs: {head}")
for entry in manifest['files']:
    if hashlib.sha256((source / entry['path']).read_bytes()).hexdigest() != entry['sha256']:
        raise SystemExit(f"Source changed: {entry['path']}")
fixture_manifest = PROJECT / 'tests/fixtures/upstream/manifest.json'
key_manifest = PROJECT / 'docs/key-names-manifest.json'
if key_manifest.exists():
    for entry in json.loads(key_manifest.read_text())['files']:
        if hashlib.sha256((source / entry['path']).read_bytes()).hexdigest() != entry['sha256']:
            raise SystemExit(f"HID source changed: {entry['path']}")
if fixture_manifest.exists():
    for entry in json.loads(fixture_manifest.read_text())['files']:
        copied = fixture_manifest.parent / entry['path']
        original = source / entry['source_path']
        if copied.read_bytes() != original.read_bytes():
            raise SystemExit(f"Fixture differs from source: {entry['path']}")
        if hashlib.sha256(copied.read_bytes()).hexdigest() != entry['sha256']:
            raise SystemExit(f"Fixture hash differs: {entry['path']}")
print(f"Verified HEAD and {len(manifest['files'])} source hashes; fixture copies verified")
