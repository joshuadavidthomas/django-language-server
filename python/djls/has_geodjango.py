from __future__ import annotations

import json

from django.conf import settings

if __name__ == "__main__":
    print(
        json.dumps({"has_geodjango": "django.contrib.gis" in settings.INSTALLED_APPS})
    )
