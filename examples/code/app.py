#!/usr/bin/env python3
"""Python example for syntax highlighting demo."""

from dataclasses import dataclass
from pathlib import Path


@dataclass
class FileEntry:
    name: str
    size: int
    is_dir: bool = False


def list_dir(path: Path) -> list[FileEntry]:
    entries = []
    for child in sorted(path.iterdir()):
        stat = child.stat()
        entries.append(FileEntry(child.name, stat.st_size, child.is_dir()))
    return entries


if __name__ == "__main__":
    for entry in list_dir(Path.cwd()):
        icon = "📁" if entry.is_dir else "📄"
        print(f"{icon} {entry.name} ({entry.size} bytes)")
