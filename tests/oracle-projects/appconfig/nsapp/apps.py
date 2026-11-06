from __future__ import annotations

from pathlib import Path

from django.apps import AppConfig


class Custom(AppConfig):
    name = "nsapp"
    path = str(Path(__file__).resolve().parents[1] / "vendor" / "nsapp")
