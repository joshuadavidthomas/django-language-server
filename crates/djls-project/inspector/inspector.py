from __future__ import annotations

from dataclasses import asdict
from dataclasses import dataclass
from typing import Any

try:
    # Try direct import (when running as zipapp)
    from queries import Query
    from queries import QueryData
    from queries import get_installed_templatetags
    from queries import get_python_environment_info
    from queries import get_template_dirs
    from queries import initialize_django
except ImportError:
    # Fall back to relative import (when running with python -m)
    from .queries import Query
    from .queries import QueryData
    from .queries import get_installed_templatetags
    from .queries import get_python_environment_info
    from .queries import get_template_dirs
    from .queries import initialize_django


@dataclass
class DjlsRequest:
    query: Query
    args: list[str] | None = None


@dataclass
class DjlsResponse:
    ok: bool
    data: QueryData | None = None
    error: str | None = None

    def to_dict(self) -> dict[str, Any]:
        d = asdict(self)
        # Convert Path objects to strings for JSON serialization
        if self.data:
            if hasattr(self.data, "__dataclass_fields__"):
                data_dict = asdict(self.data)
                # Convert Path objects to strings
                for key, value in data_dict.items():
                    # Handle single Path objects
                    if hasattr(value, "__fspath__"):  # Path-like object
                        data_dict[key] = str(value)
                    # Handle lists of Path objects
                    elif (
                        isinstance(value, list)
                        and value
                        and hasattr(value[0], "__fspath__")
                    ):
                        data_dict[key] = [str(p) for p in value]
                    # Handle optional Path objects (could be None)
                    elif value is None:
                        pass  # Keep None as is
                d["data"] = data_dict
        return d


def handle_request(request: dict[str, Any]) -> DjlsResponse:
    try:
        query_str = request.get("query")
        if not query_str:
            return DjlsResponse(ok=False, error="Missing 'query' field in request")

        try:
            query = Query(query_str)
        except ValueError:
            return DjlsResponse(ok=False, error=f"Unknown query type: {query_str}")

        args = request.get("args")

        if query == Query.DJANGO_INIT:
            success, error = initialize_django()
            return DjlsResponse(ok=success, data=None, error=error)

        elif query == Query.PYTHON_ENV:
            return DjlsResponse(ok=True, data=get_python_environment_info())

        elif query == Query.TEMPLATE_DIRS:
            return DjlsResponse(ok=True, data=get_template_dirs())

        elif query == Query.TEMPLATETAGS:
            return DjlsResponse(ok=True, data=get_installed_templatetags())

        return DjlsResponse(ok=False, error=f"Unhandled query type: {query}")

    except Exception as e:
        return DjlsResponse(ok=False, error=str(e))
