use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_source::File;
use djls_source::Origin;
use rustc_hash::FxHashMap;
use serde::Serialize;

use crate::python::PythonPathBindings;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum ParseStatus {
    #[default]
    Parsed,
    Unparseable,
}

impl ParseStatus {
    pub(super) const fn join(self, other: Self) -> Self {
        match (self, other) {
            (Self::Unparseable, _) | (_, Self::Unparseable) => Self::Unparseable,
            (Self::Parsed, Self::Parsed) => Self::Parsed,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PythonSemanticModel {
    pub(super) bindings: PythonBindings,
    pub(super) files_read: Vec<File>,
    pub(super) source_paths: FxHashMap<File, Utf8PathBuf>,
    pub(super) mutations: Vec<PythonMutation>,
    pub(super) status: ParseStatus,
}

impl PythonSemanticModel {
    pub(crate) fn binding(&self, name: &str) -> Option<&PythonBinding> {
        self.bindings.get(name)
    }

    pub(crate) fn files_read(&self) -> &[File] {
        &self.files_read
    }

    pub(crate) fn mutations(&self) -> &[PythonMutation] {
        &self.mutations
    }

    pub(crate) fn source_path(&self, file: File) -> Option<&Utf8Path> {
        self.source_paths.get(&file).map(Utf8PathBuf::as_path)
    }

    pub(crate) fn parse_status(&self) -> ParseStatus {
        self.status
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PythonMutation {
    pub(super) root: String,
    pub(super) access: Vec<PythonMutationAccess>,
    pub(super) method: String,
}

impl PythonMutation {
    pub(crate) fn root(&self) -> &str {
        &self.root
    }

    pub(crate) fn access(&self) -> &[PythonMutationAccess] {
        &self.access
    }

    pub(crate) fn method(&self) -> &str {
        &self.method
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PythonMutationAccess {
    Index(usize),
    Key(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PythonBinding {
    pub(super) name: String,
    pub(super) values: Vec<PythonBoundValue>,
    pub(super) completeness: PythonCompleteness,
}

impl PythonBinding {
    pub(super) fn unknown(name: &str, origin: Origin) -> Self {
        Self {
            name: name.to_string(),
            values: vec![PythonBoundValue::unknown(origin)],
            completeness: PythonCompleteness::Partial,
        }
    }

    pub(super) fn full(name: &str, value: PythonValue, binding_origin: Origin) -> Self {
        let completeness = value.completeness();
        Self {
            name: name.to_string(),
            values: vec![PythonBoundValue {
                value,
                binding_origin,
            }],
            completeness,
        }
    }

    pub(crate) fn values(&self) -> &[PythonBoundValue] {
        &self.values
    }

    pub(crate) fn is_complete(&self) -> bool {
        self.completeness == PythonCompleteness::Full
            && self.values.iter().all(PythonBoundValue::is_complete)
    }

    pub(super) fn mark_partial(&mut self) {
        self.completeness = PythonCompleteness::Partial;
    }

    pub(super) fn joined(
        name: &str,
        bindings: impl IntoIterator<Item = Option<Self>>,
    ) -> Option<Self> {
        let mut values: Vec<PythonBoundValue> = Vec::new();
        let mut completeness = PythonCompleteness::Full;
        let mut saw_binding = false;

        for binding in bindings {
            let Some(binding) = binding else {
                completeness = PythonCompleteness::Partial;
                continue;
            };
            saw_binding = true;
            if !binding.is_complete() {
                completeness = PythonCompleteness::Partial;
            }
            for value in binding.values {
                if !values
                    .iter()
                    .any(|existing| existing.semantically_eq(&value))
                {
                    values.push(value);
                }
            }
        }

        if !saw_binding {
            return None;
        }

        if values.len() != 1 {
            completeness = PythonCompleteness::Partial;
        }

        Some(Self {
            name: name.to_string(),
            values,
            completeness,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PythonBoundValue {
    pub(super) value: PythonValue,
    pub(super) binding_origin: Origin,
}

impl PythonBoundValue {
    pub(super) fn unknown(origin: Origin) -> Self {
        Self {
            value: PythonValue::unknown(origin),
            binding_origin: origin,
        }
    }

    pub(crate) fn value(&self) -> &PythonValue {
        &self.value
    }

    pub(crate) fn value_origin(&self) -> Origin {
        self.value.origin()
    }

    pub(crate) fn is_complete(&self) -> bool {
        self.value.is_complete()
    }

    pub(super) fn semantically_eq(&self, other: &Self) -> bool {
        self.value.semantically_eq(&other.value)
    }
}

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

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(super) struct PythonBindings {
    pub(super) by_name: FxHashMap<String, PythonBinding>,
}

impl PythonBindings {
    pub(super) fn get(&self, name: &str) -> Option<&PythonBinding> {
        self.by_name.get(name)
    }

    pub(super) fn bind(&mut self, name: &str, binding: PythonBinding) {
        self.by_name.insert(name.to_string(), binding);
    }

    pub(super) fn remove(&mut self, name: &str) {
        self.by_name.remove(name);
    }

    pub(super) fn mark_all_partial(&mut self) {
        for binding in self.by_name.values_mut() {
            binding.mark_partial();
        }
    }

    pub(super) fn merge_star_import(&mut self, imported: &PythonSemanticModel) {
        self.by_name.extend(imported.bindings.by_name.clone());
    }

    pub(super) fn path_bindings(&self) -> PythonPathBindings {
        let mut paths = PythonPathBindings::default();
        for (name, binding) in &self.by_name {
            let [bound] = binding.values.as_slice() else {
                continue;
            };
            let PythonValueKind::Path(path) = bound.value.kind() else {
                continue;
            };
            if bound.is_complete() && binding.is_complete() {
                paths.set(name.clone(), path.clone());
            }
        }
        paths
    }

    pub(super) fn bool_value(&self, name: &str) -> Option<bool> {
        let binding = self.by_name.get(name)?;
        if !binding.is_complete() {
            return None;
        }
        let [bound] = binding.values.as_slice() else {
            return None;
        };
        match bound.value.kind() {
            PythonValueKind::Bool(value) => Some(*value),
            _ => None,
        }
    }
}
