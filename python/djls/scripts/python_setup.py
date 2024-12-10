from __future__ import annotations

import importlib.metadata
import json
import sys
import sysconfig
from typing import Dict
from typing import List
from typing import Optional
from typing import TypedDict


def get_version_info():
    version_parts = sys.version.split()[0].split(".")
    patch_and_suffix = version_parts[2]
    for i, c in enumerate(patch_and_suffix):
        if not c.isdigit():
            patch = patch_and_suffix[:i]
            suffix = patch_and_suffix[i:]
            break
    else:
        patch = patch_and_suffix
        suffix = None

    return {
        "major": int(version_parts[0]),
        "minor": int(version_parts[1]),
        "patch": int(patch),
        "suffix": suffix,
    }


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


def get_python_info() -> (
    Dict[
        str,
        str
        | Dict[str, str]
        | List[str]
        | Dict[str, Package]
        | Dict[str, int | str | None],
    ]
):
    return {
        "version_info": get_version_info(),
        "sysconfig_paths": sysconfig.get_paths(),
        "sys_prefix": sys.prefix,
        "sys_base_prefix": sys.base_prefix,
        "sys_executable": sys.executable,
        "sys_path": [p for p in sys.path if p],
        "packages": get_installed_packages(),
    }


if __name__ == "__main__":
    print(json.dumps(get_python_info()))
