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
    from queries import initialize_django
except ImportError:
    # Fall back to relative import (when running with python -m)
    from .queries import Query
    from .queries import QueryData
    from .queries import get_installed_templatetags
    from .queries import get_python_environment_info
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
                    if key in ["sys_base_prefix", "sys_executable", "sys_prefix"]:
                        if value:
                            data_dict[key] = str(value)
                    elif key == "sys_path":
                        data_dict[key] = [str(p) for p in value]
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

        if query == Query.PYTHON_ENV:
            return DjlsResponse(ok=True, data=get_python_environment_info())

        elif query == Query.TEMPLATETAGS:
            return DjlsResponse(ok=True, data=get_installed_templatetags())

        elif query == Query.DJANGO_INIT:
            return DjlsResponse(ok=True, data=initialize_django())

        return DjlsResponse(ok=False, error=f"Unhandled query type: {query}")

    except Exception as e:
        return DjlsResponse(ok=False, error=str(e))
