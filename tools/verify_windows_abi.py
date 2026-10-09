#!/usr/bin/env python3
"""Compile the real Windows SDK oracle. No hook/input API calls or executable.

Requires Windows, VS Build Tools with the C++ workload and Windows SDK.
vswhere locates vcvarsall; it is independent of the VS version/edition.
"""
import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile


def cmd_path(path: Path) -> str:
    value = str(path)
    # cmd.exe expands percent even inside quotes. Reject rather than interpolating
    # unsafe path text into the batch. Quoted ampersands/spaces remain literal;
    # delayed expansion is explicitly disabled below.
    if any(character in value for character in '%!^"\r\n'):
        raise ValueError("SDK probe path contains unsupported cmd.exe metacharacters")
    return f'"{value}"'


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--arch", choices=("x64", "x86"), required=True)
    args = parser.parse_args()
    pointer_size = 8 if args.arch == "x64" else 4
    if os.name != "nt":
        parser.error("real Windows SDK/MSVC required; this probe cannot run on Linux")
    program_files = os.environ.get("ProgramFiles(x86)")
    if not program_files:
        raise RuntimeError("ProgramFiles(x86) is absent; cannot locate vswhere")
    vswhere = Path(program_files) / "Microsoft Visual Studio/Installer/vswhere.exe"
    result = subprocess.run(
        [str(vswhere), "-latest", "-products", "*", "-requires",
         "Microsoft.VisualStudio.Component.VC.Tools.x86.x64", "-format", "json", "-utf8"],
        check=True, capture_output=True, encoding="utf-8-sig", timeout=30,
    )
    installations = json.loads(result.stdout)
    if len(installations) != 1:
        raise RuntimeError("a usable Visual Studio C++ installation was not found")
    vcvars = Path(installations[0]["installationPath"]) / "VC/Auxiliary/Build/vcvarsall.bat"
    if not vcvars.is_file():
        raise RuntimeError("vcvarsall.bat is absent")
    source = Path(__file__).resolve().with_name("windows_input_abi.c")
    if not source.is_file():
        raise RuntimeError("SDK oracle source is absent")
    with tempfile.TemporaryDirectory(prefix="keyweave-windows-abi-") as directory:
        root = Path(directory)
        batch = root / "probe.cmd"
        # Only trusted discovered paths and the two fixed arch choices enter the
        # batch. /Zs performs syntax/type checking with no link or execution.
        batch.write_text(
            "@echo off\nchcp 65001 >nul\nsetlocal DisableDelayedExpansion\n"
            f"call {cmd_path(vcvars)} {args.arch}\n"
            "if errorlevel 1 exit /b %errorlevel%\n"
            f"cl.exe /nologo /W4 /WX /std:c11 /Zs /DKEYWEAVE_EXPECT_PTR={pointer_size} {cmd_path(source)}\n"
            "exit /b %errorlevel%\n",
            encoding="utf-8",
        )
        # A fixed command name avoids cmd parsing any dynamic directory text.
        subprocess.run(["cmd.exe", "/d", "/c", "probe.cmd"], cwd=root, check=True, timeout=60)
    print(f"Windows SDK {args.arch} ABI/constants/prototypes: passed (compile only)")
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (OSError, ValueError, RuntimeError, subprocess.SubprocessError) as error:
        print(f"Windows SDK ABI probe failed: {error}", file=sys.stderr)
        sys.exit(1)
