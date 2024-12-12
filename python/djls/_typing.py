from __future__ import annotations

import sys

if sys.version_info >= (3, 12):
    from typing import override as typing_override
else:
    from typing_extensions import (
        override as typing_override,  # pyright: ignore[reportUnreachable]
    )

override = typing_override
