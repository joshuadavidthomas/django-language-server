use camino::Utf8PathBuf;
use djls_source::Origin;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PythonValue {
    pub(super) kind: PythonValueKind,
    pub(super) origin: Origin,
    pub(super) completeness: PythonCompleteness,
}

impl PythonValue {
    pub(super) fn new(
        kind: PythonValueKind,
        origin: Origin,
        completeness: PythonCompleteness,
    ) -> Self {
        Self {
            kind,
            origin,
            completeness,
        }
    }

    pub(super) fn full(kind: PythonValueKind, origin: Origin) -> Self {
        Self::new(kind, origin, PythonCompleteness::Full)
    }

    pub(super) fn partial(kind: PythonValueKind, origin: Origin) -> Self {
        Self::new(kind, origin, PythonCompleteness::Partial)
    }

    pub(super) fn unknown(origin: Origin) -> Self {
        Self::partial(PythonValueKind::Unknown, origin)
    }

    pub(crate) fn kind(&self) -> &PythonValueKind {
        &self.kind
    }

    pub(crate) fn origin(&self) -> Origin {
        self.origin
    }

    pub(crate) fn is_complete(&self) -> bool {
        self.completeness == PythonCompleteness::Full
    }

    pub(crate) fn completeness(&self) -> PythonCompleteness {
        self.completeness
    }

    pub(super) fn mark_partial(&mut self) {
        self.completeness = PythonCompleteness::Partial;
    }

    pub(super) fn semantically_eq(&self, other: &Self) -> bool {
        self.completeness == other.completeness
            && match (&self.kind, &other.kind) {
                (PythonValueKind::Str(left), PythonValueKind::Str(right)) => {
                    left == right && self.origin.file == other.origin.file
                }
                _ => self.kind.semantically_eq(&other.kind),
            }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PythonValueKind {
    Str(String),
    Bool(bool),
    List(Vec<PythonValue>),
    Dict(PythonDict),
    Path(Utf8PathBuf),
    Unknown,
}

impl PythonValueKind {
    pub(super) fn semantically_eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Str(left), Self::Str(right)) => left == right,
            (Self::Bool(left), Self::Bool(right)) => left == right,
            (Self::List(left), Self::List(right)) => {
                left.len() == right.len()
                    && left
                        .iter()
                        .zip(right)
                        .all(|(left, right)| left.semantically_eq(right))
            }
            (Self::Dict(left), Self::Dict(right)) => left.semantically_eq(right),
            (Self::Path(left), Self::Path(right)) => left == right,
            (Self::Unknown, Self::Unknown) => true,
            _ => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PythonDict {
    pub(super) entries: Vec<PythonDictEntry>,
}

impl PythonDict {
    pub(crate) fn entries(&self) -> &[PythonDictEntry] {
        &self.entries
    }

    pub(super) fn semantically_eq(&self, other: &Self) -> bool {
        self.entries.len() == other.entries.len()
            && self
                .entries
                .iter()
                .zip(&other.entries)
                .all(|(left, right)| left.semantically_eq(right))
    }

    pub(super) fn get_string_key_mut(&mut self, key: &str) -> Option<&mut PythonValue> {
        self.entries.iter_mut().find_map(|entry| {
            if matches!(entry.key.kind(), PythonValueKind::Str(candidate) if candidate == key) {
                Some(&mut entry.value)
            } else {
                None
            }
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PythonDictEntry {
    pub(super) key: PythonValue,
    pub(super) value: PythonValue,
}

impl PythonDictEntry {
    pub(crate) fn key(&self) -> &PythonValue {
        &self.key
    }

    pub(crate) fn value(&self) -> &PythonValue {
        &self.value
    }

    pub(super) fn semantically_eq(&self, other: &Self) -> bool {
        self.key.semantically_eq(&other.key) && self.value.semantically_eq(&other.value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PythonCompleteness {
    Full,
    Partial,
}
