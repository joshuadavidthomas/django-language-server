use super::PythonPathNamespace;

/// Nominal identities for the supported operating-system environment surface.
/// They travel through ordinary Python bindings so aliases, branches, and
/// shadowing follow the same rules as every other value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum PythonEnvIntrinsic {
    EnvironObject,
    EnvironGetFunction,
    GetenvFunction,
}

impl PythonEnvIntrinsic {
    pub(crate) fn from_named_import(
        level: u32,
        module: Option<&str>,
        member: &str,
    ) -> Option<Self> {
        match (level, module, member) {
            (0, Some("os"), "environ") => Some(Self::EnvironObject),
            (0, Some("os"), "getenv") => Some(Self::GetenvFunction),
            _ => None,
        }
    }

    pub(crate) fn member(self, name: &str) -> Option<Self> {
        match (self, name) {
            (Self::EnvironObject, "get") => Some(Self::EnvironGetFunction),
            (Self::EnvironObject | Self::EnvironGetFunction | Self::GetenvFunction, _) => None,
        }
    }

    pub(crate) const fn mutable_namespace(self) -> PythonPathNamespace {
        match self {
            Self::EnvironObject | Self::EnvironGetFunction | Self::GetenvFunction => {
                PythonPathNamespace::Os
            }
        }
    }

    pub(crate) const fn structural_rank(self) -> u8 {
        match self {
            Self::EnvironObject => 0,
            Self::EnvironGetFunction => 1,
            Self::GetenvFunction => 2,
        }
    }
}
