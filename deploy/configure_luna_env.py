#!/usr/bin/env python3
"""Atomically configure an infofetcher environment for the Luna gateway."""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import sys
import tempfile
from datetime import datetime, timezone
from pathlib import Path


UPDATES = {
    "LLM_BASE_URL": "https://luna-api.error.moe/v1",
    "LLM_MODEL": "gpt-5.6-luna",
    "TRANSLATION_BASE_URL": "https://luna-api.error.moe/v1",
    "TRANSLATION_MODEL": "gpt-5.6-luna",
}


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("environment", type=Path)
    args = parser.parse_args()

    api_key = sys.stdin.read().strip()
    if not re.fullmatch(r"[A-Za-z0-9._-]{20,256}", api_key):
        parser.error("stdin did not contain a valid API key")

    path = args.environment.resolve(strict=True)
    stat = path.stat()
    updates = {
        **UPDATES,
        "LLM_API_KEY": api_key,
        "TRANSLATION_API_KEY": api_key,
    }
    seen: set[str] = set()
    output: list[str] = []
    for line in path.read_text().splitlines():
        name = line.split("=", 1)[0].strip() if "=" in line else ""
        if name in updates:
            if name not in seen:
                output.append(f"{name}={updates[name]}")
                seen.add(name)
            continue
        output.append(line)
    for name, value in updates.items():
        if name not in seen:
            output.append(f"{name}={value}")

    timestamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    backup = path.with_name(f"{path.name}.before-luna-{timestamp}")
    shutil.copy2(path, backup)
    os.chown(backup, stat.st_uid, stat.st_gid)

    temporary_name = ""
    try:
        with tempfile.NamedTemporaryFile(
            "w", encoding="utf-8", dir=path.parent, delete=False
        ) as temporary:
            temporary_name = temporary.name
            temporary.write("\n".join(output) + "\n")
            temporary.flush()
            os.fsync(temporary.fileno())
            os.fchmod(temporary.fileno(), stat.st_mode)
            os.fchown(temporary.fileno(), stat.st_uid, stat.st_gid)
        os.replace(temporary_name, path)
    finally:
        if temporary_name and os.path.exists(temporary_name):
            os.unlink(temporary_name)

    print(
        json.dumps(
            {
                "backup": str(backup),
                "LLM_BASE_URL": UPDATES["LLM_BASE_URL"],
                "LLM_MODEL": UPDATES["LLM_MODEL"],
                "LLM_API_KEY": "present",
                "TRANSLATION_BASE_URL": UPDATES["TRANSLATION_BASE_URL"],
                "TRANSLATION_MODEL": UPDATES["TRANSLATION_MODEL"],
                "TRANSLATION_API_KEY": "present",
            },
            separators=(",", ":"),
        )
    )


if __name__ == "__main__":
    main()
