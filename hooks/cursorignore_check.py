#!/usr/bin/env python3
"""Guardian: detect paths that should usually be in .cursorignore (stdlib only)."""
from __future__ import annotations

import argparse
import fnmatch
import json
import os
import sys
from pathlib import Path
from typing import Optional


def load_json(path: Path) -> dict:
    with path.open(encoding="utf-8") as f:
        return json.load(f)


def find_workspace_root(file_path: Path, roots: Optional[list]) -> Optional[Path]:
    if roots:
        abs_file = file_path.resolve()
        for r in roots:
            rp = Path(r).resolve()
            try:
                abs_file.relative_to(rp)
                return rp
            except ValueError:
                continue
    p = file_path.resolve()
    for parent in [p, *p.parents]:
        if (parent / ".cursorignore").is_file() or (parent / ".git").exists():
            return parent
    return p.parent if p.parent != p else None


def parse_ignore_file(path: Path) -> list[str]:
    if not path.is_file():
        return []
    out: list[str] = []
    for line in path.read_text(encoding="utf-8", errors="replace").splitlines():
        s = line.strip()
        if not s or s.startswith("#"):
            continue
        out.append(s)
    return out


def path_matches_cursorignore(rel: str, patterns: list[str]) -> bool:
    rel_norm = rel.replace("\\", "/").lstrip("./")
    for pat in patterns:
        pat = pat.strip()
        if not pat:
            continue
        if fnmatch.fnmatch(rel_norm, pat):
            return True
        if fnmatch.fnmatch("/" + rel_norm, pat):
            return True
        # trailing slash directory patterns
        if pat.endswith("/") and fnmatch.fnmatch(rel_norm + "/", pat):
            return True
    return False


def load_allow_file(workspace: Path) -> list[str]:
    p = workspace / ".guardian" / "cursorignore-allow"
    if not p.is_file():
        return []
    lines: list[str] = []
    for line in p.read_text(encoding="utf-8", errors="replace").splitlines():
        s = line.strip()
        if not s or s.startswith("#"):
            continue
        lines.append(s)
    return lines


def path_matches_allow(rel: str, patterns: list[str]) -> bool:
    rel_norm = rel.replace("\\", "/").lstrip("./")
    for pat in patterns:
        if fnmatch.fnmatch(rel_norm, pat):
            return True
        if fnmatch.fnmatch(os.path.basename(rel_norm), pat):
            return True
    return False


def check_file(
    file_path: str,
    checklist_path: Path,
    workspace_roots: Optional[list],
) -> dict:
    fp = Path(file_path)
    if not fp.is_file() and not fp.exists():
        return {"match": False}

    root = find_workspace_root(fp, workspace_roots)
    if root is None:
        return {"match": False}

    try:
        rel = str(fp.resolve().relative_to(root.resolve()))
    except ValueError:
        return {"match": False}

    ignore_patterns = parse_ignore_file(root / ".cursorignore")
    if path_matches_cursorignore(rel, ignore_patterns):
        return {"match": False}

    allow_patterns = load_allow_file(root)
    if path_matches_allow(rel, allow_patterns):
        return {"match": False}

    data = load_json(checklist_path)
    parts = Path(rel).parts
    for entry in data.get("entries", []):
        seg = entry.get("segment", "")
        if seg and seg in parts:
            return {
                "match": True,
                "segment": seg,
                "rationale": entry.get("rationale", ""),
                "severity": entry.get("severity", "warn"),
                "workspace": str(root),
                "relative_path": rel,
            }
    return {"match": False}


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--file", required=True)
    ap.add_argument("--checklist", required=True)
    ap.add_argument("--workspace-root", action="append", default=None)
    args = ap.parse_args()
    roots = args.workspace_root if args.workspace_root else None
    result = check_file(args.file, Path(args.checklist), roots)
    json.dump(result, sys.stdout)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
