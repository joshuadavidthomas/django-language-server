# WARNING: This file is generated by protobuf. DO NOT EDIT!
# Any changes made to this file will be overwritten when the protobuf files are regenerated.
# Source: v1/python.proto

# -*- coding: utf-8 -*-
# Generated by the protocol buffer compiler.  DO NOT EDIT!
# NO CHECKED-IN PROTOBUF GENCODE
# source: v1/python.proto
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
    'v1/python.proto'
)
# @@protoc_insertion_point(imports)

_sym_db = _symbol_database.Default()




DESCRIPTOR = _descriptor_pool.Default().AddSerializedFile(b'\n\x0fv1/python.proto\x12\x0e\x64jls.v1.python\"\x9c\x01\n\x06Python\x12\x1e\n\x02os\x18\x01 \x01(\x0b\x32\x12.djls.v1.python.Os\x12\"\n\x04site\x18\x02 \x01(\x0b\x32\x14.djls.v1.python.Site\x12 \n\x03sys\x18\x03 \x01(\x0b\x32\x13.djls.v1.python.Sys\x12,\n\tsysconfig\x18\x04 \x01(\x0b\x32\x19.djls.v1.python.Sysconfig\"f\n\x02Os\x12\x30\n\x07\x65nviron\x18\x01 \x03(\x0b\x32\x1f.djls.v1.python.Os.EnvironEntry\x1a.\n\x0c\x45nvironEntry\x12\x0b\n\x03key\x18\x01 \x01(\t\x12\r\n\x05value\x18\x02 \x01(\t:\x02\x38\x01\"\x86\x01\n\x04Site\x12\x34\n\x08packages\x18\x01 \x03(\x0b\x32\".djls.v1.python.Site.PackagesEntry\x1aH\n\rPackagesEntry\x12\x0b\n\x03key\x18\x01 \x01(\t\x12&\n\x05value\x18\x02 \x01(\x0b\x32\x17.djls.v1.python.Package:\x02\x38\x01\"\xe0\x02\n\x03Sys\x12\x13\n\x0b\x64\x65\x62ug_build\x18\x01 \x01(\x08\x12\x10\n\x08\x64\x65v_mode\x18\x02 \x01(\x08\x12\x0f\n\x07is_venv\x18\x03 \x01(\x08\x12\x10\n\x08\x61\x62iflags\x18\x04 \x01(\t\x12\x13\n\x0b\x62\x61se_prefix\x18\x05 \x01(\t\x12\x18\n\x10\x64\x65\x66\x61ult_encoding\x18\x06 \x01(\t\x12\x12\n\nexecutable\x18\x07 \x01(\t\x12\x1b\n\x13\x66ilesystem_encoding\x18\x08 \x01(\t\x12\x1b\n\x13implementation_name\x18\t \x01(\t\x12\x10\n\x08platform\x18\n \x01(\t\x12\x0e\n\x06prefix\x18\x0b \x01(\t\x12\x1c\n\x14\x62uiltin_module_names\x18\x0c \x03(\t\x12\x11\n\tdll_paths\x18\r \x03(\t\x12\x0c\n\x04path\x18\x0e \x03(\t\x12\x31\n\x0cversion_info\x18\x0f \x01(\x0b\x32\x1b.djls.v1.python.VersionInfo\"~\n\x0bVersionInfo\x12\r\n\x05major\x18\x01 \x01(\r\x12\r\n\x05minor\x18\x02 \x01(\r\x12\r\n\x05micro\x18\x03 \x01(\r\x12\x32\n\x0creleaselevel\x18\x04 \x01(\x0e\x32\x1c.djls.v1.python.ReleaseLevel\x12\x0e\n\x06serial\x18\x05 \x01(\r\"\x96\x01\n\tSysconfig\x12\x0c\n\x04\x64\x61ta\x18\x01 \x01(\t\x12\x0f\n\x07include\x18\x02 \x01(\t\x12\x13\n\x0bplatinclude\x18\x03 \x01(\t\x12\x0f\n\x07platlib\x18\x04 \x01(\t\x12\x12\n\nplatstdlib\x18\x05 \x01(\t\x12\x0f\n\x07purelib\x18\x06 \x01(\t\x12\x0f\n\x07scripts\x18\x07 \x01(\t\x12\x0e\n\x06stdlib\x18\x08 \x01(\t\"\x97\x02\n\x07Package\x12\x11\n\tdist_name\x18\x01 \x01(\t\x12\x14\n\x0c\x64ist_version\x18\x02 \x01(\t\x12\x1a\n\rdist_editable\x18\x03 \x01(\x08H\x00\x88\x01\x01\x12\x1e\n\x11\x64ist_entry_points\x18\x04 \x01(\tH\x01\x88\x01\x01\x12\x1a\n\rdist_location\x18\x05 \x01(\tH\x02\x88\x01\x01\x12\x15\n\rdist_requires\x18\x06 \x03(\t\x12!\n\x14\x64ist_requires_python\x18\x07 \x01(\tH\x03\x88\x01\x01\x42\x10\n\x0e_dist_editableB\x14\n\x12_dist_entry_pointsB\x10\n\x0e_dist_locationB\x17\n\x15_dist_requires_python\"\x17\n\x15GetEnvironmentRequest\"@\n\x16GetEnvironmentResponse\x12&\n\x06python\x18\x01 \x01(\x0b\x32\x16.djls.v1.python.Python*=\n\x0cReleaseLevel\x12\t\n\x05\x41LPHA\x10\x00\x12\x08\n\x04\x42\x45TA\x10\x01\x12\r\n\tCANDIDATE\x10\x02\x12\t\n\x05\x46INAL\x10\x03\x62\x06proto3')

