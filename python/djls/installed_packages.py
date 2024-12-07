from __future__ import annotations

import importlib.metadata
import json
from typing import Dict
from typing import Optional
from typing import TypedDict


class Package(TypedDict):
    name: str
    version: str
    location: Optional[str]


def get_installed_packages() -> Dict[str, Package]:
    packages: Dict[str, Package] = {}
    for dist in importlib.metadata.distributions():
        try:
            location_path = dist.locate_file("")
            location = location_path.parent.as_posix() if location_path else None

            packages[dist.metadata["Name"]] = {
                "name": dist.metadata["Name"],
                "version": dist.version,
                "location": location,
            }
        except Exception:
            continue
    return packages


if __name__ == "__main__":
    print(json.dumps({"installed_packages": get_installed_packages()}))
