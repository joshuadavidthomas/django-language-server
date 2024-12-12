from __future__ import annotations

import importlib.metadata
import os
import sys
import sysconfig
from abc import ABC
from abc import abstractmethod
from typing import ClassVar
from typing import Generic
from typing import TypeVar

from google.protobuf.message import Message

from ._typing import override
from .proto.v1 import check_pb2
from .proto.v1 import django_pb2
from .proto.v1 import python_pb2

Request = TypeVar("Request", bound=Message)
Response = TypeVar("Response", bound=Message)


class Command(ABC, Generic[Request, Response]):
    name: ClassVar[str]
    request: ClassVar[type[Message]]
    response: ClassVar[type[Message]]

    def __init_subclass__(cls) -> None:
        super().__init_subclass__()
        class_vars = ["name", "request", "response"]
        for class_var in class_vars:
            if not hasattr(cls, class_var):
                raise TypeError(
                    f"Command subclass {cls.__name__} must define '{class_var}'"
                )

    @abstractmethod
    def execute(self, request: Request) -> Response: ...


class CheckHealth(Command[check_pb2.HealthRequest, check_pb2.HealthResponse]):
    name = "check__health"
    request = check_pb2.HealthRequest
    response = check_pb2.HealthResponse

    @override
    def execute(self, request: check_pb2.HealthRequest) -> check_pb2.HealthResponse:
        return check_pb2.HealthResponse(passed=True)


class CheckDjangoAvailable(
    Command[check_pb2.DjangoAvailableRequest, check_pb2.DjangoAvailableResponse]
):
    name = "check__django_available"
    request = check_pb2.DjangoAvailableRequest
    response = check_pb2.DjangoAvailableResponse

    @override
    def execute(
        self, request: check_pb2.DjangoAvailableRequest
    ) -> check_pb2.DjangoAvailableResponse:
        try:
            import django

            return check_pb2.DjangoAvailableResponse(passed=True)
        except ImportError:
            return check_pb2.DjangoAvailableResponse(
                passed=False, error="Django is not installed"
            )


class CheckAppInstalled(
    Command[check_pb2.AppInstalledRequest, check_pb2.AppInstalledResponse]
):
    name = "check__app_installed"
    request = check_pb2.AppInstalledRequest
    response = check_pb2.AppInstalledResponse

    @override
    def execute(
        self, request: check_pb2.AppInstalledRequest
    ) -> check_pb2.AppInstalledResponse:
        try:
            from django.apps import apps

            return check_pb2.AppInstalledResponse(
                passed=apps.is_installed(request.app_name)
            )
        except ImportError:
            return check_pb2.AppInstalledResponse(
                passed=False, error="Django is not installed"
            )


class PythonGetEnvironment(
    Command[python_pb2.GetEnvironmentRequest, python_pb2.GetEnvironmentResponse]
):
    name = "python__get_environment"
    request = python_pb2.GetEnvironmentRequest
    response = python_pb2.GetEnvironmentResponse

    @override
    def execute(
        self, request: python_pb2.GetEnvironmentRequest
    ) -> python_pb2.GetEnvironmentResponse:
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

        return python_pb2.GetEnvironmentResponse(
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


class DjangoGetProjectInfo(
    Command[django_pb2.GetProjectInfoRequest, django_pb2.GetProjectInfoResponse]
):
    name = "django__get_project_info"
    request = django_pb2.GetProjectInfoRequest
    response = django_pb2.GetProjectInfoResponse

    @override
    def execute(
        self, request: django_pb2.GetProjectInfoRequest
    ) -> django_pb2.GetProjectInfoResponse:
        import django

        return django_pb2.GetProjectInfoResponse(
            project=django_pb2.Project(version=django.__version__)
        )


COMMANDS = [
    CheckAppInstalled,
    CheckDjangoAvailable,
    CheckHealth,
    PythonGetEnvironment,
    DjangoGetProjectInfo,
]
