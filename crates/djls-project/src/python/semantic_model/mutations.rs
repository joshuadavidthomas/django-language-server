#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PythonMutationAccess {
    Index(usize),
    Key(String),
}
