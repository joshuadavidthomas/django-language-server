# WARNING: This file is generated by protobuf. DO NOT EDIT!
# Any changes made to this file will be overwritten when the protobuf files are regenerated.
# Source: v1/messages.proto

from . import commands_pb2 as _commands_pb2
from google.protobuf.internal import enum_type_wrapper as _enum_type_wrapper
from google.protobuf import descriptor as _descriptor
from google.protobuf import message as _message
from typing import ClassVar as _ClassVar, Mapping as _Mapping, Optional as _Optional, Union as _Union

DESCRIPTOR: _descriptor.FileDescriptor

class Request(_message.Message):
    __slots__ = ("check__health", "check__geodjango_prereqs", "python__get_environment", "django__get_project_info")
    CHECK__HEALTH_FIELD_NUMBER: _ClassVar[int]
    CHECK__GEODJANGO_PREREQS_FIELD_NUMBER: _ClassVar[int]
    PYTHON__GET_ENVIRONMENT_FIELD_NUMBER: _ClassVar[int]
    DJANGO__GET_PROJECT_INFO_FIELD_NUMBER: _ClassVar[int]
    check__health: _commands_pb2.Check.HealthRequest
    check__geodjango_prereqs: _commands_pb2.Check.GeoDjangoPrereqsRequest
    python__get_environment: _commands_pb2.Python.GetEnvironmentRequest
    django__get_project_info: _commands_pb2.Django.GetProjectInfoRequest
    def __init__(self, check__health: _Optional[_Union[_commands_pb2.Check.HealthRequest, _Mapping]] = ..., check__geodjango_prereqs: _Optional[_Union[_commands_pb2.Check.GeoDjangoPrereqsRequest, _Mapping]] = ..., python__get_environment: _Optional[_Union[_commands_pb2.Python.GetEnvironmentRequest, _Mapping]] = ..., django__get_project_info: _Optional[_Union[_commands_pb2.Django.GetProjectInfoRequest, _Mapping]] = ...) -> None: ...

class Response(_message.Message):
    __slots__ = ("check__health", "check__geodjango_prereqs", "python__get_environment", "django__get_project_info", "error")
    CHECK__HEALTH_FIELD_NUMBER: _ClassVar[int]
    CHECK__GEODJANGO_PREREQS_FIELD_NUMBER: _ClassVar[int]
    PYTHON__GET_ENVIRONMENT_FIELD_NUMBER: _ClassVar[int]
    DJANGO__GET_PROJECT_INFO_FIELD_NUMBER: _ClassVar[int]
    ERROR_FIELD_NUMBER: _ClassVar[int]
    check__health: _commands_pb2.Check.HealthResponse
    check__geodjango_prereqs: _commands_pb2.Check.GeoDjangoPrereqsResponse
    python__get_environment: _commands_pb2.Python.GetEnvironmentResponse
    django__get_project_info: _commands_pb2.Django.GetProjectInfoResponse
    error: Error
    def __init__(self, check__health: _Optional[_Union[_commands_pb2.Check.HealthResponse, _Mapping]] = ..., check__geodjango_prereqs: _Optional[_Union[_commands_pb2.Check.GeoDjangoPrereqsResponse, _Mapping]] = ..., python__get_environment: _Optional[_Union[_commands_pb2.Python.GetEnvironmentResponse, _Mapping]] = ..., django__get_project_info: _Optional[_Union[_commands_pb2.Django.GetProjectInfoResponse, _Mapping]] = ..., error: _Optional[_Union[Error, _Mapping]] = ...) -> None: ...

class Error(_message.Message):
    __slots__ = ("code", "message", "traceback")
    class Code(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
        __slots__ = ()
        UNKNOWN: _ClassVar[Error.Code]
        INVALID_REQUEST: _ClassVar[Error.Code]
        PYTHON_ERROR: _ClassVar[Error.Code]
        DJANGO_ERROR: _ClassVar[Error.Code]
    UNKNOWN: Error.Code
    INVALID_REQUEST: Error.Code
    PYTHON_ERROR: Error.Code
    DJANGO_ERROR: Error.Code
    CODE_FIELD_NUMBER: _ClassVar[int]
    MESSAGE_FIELD_NUMBER: _ClassVar[int]
    TRACEBACK_FIELD_NUMBER: _ClassVar[int]
    code: Error.Code
    message: str
    traceback: str
    def __init__(self, code: _Optional[_Union[Error.Code, str]] = ..., message: _Optional[str] = ..., traceback: _Optional[str] = ...) -> None: ...
