from __future__ import annotations

import importlib.metadata
import inspect
import os
import subprocess
import sys
import sysconfig
import traceback
from collections.abc import Awaitable
from collections.abc import Coroutine
from functools import wraps
from typing import Any
from typing import Callable
from typing import TypeVar
from typing import cast

import django
from django.apps import apps
from google.protobuf.message import Message

from .proto.v1 import commands_pb2
from .proto.v1 import django_pb2
from .proto.v1 import messages_pb2
from .proto.v1 import python_pb2

T = TypeVar("T", bound=Message)
R = TypeVar("R", bound=Message)

handlers: dict[str, Callable[[Message], Coroutine[Any, Any, Message]]] = {}


def proto_handler(
    request_type: type[T],
    error: messages_pb2.Error | None = None,
) -> Callable[
    [Callable[[T], R] | Callable[[T], Awaitable[R]]],
    Callable[[T], Coroutine[Any, Any, R]],
]:
    for req_field in messages_pb2.Request.DESCRIPTOR.fields:
        if req_field.message_type == request_type.DESCRIPTOR:
            command_name = req_field.name
            # Find corresponding response type
            for resp_field in messages_pb2.Response.DESCRIPTOR.fields:
                if resp_field.name == command_name:
                    response_type = resp_field.message_type._concrete_class
                    break
            else:
                raise ValueError(f"No response type found for {request_type}")
            break
    else:
        raise ValueError(f"Message type {request_type} not found in Request message")

    def decorator(
        func: Callable[[T], R] | Callable[[T], Awaitable[R]],
    ) -> Callable[[T], Coroutine[Any, Any, R]]:
        is_async = inspect.iscoroutinefunction(func)

        @wraps(func)
        async def wrapper(request: T) -> R:
            try:
                if is_async:
                    result = await cast(Callable[[T], Awaitable[R]], func)(request)
                else:
                    result = cast(Callable[[T], R], func)(request)
                # Runtime type checking
                if not isinstance(result, response_type):
                    raise TypeError(
                        f"Handler returned {type(result)}, expected {response_type}"
                    )
                return result
            except Exception as e:
                if error:
                    err = error
                else:
                    err = messages_pb2.Error(
                        code=messages_pb2.Error.PYTHON_ERROR,
                        message=str(e),
                        traceback=traceback.format_exc(),
                    )
                return cast(R, messages_pb2.Response(error=err))

        handlers[command_name] = wrapper  # pyright: ignore[reportArgumentType]

        return wrapper

    return decorator


@proto_handler(commands_pb2.Check.HealthRequest)
async def check__health(
    _request: commands_pb2.Check.HealthRequest,
) -> commands_pb2.Check.HealthResponse:
    return commands_pb2.Check.HealthResponse(passed=True)


@proto_handler(commands_pb2.Check.GeoDjangoPrereqsRequest)
async def check__geodjango_prereqs(
    request: commands_pb2.Check.GeoDjangoPrereqsRequest,
) -> commands_pb2.Check.GeoDjangoPrereqsResponse:
    has_geodjango = apps.is_installed("django.contrib.gis")

    try:
        gdal_process = subprocess.run(
            ["gdalinfo", "--version"], capture_output=True, check=False
        )
        gdal_is_installed = gdal_process.returncode == 0
    except FileNotFoundError:
        gdal_is_installed = False

    return commands_pb2.Check.GeoDjangoPrereqsResponse(
        passed=(not has_geodjango) or gdal_is_installed
    )


@proto_handler(commands_pb2.Python.GetEnvironmentRequest)
async def python__get_environment(
    _request: commands_pb2.Python.GetEnvironmentRequest,
) -> commands_pb2.Python.GetEnvironmentResponse:
    packages = {}
    for dist in importlib.metadata.distributions():
        try:
            requires = []
            try:
                requires = list(dist.requires) if hasattr(dist, "requires") else []
            except Exception:
                pass

            location = None
            try:
                location = str(dist._path) if hasattr(dist, "_path") else None
            except Exception:
                pass

            packages[dist.metadata["Name"]] = python_pb2.Package(
                dist_name=dist.metadata["Name"],
                dist_version=dist.metadata["Version"],
                dist_location=location,
                dist_requires=requires,
                dist_requires_python=dist.metadata.get("Requires-Python"),
                dist_entry_points=str(dist.entry_points)
                if hasattr(dist, "entry_points")
                else None,
            )
        except Exception:
            continue

    sysconfig_paths = sysconfig.get_paths()

    version_info = python_pb2.VersionInfo(
        major=sys.version_info.major,
        minor=sys.version_info.minor,
        micro=sys.version_info.micro,
        releaselevel={
            "alpha": python_pb2.ReleaseLevel.ALPHA,
            "beta": python_pb2.ReleaseLevel.BETA,
            "candidate": python_pb2.ReleaseLevel.CANDIDATE,
            "final": python_pb2.ReleaseLevel.FINAL,
        }[sys.version_info.releaselevel],
        serial=sys.version_info.serial,
    )

    return commands_pb2.Python.GetEnvironmentResponse(
        python=python_pb2.Python(
            os=python_pb2.Os(environ={k: v for k, v in os.environ.items()}),
            site=python_pb2.Site(packages=packages),
            sys=python_pb2.Sys(
                debug_build=hasattr(sys, "gettotalrefcount"),
                dev_mode=sys.flags.dev_mode,
                is_venv=sys.prefix != sys.base_prefix,
                abiflags=sys.abiflags,
                base_prefix=sys.base_prefix,
                default_encoding=sys.getdefaultencoding(),
                executable=sys.executable,
                filesystem_encoding=sys.getfilesystemencoding(),
                implementation_name=sys.implementation.name,
                platform=sys.platform,
                prefix=sys.prefix,
                builtin_module_names=list(sys.builtin_module_names),
                dll_paths=sys.path if sys.platform == "win32" else [],
                path=sys.path,
                version_info=version_info,
            ),
            sysconfig=python_pb2.Sysconfig(
                data=sysconfig_paths.get("data", ""),
                include=sysconfig_paths.get("include", ""),
                platinclude=sysconfig_paths.get("platinclude", ""),
                platlib=sysconfig_paths.get("platlib", ""),
                platstdlib=sysconfig_paths.get("platstdlib", ""),
                purelib=sysconfig_paths.get("purelib", ""),
                scripts=sysconfig_paths.get("scripts", ""),
                stdlib=sysconfig_paths.get("stdlib", ""),
            ),
        )
    )


@proto_handler(commands_pb2.Django.GetProjectInfoRequest)
async def django__get_project_info(
    _request: commands_pb2.Django.GetProjectInfoRequest,
) -> commands_pb2.Django.GetProjectInfoResponse:
    return commands_pb2.Django.GetProjectInfoResponse(
        project=django_pb2.Project(version=django.__version__)
    )
