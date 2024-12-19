# handlers.py
from __future__ import annotations

import inspect
from collections.abc import Awaitable
from functools import wraps
from typing import Any
from typing import Callable
from typing import TypeVar
from typing import cast

from pydantic import BaseModel

from . import __version__
from .schema import ErrorResponse
from .schema import HealthCheck
from .schema import HealthCheckResponse
from .schema import Messages
from .schema import Response

T = TypeVar("T", bound=BaseModel | None)
R = TypeVar("R", bound=BaseModel)

handlers: dict[Messages, Callable[[Any | None], Awaitable[Response]]] = {}


def handler(
    message: Messages,
) -> Callable[
    [Callable[..., R | Awaitable[R]]], Callable[[Any | None], Awaitable[Response]]
]:
    def decorator(
        func: Callable[..., R | Awaitable[R]],
    ) -> Callable[[Any | None], Awaitable[Response]]:
        is_async = inspect.iscoroutinefunction(func)
        params = inspect.signature(func).parameters

        @wraps(func)
        async def wrapper(data: Any | None) -> Response:
            try:
                if is_async:
                    if params:
                        result = await cast(Callable[[Any], Awaitable[R]], func)(data)
                    else:
                        result = await cast(Callable[[], Awaitable[R]], func)()
                else:
                    if params:
                        result = cast(Callable[[Any], R], func)(data)
                    else:
                        result = cast(Callable[[], R], func)()
                return result
            except Exception as e:
                return Response(
                    data={},
                    error=ErrorResponse(
                        code="python_error",
                        message=str(e),
                    ),
                    message=message,
                    success=False,
                )

        handlers[message] = wrapper
        return wrapper

    return decorator


@handler(Messages.HEALTH_CHECK)
async def check_health() -> HealthCheckResponse:
    check = HealthCheck(status="OK", version=__version__)
    logger.debug(f"{check=}")
    return HealthCheckResponse(data=check, message=Messages.HEALTH_CHECK, success=True)
