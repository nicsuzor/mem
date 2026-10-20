#!/usr/bin/env python3
"""Move PKB notes with status 'archived' into the sessions repo and remove them from the PKB."""

import argparse
import os
import re
import shutil
import sys
from pathlib import Path


def extract_frontmatter_status(content: str) -> str | None:
    """Extract status value from YAML frontmatter if present."""
    if not content.startswith("---"):
        return None
    end_marker = content.find("\n---", 3)
    if end_marker == -1:
        return None
    fm_block = content[3:end_marker]
    for line in fm_block.splitlines():
        m = re.match(r"^\s*status:\s*['\"]?([a-zA-Z0-9_\-]+)['\"]?", line)
        if m:
            return m.group(1).strip().lower()
    return None


def is_archived_note(file_path: Path) -> bool:
    """Return True if the markdown note has status: archived in frontmatter."""
    try:
        with open(file_path, "r", encoding="utf-8", errors="replace") as f:
            content = f.read()
    except (OSError, UnicodeDecodeError):
        return False
    return extract_frontmatter_status(content) == "archived"


def archive_notes(pkb_root: Path, sessions_repo: Path, dry_run: bool = False, verbose: bool = False) -> int:
    """Find and move archived notes from PKB to sessions repo. Returns count of moved notes."""
    pkb_root = pkb_root.resolve()
    sessions_repo = sessions_repo.resolve()
    archived_dest_root = sessions_repo / "archived"

    if not pkb_root.is_dir():
        print(f"Error: PKB root directory does not exist: {pkb_root}", file=sys.stderr)
        return 1

    archived_files: list[Path] = []
    # Walk PKB root, ignoring hidden dirs like .git
    for root, dirs, files in os.walk(pkb_root):
        dirs[:] = [d for d in dirs if not d.startswith(".")]
        for file in files:
            if file.endswith(".md"):
                full_path = Path(root) / file
                if is_archived_note(full_path):
                    archived_files.append(full_path)

    moved_count = 0
    for src_path in archived_files:
        rel_path = src_path.relative_to(pkb_root)
        dest_path = archived_dest_root / rel_path

        if verbose or dry_run:
            prefix = "[DRY RUN] Would move" if dry_run else "Moving"
            print(f"{prefix}: {rel_path} -> {dest_path}")

        if not dry_run:
            dest_path.parent.mkdir(parents=True, exist_ok=True)
            shutil.move(str(src_path), str(dest_path))

        moved_count += 1

    action = "Would move" if dry_run else "Moved"
    print(f"{action} {moved_count} archived note(s) from {pkb_root} to {archived_dest_root}")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Move PKB notes with status 'archived' into the sessions repo and remove from PKB."
    )
    parser.add_argument(
        "--pkb-root",
        type=Path,
        default=Path(os.environ.get("ACA_DATA", os.path.expanduser("~/brain"))),
        help="Path to PKB root directory (default: $ACA_DATA or ~/brain)",
    )
    parser.add_argument(
        "--sessions-repo",
        type=Path,
        default=Path(os.environ.get("SESSIONS_REPO", "/data/sessions")),
        help="Path to sessions repository root (default: $SESSIONS_REPO or /data/sessions)",
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="Preview files that would be moved without modifying anything",
    )
    parser.add_argument(
        "--verbose",
        "-v",
        action="store_true",
        help="Print detailed progress for each file",
    )

    args = parser.parse_args()
    return archive_notes(args.pkb_root, args.sessions_repo, dry_run=args.dry_run, verbose=args.verbose)


if __name__ == "__main__":
    sys.exit(main())
