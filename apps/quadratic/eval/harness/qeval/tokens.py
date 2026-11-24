from __future__ import annotations

import json
from functools import cache
from pathlib import Path

from qeval.sandbox import CACHE


@cache
def _encoding():
    import os

    os.environ.setdefault("TIKTOKEN_CACHE_DIR", str(CACHE / "tiktoken"))
    import tiktoken

    return tiktoken.get_encoding("o200k_base")


def count(text: str) -> int:
    return len(_encoding().encode(text, disallowed_special=()))


def compact(value) -> str:
    return json.dumps(value, separators=(",", ":"), ensure_ascii=False)


def count_tools(tools_list: dict) -> int:
    """Compact JSON in o200k_base, the way lific's README counts its own tools/list."""
    return count(compact(tools_list))
