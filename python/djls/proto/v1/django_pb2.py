# WARNING: This file is generated by protobuf. DO NOT EDIT!
# Any changes made to this file will be overwritten when the protobuf files are regenerated.
# Source: v1/django.proto

# -*- coding: utf-8 -*-
# Generated by the protocol buffer compiler.  DO NOT EDIT!
# NO CHECKED-IN PROTOBUF GENCODE
# source: v1/django.proto
# Protobuf Python Version: 5.29.1
"""Generated protocol buffer code."""
from google.protobuf import descriptor as _descriptor
from google.protobuf import descriptor_pool as _descriptor_pool
from google.protobuf import runtime_version as _runtime_version
from google.protobuf import symbol_database as _symbol_database
from google.protobuf.internal import builder as _builder
_runtime_version.ValidateProtobufRuntimeVersion(
    _runtime_version.Domain.PUBLIC,
    5,
    29,
    1,
    '',
    'v1/django.proto'
)
# @@protoc_insertion_point(imports)

_sym_db = _symbol_database.Default()




DESCRIPTOR = _descriptor_pool.Default().AddSerializedFile(b'\n\x0fv1/django.proto\x12\x0e\x64jls.v1.django\"\x1a\n\x07Project\x12\x0f\n\x07version\x18\x03 \x01(\t\"\x17\n\x15GetProjectInfoRequest\"B\n\x16GetProjectInfoResponse\x12(\n\x07project\x18\x01 \x01(\x0b\x32\x17.djls.v1.django.Projectb\x06proto3')

_globals = globals()
_builder.BuildMessageAndEnumDescriptors(DESCRIPTOR, _globals)
_builder.BuildTopDescriptorsAndMessages(DESCRIPTOR, 'v1.django_pb2', _globals)
if not _descriptor._USE_C_DESCRIPTORS:
  DESCRIPTOR._loaded_options = None
  _globals['_PROJECT']._serialized_start=35
  _globals['_PROJECT']._serialized_end=61
  _globals['_GETPROJECTINFOREQUEST']._serialized_start=63
  _globals['_GETPROJECTINFOREQUEST']._serialized_end=86
  _globals['_GETPROJECTINFORESPONSE']._serialized_start=88
  _globals['_GETPROJECTINFORESPONSE']._serialized_end=154
# @@protoc_insertion_point(module_scope)
