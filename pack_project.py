#!/usr/bin/env python3
"""Pack a project into project.zip using only the Python standard library.

Examples:
    python pack_project.py
    python pack_project.py --source /workspace/work/wavechess --output /workspace/project.zip
    python pack_project.py --include-generated

The default source is the current directory. Hidden project files, reports,
scripts, and dist/ are included. Generated directories listed in GENERATED are
omitted unless --include-generated is supplied. Git metadata and credential/
session directories listed in PRIVATE are always omitted. Directory symlinks
are not followed; file symlinks outside the source are omitted. The output ZIP
and its temporary staging files are never included in themselves.

The printed Markdown link is a local relative file link. Creating a ZIP cannot
by itself attach a file to a chat or provide a hosted download URL.
"""

from __future__ import annotations

import argparse
import hashlib
import os
from pathlib import Path
import sys
import tempfile
from urllib.parse import quote
import zipfile


PRIVATE = {".git", ".aws", ".ssh", ".codex"}
GENERATED = {
    "target", "node_modules", ".venv", "venv", "__pycache__",
    ".pytest_cache", ".mypy_cache", ".ruff_cache",
}


def project_files(source: Path, output: Path, include_generated: bool):
    ignored = PRIVATE if include_generated else PRIVATE | GENERATED
    temporary_prefix = f".{output.name}."
    for directory, dirs, files in os.walk(source, followlinks=False):
        current = Path(directory)
        dirs[:] = sorted(
            name for name in dirs
            if name not in ignored and not (current / name).is_symlink()
        )
        for name in sorted(files):
            path = current / name
            if name in PRIVATE:
                continue
            if name.startswith(temporary_prefix) and name.endswith(".tmp"):
                continue
            resolved = path.resolve()
            if resolved == output or not resolved.is_relative_to(source):
                continue
            if not path.is_file():
                continue
            yield path


def pack(source: Path, output: Path, include_generated: bool = False) -> tuple[int, str]:
    source = source.resolve(strict=True)
    if not source.is_dir():
        raise ValueError(f"Source is not a directory: {source}")
    output = output.resolve()
    if output.exists() and not output.is_file():
        raise ValueError(f"Output is not a file: {output}")
    output.parent.mkdir(parents=True, exist_ok=True)
    files = list(project_files(source, output, include_generated))
    if not files:
        raise ValueError(f"No project files found in {source}")

    descriptor, temporary_name = tempfile.mkstemp(
        prefix=f".{output.name}.", suffix=".tmp", dir=output.parent
    )
    os.close(descriptor)
    temporary = Path(temporary_name)
    try:
        with zipfile.ZipFile(
            temporary, "w", compression=zipfile.ZIP_DEFLATED,
            compresslevel=9, allowZip64=True, strict_timestamps=False,
        ) as archive:
            for path in files:
                archive.write(path, arcname=path.relative_to(source).as_posix())
        with zipfile.ZipFile(temporary, "r") as archive:
            damaged = archive.testzip()
            if damaged is not None:
                raise zipfile.BadZipFile(f"CRC check failed: {damaged}")
        digest = hashlib.sha256()
        with temporary.open("rb") as stream:
            for chunk in iter(lambda: stream.read(1024 * 1024), b""):
                digest.update(chunk)
        os.replace(temporary, output)
        return len(files), digest.hexdigest()
    finally:
        temporary.unlink(missing_ok=True)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--source", type=Path, default=Path.cwd(),
                        help="Project directory (default: current directory)")
    parser.add_argument("--output", type=Path,
                        help="Archive path (default: SOURCE/project.zip)")
    parser.add_argument("--include-generated", action="store_true",
                        help="Include target, virtual environments, and other generated directories")
    args = parser.parse_args()
    source = args.source.absolute()
    output = args.output.absolute() if args.output else source / "project.zip"
    try:
        count, digest = pack(source, output, args.include_generated)
    except (OSError, ValueError, zipfile.BadZipFile) as error:
        print(f"Archive creation failed: {error}", file=sys.stderr)
        return 1
    print(f"Archive: {output.resolve()}")
    print(f"Files: {count}")
    print(f"Bytes: {output.stat().st_size}")
    print(f"SHA256: {digest}")
    relative = os.path.relpath(output.resolve(), Path.cwd())
    label = output.name.replace("[", r"\[").replace("]", r"\]")
    print("Local Markdown link (not a hosted chat attachment):")
    print(f"[{label}]({quote(Path(relative).as_posix(), safe='/')})")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
