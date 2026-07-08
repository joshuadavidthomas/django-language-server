use djls_source::Origin;
use rustc_hash::FxHashMap;

use super::model::PythonSemanticModel;
use super::values::PythonCompleteness;
use super::values::PythonValue;
use super::values::PythonValueKind;
use crate::python::PythonPathBindings;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PythonBinding {
    pub(super) name: String,
    pub(super) values: Vec<PythonBindingValue>,
    pub(super) completeness: PythonCompleteness,
}

impl PythonBinding {
    pub(super) fn unknown(name: &str, origin: Origin) -> Self {
        Self {
            name: name.to_string(),
            values: vec![PythonBindingValue::unknown(origin)],
            completeness: PythonCompleteness::Partial,
        }
    }

    pub(super) fn full(name: &str, value: PythonValue, binding_origin: Origin) -> Self {
        let completeness = value.completeness();
        Self {
            name: name.to_string(),
            values: vec![PythonBindingValue {
                value,
                binding_origin,
            }],
            completeness,
        }
    }

    pub(crate) fn values(&self) -> &[PythonBindingValue] {
        &self.values
    }

    pub(crate) fn is_complete(&self) -> bool {
        self.completeness == PythonCompleteness::Full
            && self.values.iter().all(PythonBindingValue::is_complete)
    }

    pub(super) fn mark_partial(&mut self) {
        self.completeness = PythonCompleteness::Partial;
    }

    pub(super) fn joined(
        name: &str,
        bindings: impl IntoIterator<Item = Option<Self>>,
    ) -> Option<Self> {
        let mut values: Vec<PythonBindingValue> = Vec::new();
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
pub(crate) struct PythonBindingValue {
    pub(super) value: PythonValue,
    pub(super) binding_origin: Origin,
}

impl PythonBindingValue {
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
