# has_import.py
from __future__ import annotations

import json
import sys


def check_import(module: str) -> bool:
    try:
        module_parts = module.split(".")
        current = __import__(module_parts[0])
        for part in module_parts[1:]:
            current = getattr(current, part)
        return True
    except (ImportError, AttributeError):
        return False


if __name__ == "__main__":
    result = {"can_import": check_import(sys.argv[1])}
    print(json.dumps(result))