_globals = globals()
_builder.BuildMessageAndEnumDescriptors(DESCRIPTOR, _globals)
_builder.BuildTopDescriptorsAndMessages(DESCRIPTOR, 'v1.python_pb2', _globals)
if not _descriptor._USE_C_DESCRIPTORS:
  DESCRIPTOR._loaded_options = None
  _globals['_OS_ENVIRONENTRY']._loaded_options = None
  _globals['_OS_ENVIRONENTRY']._serialized_options = b'8\001'
  _globals['_SITE_PACKAGESENTRY']._loaded_options = None
  _globals['_SITE_PACKAGESENTRY']._serialized_options = b'8\001'
  _globals['_RELEASELEVEL']._serialized_start=1444
  _globals['_RELEASELEVEL']._serialized_end=1505
  _globals['_PYTHON']._serialized_start=36
  _globals['_PYTHON']._serialized_end=192
  _globals['_OS']._serialized_start=194
  _globals['_OS']._serialized_end=296
  _globals['_OS_ENVIRONENTRY']._serialized_start=250
  _globals['_OS_ENVIRONENTRY']._serialized_end=296
  _globals['_SITE']._serialized_start=299
  _globals['_SITE']._serialized_end=433
  _globals['_SITE_PACKAGESENTRY']._serialized_start=361
  _globals['_SITE_PACKAGESENTRY']._serialized_end=433
  _globals['_SYS']._serialized_start=436
  _globals['_SYS']._serialized_end=788
  _globals['_VERSIONINFO']._serialized_start=790
  _globals['_VERSIONINFO']._serialized_end=916
  _globals['_SYSCONFIG']._serialized_start=919
  _globals['_SYSCONFIG']._serialized_end=1069
  _globals['_PACKAGE']._serialized_start=1072
  _globals['_PACKAGE']._serialized_end=1351
  _globals['_GETENVIRONMENTREQUEST']._serialized_start=1353
  _globals['_GETENVIRONMENTREQUEST']._serialized_end=1376
  _globals['_GETENVIRONMENTRESPONSE']._serialized_start=1378
  _globals['_GETENVIRONMENTRESPONSE']._serialized_end=1442
# @@protoc_insertion_point(module_scope)
