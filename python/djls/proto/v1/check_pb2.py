# WARNING: This file is generated by protobuf. DO NOT EDIT!
# Any changes made to this file will be overwritten when the protobuf files are regenerated.
# Source: v1/check.proto

# -*- coding: utf-8 -*-
# Generated by the protocol buffer compiler.  DO NOT EDIT!
# NO CHECKED-IN PROTOBUF GENCODE
# source: v1/check.proto
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
    'v1/check.proto'
)
# @@protoc_insertion_point(imports)

_sym_db = _symbol_database.Default()




DESCRIPTOR = _descriptor_pool.Default().AddSerializedFile(b'\n\x0ev1/check.proto\x12\rdjls.v1.check\"\x0f\n\rHealthRequest\">\n\x0eHealthResponse\x12\x0e\n\x06passed\x18\x01 \x01(\x08\x12\x12\n\x05\x65rror\x18\x02 \x01(\tH\x00\x88\x01\x01\x42\x08\n\x06_error\"\x19\n\x17GeoDjangoPrereqsRequest\"H\n\x18GeoDjangoPrereqsResponse\x12\x0e\n\x06passed\x18\x01 \x01(\x08\x12\x12\n\x05\x65rror\x18\x02 \x01(\tH\x00\x88\x01\x01\x42\x08\n\x06_errorb\x06proto3')

_globals = globals()
_builder.BuildMessageAndEnumDescriptors(DESCRIPTOR, _globals)
_builder.BuildTopDescriptorsAndMessages(DESCRIPTOR, 'v1.check_pb2', _globals)
if not _descriptor._USE_C_DESCRIPTORS:
  DESCRIPTOR._loaded_options = None
  _globals['_HEALTHREQUEST']._serialized_start=33
  _globals['_HEALTHREQUEST']._serialized_end=48
  _globals['_HEALTHRESPONSE']._serialized_start=50
  _globals['_HEALTHRESPONSE']._serialized_end=112
  _globals['_GEODJANGOPREREQSREQUEST']._serialized_start=114
  _globals['_GEODJANGOPREREQSREQUEST']._serialized_end=139
  _globals['_GEODJANGOPREREQSRESPONSE']._serialized_start=141
  _globals['_GEODJANGOPREREQSRESPONSE']._serialized_end=213
# @@protoc_insertion_point(module_scope)
