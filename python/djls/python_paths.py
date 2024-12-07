from __future__ import annotations

import json
import sys
from typing import Dict
from typing import List


def get_python_paths() -> Dict[str, str | List[str]]:
    return {
        "prefix": sys.prefix,
        "base_prefix": sys.base_prefix,
        "executable": sys.executable,
        "path": [p for p in sys.path if p],
    }


if __name__ == "__main__":
    print(json.dumps({"paths": get_python_paths()}))
