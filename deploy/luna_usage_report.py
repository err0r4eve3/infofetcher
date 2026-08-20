#!/usr/bin/env python3
"""Report token usage recorded by the Luna-only CPA gateway."""

from __future__ import annotations

import argparse
import json
import sqlite3
from datetime import datetime, timedelta, timezone
from pathlib import Path
from typing import Any


def row_dict(row: sqlite3.Row) -> dict[str, Any]:
    return {key: row[key] for key in row.keys()}


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--database",
        type=Path,
        default=Path("/var/lib/cliproxyapi/luna-usage.sqlite3"),
    )
    parser.add_argument("--since-hours", type=float, default=24.0)
    parser.add_argument("--recent", type=int, default=10)
    args = parser.parse_args()

    if args.since_hours <= 0:
        parser.error("--since-hours must be greater than zero")
    if args.recent < 0:
        parser.error("--recent must be zero or greater")

    since = datetime.now(timezone.utc) - timedelta(hours=args.since_hours)
    since_at = since.isoformat(timespec="milliseconds")
    connection = sqlite3.connect(f"file:{args.database}?mode=ro", uri=True)
    connection.row_factory = sqlite3.Row

    totals = connection.execute(
        """
        SELECT count(*) AS requests,
               coalesce(sum(success), 0) AS successful_requests,
               coalesce(sum(CASE WHEN success = 0 THEN 1 ELSE 0 END), 0)
                   AS failed_requests,
               coalesce(sum(input_tokens), 0) AS input_tokens,
               coalesce(sum(cached_input_tokens), 0) AS cached_input_tokens,
               coalesce(sum(output_tokens), 0) AS output_tokens,
               coalesce(sum(reasoning_tokens), 0) AS reasoning_tokens,
               coalesce(sum(total_tokens), 0) AS total_tokens
          FROM requests
         WHERE started_at >= ?
        """,
        (since_at,),
    ).fetchone()

    recent = connection.execute(
        """
        SELECT request_id, started_at, duration_ms, model, http_status, success,
               input_tokens, cached_input_tokens, output_tokens,
               reasoning_tokens, total_tokens, error_type
          FROM requests
         WHERE started_at >= ?
         ORDER BY id DESC
         LIMIT ?
        """,
        (since_at, args.recent),
    ).fetchall()

    print(
        json.dumps(
            {
                "since": since_at,
                "generated_at": datetime.now(timezone.utc).isoformat(
                    timespec="milliseconds"
                ),
                "totals": row_dict(totals),
                "recent": [row_dict(row) for row in recent],
            },
            ensure_ascii=True,
            indent=2,
        )
    )


if __name__ == "__main__":
    main()
