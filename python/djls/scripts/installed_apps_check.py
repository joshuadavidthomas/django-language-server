from __future__ import annotations

import json
import sys

from django.conf import settings

if __name__ == "__main__":
    print(json.dumps({"has_app": sys.argv[1] in settings.INSTALLED_APPS}))
