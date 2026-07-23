use camino::Utf8Component;
use camino::Utf8Path;
use camino::Utf8PathBuf;

/// A path value is either an exact path object or a nominal intrinsic used to
/// construct and transform paths. Keeping both under one owner makes callers
/// distinguish concrete path data from the small supported standard-library
/// surface explicitly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PythonPath {
    Object(Utf8PathBuf),
    Intrinsic(PythonPathIntrinsic),
}

impl PythonPath {
    pub(crate) fn object(path: Utf8PathBuf) -> Self {
        Self::Object(path)
    }

    pub(crate) fn from_absolute_string(value: &str) -> Option<Self> {
        let path = Utf8Path::new(value);
        path.is_absolute().then(|| Self::Object(path.to_path_buf()))
    }

    pub(crate) fn intrinsic(intrinsic: PythonPathIntrinsic) -> Self {
        Self::Intrinsic(intrinsic)
    }

    pub(crate) fn object_path(&self) -> Option<&Utf8Path> {
        match self {
            Self::Object(path) => Some(path),
            Self::Intrinsic(_) => None,
        }
    }

    pub(crate) fn parent(&self) -> Option<Self> {
        let path = self.object_path()?;
        let parent = path.parent().unwrap_or_else(|| Utf8Path::new("/"));
        Some(Self::Object(parent.to_path_buf()))
    }

    pub(crate) fn join(&self, segment: &str) -> Option<Self> {
        Some(Self::Object(self.object_path()?.join(segment)))
    }

    pub(crate) fn resolve(&self) -> Option<Self> {
        let path = self.object_path()?;
        if !path.is_absolute() {
            return None;
        }

        let mut resolved = Utf8PathBuf::new();
        for component in path.components() {
            match component {
                Utf8Component::Prefix(prefix) => resolved.push(prefix.as_str()),
                Utf8Component::RootDir => resolved.push(Utf8Path::new("/")),
                Utf8Component::CurDir => {}
                Utf8Component::ParentDir => {
                    resolved.pop();
                }
                Utf8Component::Normal(component) => resolved.push(component),
            }
        }
        Some(Self::Object(resolved))
    }
}

/// Nominal identities for the small standard-library surface used by static
/// path evaluation. They travel through ordinary Python bindings so aliases,
/// branches, and shadowing follow the same rules as every other value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum PythonPathNamespace {
    Builtins,
    Pathlib,
    Os,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum PythonPathIntrinsic {
    BuiltinsModule,
    BuiltinStrType,
    PathlibModule,
    PathlibPathType,
    OsModule,
    OsPathModule,
    OsPathJoinFunction,
    OsPathDirnameFunction,
    OsPathAbspathFunction,
}

impl PythonPathIntrinsic {
    pub(crate) fn unbound_intrinsic(name: &str) -> Option<Self> {
        match name {
            "str" => Some(Self::BuiltinStrType),
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
            (Some("builtins"), "str") => Some(Self::BuiltinStrType),
            (Some("pathlib"), "Path") => Some(Self::PathlibPathType),
            (Some("os"), "path") => Some(Self::OsPathModule),
            (Some("os.path"), "join") => Some(Self::OsPathJoinFunction),
            (Some("os.path"), "dirname") => Some(Self::OsPathDirnameFunction),
            (Some("os.path"), "abspath") => Some(Self::OsPathAbspathFunction),
            _ => None,
        }
    }

    pub(crate) fn mutable_namespace(self) -> PythonPathNamespace {
        match self {
            Self::BuiltinsModule | Self::BuiltinStrType => PythonPathNamespace::Builtins,
            Self::PathlibModule | Self::PathlibPathType => PythonPathNamespace::Pathlib,
            Self::OsModule
            | Self::OsPathModule
            | Self::OsPathJoinFunction
            | Self::OsPathDirnameFunction
            | Self::OsPathAbspathFunction => PythonPathNamespace::Os,
        }
    }

    pub(crate) fn member(self, name: &str) -> Option<Self> {
        match (self, name) {
            (Self::BuiltinsModule, "str") => Some(Self::BuiltinStrType),
            (Self::PathlibModule, "Path") => Some(Self::PathlibPathType),
            (Self::OsModule, "path") => Some(Self::OsPathModule),
            (Self::OsPathModule, "join") => Some(Self::OsPathJoinFunction),
            (Self::OsPathModule, "dirname") => Some(Self::OsPathDirnameFunction),
            (Self::OsPathModule, "abspath") => Some(Self::OsPathAbspathFunction),
            _ => None,
        }
    }

    pub(crate) const fn structural_rank(self) -> u8 {
        match self {
            Self::BuiltinsModule => 0,
            Self::BuiltinStrType => 1,
            Self::PathlibModule => 2,
            Self::PathlibPathType => 3,
            Self::OsModule => 4,
            Self::OsPathModule => 5,
            Self::OsPathJoinFunction => 6,
            Self::OsPathDirnameFunction => 7,
            Self::OsPathAbspathFunction => 8,
        }
    }
}
