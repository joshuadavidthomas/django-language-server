from __future__ import annotations

import json
import sys

# When running from zipapp, we need to import without the package prefix
# since the zipapp root is already the package
try:
    # Try direct import (when running as zipapp)
    from inspector import DjlsResponse
    from inspector import handle_request
except ImportError:
    # Fall back to package import (when running with python -m)
    from djls_inspector.inspector import DjlsResponse
    from djls_inspector.inspector import handle_request


def main() -> None:
    try:
        for line in sys.stdin:
            line = line.strip()
            if not line:
                continue

            try:
                request = json.loads(line)
                response = handle_request(request)
            except json.JSONDecodeError as e:
                response = DjlsResponse(ok=False, error=f"Invalid JSON: {e}")
            except Exception as e:
                response = DjlsResponse(ok=False, error=f"Unexpected error: {e}")

            response_json = json.dumps(response.to_dict())
            print(response_json, flush=True)

    except KeyboardInterrupt:
        sys.exit(0)
    except Exception as e:
        print(f"Fatal error in inspector: {e}", file=sys.stderr)
        sys.exit(1)


if __name__ == "__main__":
    main()
