# WARNING: This file is generated by protobuf. DO NOT EDIT!
# Any changes made to this file will be overwritten when the protobuf files are regenerated.
# Source: v1/django.proto

from google.protobuf import descriptor as _descriptor
from google.protobuf import message as _message
from typing import ClassVar as _ClassVar, Mapping as _Mapping, Optional as _Optional, Union as _Union

DESCRIPTOR: _descriptor.FileDescriptor

class Project(_message.Message):
    __slots__ = ("version",)
    VERSION_FIELD_NUMBER: _ClassVar[int]
    version: str
    def __init__(self, version: _Optional[str] = ...) -> None: ...

class GetProjectInfoRequest(_message.Message):
    __slots__ = ()
    def __init__(self) -> None: ...

class GetProjectInfoResponse(_message.Message):
    __slots__ = ("project",)
    PROJECT_FIELD_NUMBER: _ClassVar[int]
    project: Project
    def __init__(self, project: _Optional[_Union[Project, _Mapping]] = ...) -> None: ...
