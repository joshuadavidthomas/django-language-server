/// Nominal identities for the small builtin and standard-library surface used
/// by static Python evaluation. They travel through ordinary Python bindings so
/// aliases, branches, shadowing, and mutation contamination follow the same
/// rules as every other value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum PythonIntrinsic {
    BuiltinsModule,
    BuiltinsStrType,
    PathlibModule,
    PathlibPathType,
    OsModule,
    OsPathModule,
    OsPathJoinFunction,
    OsPathDirnameFunction,
    OsPathAbspathFunction,
    OsEnvironObject,
    OsEnvironGetFunction,
    OsGetenvFunction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PythonIntrinsicCall {
    BuiltinsStr,
    PathlibPath,
    OsPathJoin,
    OsPathDirname,
    OsPathAbspath,
    EnvironmentRead,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum PythonIntrinsicNamespace {
    Builtins,
    Pathlib,
    Os,
}

impl PythonIntrinsic {
    pub(crate) fn unbound(name: &str) -> Option<Self> {
        match name {
            "str" => Some(Self::BuiltinsStrType),
            _ => None,
        }
    }

    pub(crate) fn from_direct_import(requested: &str, binds_root: bool) -> Option<Self> {
        match (requested, binds_root) {
            ("builtins", _) => Some(Self::BuiltinsModule),
            ("pathlib", _) => Some(Self::PathlibModule),
            ("os", _) | ("os.path", true) => Some(Self::OsModule),
            ("os.path", false) => Some(Self::OsPathModule),
            _ => None,
        }
    }

    pub(crate) fn is_known_external_module(level: u32, module: Option<&str>) -> bool {
        level == 0 && matches!(module, Some("builtins" | "pathlib" | "os" | "os.path"))
    }

    pub(crate) fn from_named_import(
        level: u32,
        module: Option<&str>,
        member: &str,
    ) -> Option<Self> {
        if !Self::is_known_external_module(level, module) {
            return None;
        }
        match (module, member) {
            (Some("builtins"), "str") => Some(Self::BuiltinsStrType),
            (Some("pathlib"), "Path") => Some(Self::PathlibPathType),
            (Some("os"), "path") => Some(Self::OsPathModule),
            (Some("os"), "environ") => Some(Self::OsEnvironObject),
            (Some("os"), "getenv") => Some(Self::OsGetenvFunction),
            (Some("os.path"), "join") => Some(Self::OsPathJoinFunction),
            (Some("os.path"), "dirname") => Some(Self::OsPathDirnameFunction),
            (Some("os.path"), "abspath") => Some(Self::OsPathAbspathFunction),
            _ => None,
        }
    }

    pub(crate) const fn mutable_namespace(self) -> PythonIntrinsicNamespace {
        match self {
            Self::BuiltinsModule | Self::BuiltinsStrType => PythonIntrinsicNamespace::Builtins,
            Self::PathlibModule | Self::PathlibPathType => PythonIntrinsicNamespace::Pathlib,
            Self::OsModule
            | Self::OsPathModule
            | Self::OsPathJoinFunction
            | Self::OsPathDirnameFunction
            | Self::OsPathAbspathFunction
            | Self::OsEnvironObject
            | Self::OsEnvironGetFunction
            | Self::OsGetenvFunction => PythonIntrinsicNamespace::Os,
        }
    }

    pub(crate) fn member(self, name: &str) -> Option<Self> {
        match (self, name) {
            (Self::BuiltinsModule, "str") => Some(Self::BuiltinsStrType),
            (Self::PathlibModule, "Path") => Some(Self::PathlibPathType),
            (Self::OsModule, "path") => Some(Self::OsPathModule),
            (Self::OsModule, "environ") => Some(Self::OsEnvironObject),
            (Self::OsModule, "getenv") => Some(Self::OsGetenvFunction),
            (Self::OsPathModule, "join") => Some(Self::OsPathJoinFunction),
            (Self::OsPathModule, "dirname") => Some(Self::OsPathDirnameFunction),
            (Self::OsPathModule, "abspath") => Some(Self::OsPathAbspathFunction),
            (Self::OsEnvironObject, "get") => Some(Self::OsEnvironGetFunction),
            (
                Self::BuiltinsModule
                | Self::BuiltinsStrType
                | Self::PathlibModule
                | Self::PathlibPathType
                | Self::OsModule
                | Self::OsPathModule
                | Self::OsPathJoinFunction
                | Self::OsPathDirnameFunction
                | Self::OsPathAbspathFunction
                | Self::OsEnvironObject
                | Self::OsEnvironGetFunction
                | Self::OsGetenvFunction,
                _,
            ) => None,
        }
    }

    pub(crate) const fn call(self) -> Option<PythonIntrinsicCall> {
        match self {
            Self::BuiltinsStrType => Some(PythonIntrinsicCall::BuiltinsStr),
            Self::PathlibPathType => Some(PythonIntrinsicCall::PathlibPath),
            Self::OsPathJoinFunction => Some(PythonIntrinsicCall::OsPathJoin),
            Self::OsPathDirnameFunction => Some(PythonIntrinsicCall::OsPathDirname),
            Self::OsPathAbspathFunction => Some(PythonIntrinsicCall::OsPathAbspath),
            Self::OsEnvironGetFunction | Self::OsGetenvFunction => {
                Some(PythonIntrinsicCall::EnvironmentRead)
            }
            Self::BuiltinsModule
            | Self::PathlibModule
            | Self::OsModule
            | Self::OsPathModule
            | Self::OsEnvironObject => None,
        }
    }
}
