#!/usr/bin/env python3
"""Convert RFC3339 / ISO-8601 timestamps to Unix epoch seconds (UTC). Prints 0 on failure."""
from __future__ import annotations

import sys
from datetime import datetime, timezone


def main() -> None:
    if len(sys.argv) < 2:
        print(0)
        return
    s = sys.argv[1].strip()
    if not s:
        print(0)
        return
    if s.endswith("Z"):
        s = s[:-1] + "+00:00"
    try:
        dt = datetime.fromisoformat(s)
    except ValueError:
        print(0)
        return
    if dt.tzinfo is None:
        dt = dt.replace(tzinfo=timezone.utc)
    print(int(dt.timestamp()))


if __name__ == "__main__":
    main()
