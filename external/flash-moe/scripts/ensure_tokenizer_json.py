#!/usr/bin/env python3
"""Place HuggingFace tokenizer.json in out_35b/ so infer can load think token IDs.

Without this file, infer prints a warning and disables think start/end tokens.

Usage:
  FLASH_MOE_TOKENIZER_JSON=/path/to/tokenizer.json python3 ensure_tokenizer_json.py
  python3 ensure_tokenizer_json.py /path/to/tokenizer.json

If out_35b/tokenizer.json already exists, does nothing (exit 0).
"""

from __future__ import annotations

import os
import shutil
import sys
from pathlib import Path


def main() -> int:
    flash_moe_root = Path(__file__).resolve().parent.parent
    destination = flash_moe_root / "out_35b" / "tokenizer.json"

    if destination.is_file():
        return 0

    source = os.environ.get("FLASH_MOE_TOKENIZER_JSON") or (
        sys.argv[1] if len(sys.argv) > 1 else None
    )
    if not source:
        print(
            "Think tokens: out_35b/tokenizer.json is missing. Infer will disable think "
            "token IDs until you add it (copy from the model's HuggingFace tokenizer.json). "
            "Optional: FLASH_MOE_TOKENIZER_JSON=/path/to/tokenizer.json or pass path as argv.",
            file=sys.stderr,
        )
        return 0

    source_path = Path(source).expanduser()
    if not source_path.is_file():
        print(f"Think tokens: not a file: {source_path}", file=sys.stderr)
        return 1

    destination.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(source_path, destination)
    print(f"Think tokens: copied {source_path} -> {destination}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
