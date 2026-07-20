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
pub(crate) enum PythonPathSymbol {
    BuiltinsModule,
    BuiltinStr,
    ModuleFile,
    PathlibModule,
    PathlibPath,
    OsModule,
    OsPathModule,
    OsPathJoin,
    OsPathDirname,
}

impl PythonPathSymbol {
    pub(crate) fn unbound_intrinsic(name: &str) -> Option<Self> {
        match name {
            "str" => Some(Self::BuiltinStr),
            "__file__" => Some(Self::ModuleFile),
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
            (Some("builtins"), "str") => Some(Self::BuiltinStr),
            (Some("pathlib"), "Path") => Some(Self::PathlibPath),
            (Some("os"), "path") => Some(Self::OsPathModule),
            (Some("os.path"), "join") => Some(Self::OsPathJoin),
            (Some("os.path"), "dirname") => Some(Self::OsPathDirname),
            _ => None,
        }
    }

    pub(crate) fn mutable_namespace(self) -> Option<PythonPathNamespace> {
        match self {
            Self::BuiltinsModule | Self::BuiltinStr => Some(PythonPathNamespace::Builtins),
            Self::PathlibModule | Self::PathlibPath => Some(PythonPathNamespace::Pathlib),
            Self::OsModule | Self::OsPathModule | Self::OsPathJoin | Self::OsPathDirname => {
                Some(PythonPathNamespace::Os)
            }
            Self::ModuleFile => None,
        }
    }

    pub(crate) fn has_mutable_namespace(self) -> bool {
        self.mutable_namespace().is_some()
    }

    pub(crate) fn shares_mutable_namespace(self, other: Self) -> bool {
        self == other
            || self
                .mutable_namespace()
                .zip(other.mutable_namespace())
                .is_some_and(|(left, right)| left == right)
    }

    pub(crate) fn member(self, name: &str) -> Option<Self> {
        match (self, name) {
            (Self::BuiltinsModule, "str") => Some(Self::BuiltinStr),
            (Self::PathlibModule, "Path") => Some(Self::PathlibPath),
            (Self::OsModule, "path") => Some(Self::OsPathModule),
            (Self::OsPathModule, "join") => Some(Self::OsPathJoin),
            (Self::OsPathModule, "dirname") => Some(Self::OsPathDirname),
            _ => None,
        }
    }

    pub(crate) const fn structural_rank(self) -> u8 {
        match self {
            Self::BuiltinsModule => 0,
            Self::BuiltinStr => 1,
            Self::ModuleFile => 2,
            Self::PathlibModule => 3,
            Self::PathlibPath => 4,
            Self::OsModule => 5,
            Self::OsPathModule => 6,
            Self::OsPathJoin => 7,
            Self::OsPathDirname => 8,
        }
    }
}
