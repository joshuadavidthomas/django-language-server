# WARNING: This file is generated by protobuf. DO NOT EDIT!
# Any changes made to this file will be overwritten when the protobuf files are regenerated.
# Source: v1/commands.proto

from . import django_pb2 as _django_pb2
from . import python_pb2 as _python_pb2
from google.protobuf import descriptor as _descriptor
from google.protobuf import message as _message
from typing import ClassVar as _ClassVar, Mapping as _Mapping, Optional as _Optional, Union as _Union

DESCRIPTOR: _descriptor.FileDescriptor

class Check(_message.Message):
    __slots__ = ()
    class HealthRequest(_message.Message):
        __slots__ = ()
        def __init__(self) -> None: ...
    class HealthResponse(_message.Message):
        __slots__ = ("passed", "error")
        PASSED_FIELD_NUMBER: _ClassVar[int]
        ERROR_FIELD_NUMBER: _ClassVar[int]
        passed: bool
        error: str
        def __init__(self, passed: bool = ..., error: _Optional[str] = ...) -> None: ...
    class GeoDjangoPrereqsRequest(_message.Message):
        __slots__ = ()
        def __init__(self) -> None: ...
    class GeoDjangoPrereqsResponse(_message.Message):
        __slots__ = ("passed", "error")
        PASSED_FIELD_NUMBER: _ClassVar[int]
        ERROR_FIELD_NUMBER: _ClassVar[int]
        passed: bool
        error: str
        def __init__(self, passed: bool = ..., error: _Optional[str] = ...) -> None: ...
    def __init__(self) -> None: ...

class Python(_message.Message):
    __slots__ = ()
    class GetEnvironmentRequest(_message.Message):
        __slots__ = ()
        def __init__(self) -> None: ...
    class GetEnvironmentResponse(_message.Message):
        __slots__ = ("python",)
        PYTHON_FIELD_NUMBER: _ClassVar[int]
        python: _python_pb2.Python
        def __init__(self, python: _Optional[_Union[_python_pb2.Python, _Mapping]] = ...) -> None: ...
    def __init__(self) -> None: ...

class Django(_message.Message):
    __slots__ = ()
    class GetProjectInfoRequest(_message.Message):
        __slots__ = ()
        def __init__(self) -> None: ...
    class GetProjectInfoResponse(_message.Message):
        __slots__ = ("project",)
        PROJECT_FIELD_NUMBER: _ClassVar[int]
        project: _django_pb2.Project
        def __init__(self, project: _Optional[_Union[_django_pb2.Project, _Mapping]] = ...) -> None: ...
    def __init__(self) -> None: ...