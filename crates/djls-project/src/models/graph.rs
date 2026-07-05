use std::borrow::Borrow;
use std::borrow::Cow;
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::btree_map::Entry;
use std::fmt;
use std::str::FromStr;
use std::sync::Arc;

use serde::Deserialize;
use serde::Serialize;
use thiserror::Error;

use crate::db::Db as ProjectDb;
use crate::project::Project;
use crate::python::ImportPathResolutionError;
use crate::python::ImportTable;
use crate::python::InvalidModuleName;
use crate::python::PythonModuleName;
use crate::python::resolve_prefix;

macro_rules! string_newtype {
    ($(#[doc = $doc:literal])* $vis:vis struct $Name:ident) => {
        $(#[doc = $doc])*
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        $vis struct $Name(Arc<str>);

        impl $Name {
            #[must_use]
            $vis fn new(value: impl Into<String>) -> Self {
                Self(Arc::from(value.into()))
            }

            #[must_use]
            $vis fn as_str(&self) -> &str {
                &self.0
            }

        }

        impl Borrow<str> for $Name {
            fn borrow(&self) -> &str {
                &self.0
            }
        }

        impl From<&str> for $Name {
            fn from(s: &str) -> Self {
                Self(Arc::from(s))
            }
        }

        impl From<String> for $Name {
            fn from(s: String) -> Self {
                Self(Arc::from(s))
            }
        }

        impl fmt::Display for $Name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.as_str().fmt(f)
            }
        }
    };
}

string_newtype! {
    /// A Django model class name (e.g., `"User"`, `"Article"`).
    pub(crate) struct ModelName
}

string_newtype! {
    /// A Python field/attribute name on a Django model (e.g., `"user"`,
    /// `"content_type"`).
    pub(crate) struct FieldName
}

/// A deterministic import identity for a Django model.
///
/// The module name is the importable Python module where the model class was
/// found, and the name is the class name within that module. Serde represents
/// the identity as a qualified import-style string such as
/// `"django.contrib.auth.models.User"`.
#[derive(Clone, Debug, Eq, PartialEq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ModelId {
    name: ModelName,
    module_name: PythonModuleName,
}

impl ModelId {
    #[must_use]
    fn new(module_name: PythonModuleName, name: ModelName) -> Self {
        Self { name, module_name }
    }

    #[must_use]
    pub fn module_name(&self) -> &PythonModuleName {
        &self.module_name
    }

    #[must_use]
    pub fn name(&self) -> &str {
        self.name.as_str()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ModelIdParseError {
    #[error("model id must include a module name and model name separated by '.'")]
    MissingModuleNameSeparator,
    #[error("model id must include a model name")]
    EmptyModelName,
    #[error("model id has invalid module name: {0}")]
    InvalidModuleName(#[from] InvalidModuleName),
}

impl FromStr for ModelId {
    type Err = ModelIdParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let (module_name, name) = value
            .rsplit_once('.')
            .ok_or(ModelIdParseError::MissingModuleNameSeparator)?;
        if name.is_empty() {
            return Err(ModelIdParseError::EmptyModelName);
        }

        let module_name = PythonModuleName::parse(module_name)?;
        Ok(Self::new(module_name, ModelName::new(name)))
    }
}

impl TryFrom<&str> for ModelId {
    type Error = ModelIdParseError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        value.parse()
    }
}

impl TryFrom<String> for ModelId {
    type Error = ModelIdParseError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        value.as_str().try_into()
    }
}

impl From<ModelId> for String {
    fn from(value: ModelId) -> Self {
        format!("{}.{}", value.module_name.as_str(), value.name.as_str())
    }
}

/// A Django model relation reference as it appears in source.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub(crate) enum RelationTarget {
    SelfRef,
    Qualified { app_label: String, name: ModelName },
    Bare { name: ModelName },
    Attribute { path: Vec<String> },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) enum RelationTargetResolution {
    Resolved(ModelId),
    Ambiguous {
        candidates: Vec<ModelId>,
        app_label: String,
        name: ModelName,
    },
    Partial {
        resolved_prefix: PythonModuleName,
        unresolved_tail: Vec<String>,
    },
    Unresolved {
        reason: RelationTargetUnresolvedReason,
    },
}

impl RelationTargetResolution {
    fn file_local_placeholder() -> Self {
        Self::Unresolved {
            reason: RelationTargetUnresolvedReason::FileLocal,
        }
    }

    fn is_file_local_placeholder(&self) -> bool {
        matches!(
            self,
            Self::Unresolved {
                reason: RelationTargetUnresolvedReason::FileLocal
            }
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) enum RelationTargetUnresolvedReason {
    /// per-file extraction cannot resolve targets; the project pass overwrites this placeholder
    FileLocal,
    /// relation has no statically-derivable target, e.g. `GenericForeignKey`
    NoStaticTarget,
    MissingImportBinding {
        binding: String,
    },
    InvalidImportedTarget {
        target: String,
    },
    ImportNotFound {
        requested: PythonModuleName,
    },
    ImportedTargetIsModule {
        module: PythonModuleName,
    },
    SameAppTargetNotFound {
        app_label: String,
        name: ModelName,
    },
    AppLabelTargetNotFound {
        app_label: String,
        name: ModelName,
    },
}

/// The kind of relation a Django model field represents.
///
/// Each variant carries its own data, so fields that don't apply to a given
/// relation kind (e.g., `target` on a `GenericForeignKey`) are simply
/// absent rather than wrapped in `Option`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "relation_type")]
pub(crate) enum RelationType {
    ForeignKey {
        target: RelationTarget,
        related_name: Option<String>,
    },
    OneToOne {
        target: RelationTarget,
        related_name: Option<String>,
    },
    ManyToMany {
        target: RelationTarget,
        related_name: Option<String>,
    },
    GenericForeignKey {
        ct_field: FieldName,
        fk_field: FieldName,
    },
}

impl RelationType {
    /// Construct a relation type from a Django field class name.
    ///
    /// Maps Python field class names to their corresponding variants:
    /// - `"ForeignKey"` → [`RelationType::ForeignKey`]
    /// - `"OneToOneField"` → [`RelationType::OneToOne`]
    /// - `"ManyToManyField"` → [`RelationType::ManyToMany`]
    ///
    /// Returns `None` for unrecognized names.
    #[must_use]
    pub(crate) fn from_field_class(
        name: &str,
        target: RelationTarget,
        related_name: Option<String>,
    ) -> Option<Self> {
        match name {
            "ForeignKey" => Some(Self::ForeignKey {
                target,
                related_name,
            }),
            "OneToOneField" => Some(Self::OneToOne {
                target,
                related_name,
            }),
            "ManyToManyField" => Some(Self::ManyToMany {
                target,
                related_name,
            }),
            _ => None,
        }
    }
}

/// What kind of Django model this is.
///
/// Currently tracks concrete vs. abstract models. An enum (rather than a
/// boolean) so that future model kinds (e.g., proxy) can be added with
/// exhaustive match enforcement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum ModelKind {
    Concrete,
    Abstract,
}

/// A relation field on a Django model.
///
/// The `relation_type` variant determines what data is available:
/// concrete relations (FK, O2O, M2M) carry a `target` and optional
/// `related_name`, while `GenericForeignKey` carries `ct_field`/`fk_field`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
// `relation_type` is serialized fact schema across corpus snapshots.
#[allow(clippy::struct_field_names)]
pub(crate) struct Relation {
    pub(crate) field_name: FieldName,
    #[serde(flatten)]
    pub(crate) relation_type: RelationType,
    #[serde(
        default = "RelationTargetResolution::file_local_placeholder",
        skip_serializing_if = "RelationTargetResolution::is_file_local_placeholder"
    )]
    resolution: RelationTargetResolution,
}

impl Relation {
    #[must_use]
    pub(crate) fn new(field_name: FieldName, relation_type: RelationType) -> Self {
        Self {
            field_name,
            relation_type,
            resolution: RelationTargetResolution::file_local_placeholder(),
        }
    }

    /// Get the target model if this is a concrete relation (FK, O2O, M2M).
    ///
    /// Returns `None` for `GenericForeignKey` since its target is determined
    /// at runtime via the content type.
    #[must_use]
    pub(crate) fn target_model(&self) -> Option<&RelationTarget> {
        match &self.relation_type {
            RelationType::ForeignKey { target, .. }
            | RelationType::OneToOne { target, .. }
            | RelationType::ManyToMany { target, .. } => Some(target),
            RelationType::GenericForeignKey { .. } => None,
        }
    }

    /// Resolve the effective `related_name` for reverse lookups.
    ///
    /// Returns `None` for `GenericForeignKey` (no static reverse name).
    ///
    /// For concrete relations: if an explicit `related_name` was provided,
    /// uses that (with `%(class)s` and `%(app_label)s` substitution applied).
    /// Otherwise synthesizes Django's default: `<model>_set` for FK/M2M, or
    /// `<model>` for `OneToOne`.
    ///
    /// `module_name` is the dotted Python module name (e.g., `"myapp.models"`);
    /// the app label is derived as the component before `models`.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn effective_related_name(
        &self,
        source_model: &str,
        module_name: &str,
    ) -> Option<String> {
        let lower = source_model.to_lowercase();
        match &self.relation_type {
            RelationType::ForeignKey { related_name, .. }
            | RelationType::ManyToMany { related_name, .. } => Some(match related_name {
                Some(name) => substitute_related_name(name, &lower, module_name),
                None => format!("{lower}_set"),
            }),
            RelationType::OneToOne { related_name, .. } => Some(match related_name {
                Some(name) => substitute_related_name(name, &lower, module_name),
                None => lower,
            }),
            RelationType::GenericForeignKey { .. } => None,
        }
    }

    fn effective_related_name_matches(
        &self,
        source_model: &str,
        module_name: &str,
        field_name: &str,
    ) -> bool {
        match &self.relation_type {
            RelationType::ForeignKey { related_name, .. }
            | RelationType::ManyToMany { related_name, .. } => match related_name {
                Some(name) => {
                    template_related_name_matches(name, source_model, module_name, field_name)
                }
                None => field_name
                    .strip_suffix("_set")
                    .is_some_and(|prefix| source_model.to_lowercase() == prefix),
            },
            RelationType::OneToOne { related_name, .. } => match related_name {
                Some(name) => {
                    template_related_name_matches(name, source_model, module_name, field_name)
                }
                None => source_model.to_lowercase() == field_name,
            },
            RelationType::GenericForeignKey { .. } => false,
        }
    }
}

fn template_related_name_matches(
    template: &str,
    source_model: &str,
    module_name: &str,
    field_name: &str,
) -> bool {
    if !template.contains("%(") {
        return template == field_name;
    }

    let lower = source_model.to_lowercase();
    substitute_related_name(template, &lower, module_name) == field_name
}

fn substitute_related_name(template: &str, lower_model: &str, module_name: &str) -> String {
    let substituted = template.replace("%(class)s", lower_model);
    if !substituted.contains("%(app_label)s") {
        return substituted;
    }

    let app_label = lower_app_label(app_label_from_module_name(module_name).unwrap_or_default());
    substituted.replace("%(app_label)s", &app_label)
}

fn lower_app_label(app_label: &str) -> Cow<'_, str> {
    if app_label.chars().any(char::is_uppercase) {
        Cow::Owned(app_label.to_lowercase())
    } else {
        Cow::Borrowed(app_label)
    }
}

/// Derive the app label from a dotted module name.
///
/// This is an approximation until real `AppConfig` support exists. It mirrors
/// the common convention: the component immediately before `models` in the
/// module name. Returns `None` when no valid app label can be determined
/// (e.g., bare `"models"` with no package prefix, or an empty name).
fn app_label_from_module_name(module_name: &str) -> Option<&str> {
    let mut first: Option<&str> = None;
    let mut previous: Option<&str> = None;

    for part in module_name.split('.') {
        if first.is_none() {
            first = Some(part);
        }
        if part == "models" {
            return match previous {
                Some(label) if label != "models" && !label.is_empty() => Some(label),
                _ => None,
            };
        }
        previous = Some(part);
    }

    match first {
        Some("models" | "") | None => None,
        Some(label) => Some(label),
    }
}

fn django_name_matches(candidate: &str, query: &str) -> bool {
    candidate == query
        || candidate.eq_ignore_ascii_case(query)
        || candidate.to_lowercase() == query.to_lowercase()
}

fn unresolved_import_path_reason(
    error: ImportPathResolutionError,
) -> RelationTargetUnresolvedReason {
    match error {
        ImportPathResolutionError::EmptyPath => {
            RelationTargetUnresolvedReason::InvalidImportedTarget {
                target: String::new(),
            }
        }
        ImportPathResolutionError::MissingBinding { binding } => {
            RelationTargetUnresolvedReason::MissingImportBinding { binding }
        }
        ImportPathResolutionError::InvalidTarget { target } => {
            RelationTargetUnresolvedReason::InvalidImportedTarget { target }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelDef {
    pub(crate) name: ModelName,
    pub(crate) module_name: PythonModuleName,
    pub(crate) line: usize,
    pub(crate) relations: Vec<Relation>,
    pub(crate) kind: ModelKind,
}

impl ModelDef {
    #[must_use]
    pub fn new(name: impl Into<String>, module_name: PythonModuleName, line: usize) -> Self {
        Self {
            name: ModelName::new(name),
            module_name,
            line,
            relations: Vec::new(),
            kind: ModelKind::Concrete,
        }
    }
}

/// Dependency graph of Django models and their relations.
///
/// Models are keyed by deterministic import identity (`ModelId`) so
/// identically-named models from different importable modules can coexist.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ModelGraph {
    models: BTreeMap<ModelId, ModelDef>,
    #[serde(skip)]
    model_ids_by_name: BTreeMap<ModelName, BTreeSet<ModelId>>,
    #[serde(skip)]
    overwritten_model_ids: Vec<ModelId>,
}

impl PartialEq for ModelGraph {
    fn eq(&self, other: &Self) -> bool {
        self.models == other.models
    }
}

impl Eq for ModelGraph {}

impl<'de> Deserialize<'de> for ModelGraph {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct SerializedModelGraph {
            models: BTreeMap<ModelId, ModelDef>,
        }

        let serialized = SerializedModelGraph::deserialize(deserializer)?;
        let mut graph = Self {
            models: serialized.models,
            model_ids_by_name: BTreeMap::new(),
            overwritten_model_ids: Vec::new(),
        };
        graph.rebuild_model_ids_by_name();
        Ok(graph)
    }
}

impl ModelGraph {
    #[must_use]
    pub fn empty_ref() -> &'static Self {
        static EMPTY: std::sync::LazyLock<ModelGraph> = std::sync::LazyLock::new(ModelGraph::new);
        &EMPTY
    }

    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[cfg(debug_assertions)]
    pub(super) fn debug_assert_no_file_local_placeholders(&self) {
        let placeholders: Vec<String> = self
            .models
            .iter()
            .flat_map(|(id, model)| {
                model
                    .relations
                    .iter()
                    .filter(|&relation| relation.resolution.is_file_local_placeholder())
                    .map(|relation| {
                        format!(
                            "{}.{}.{}",
                            id.module_name.as_str(),
                            id.name.as_str(),
                            relation.field_name.as_str()
                        )
                    })
            })
            .collect();
        debug_assert!(
            placeholders.is_empty(),
            "model graph still has file-local relation target placeholders: {}",
            placeholders.join(", ")
        );
    }

    pub(crate) fn add_model(&mut self, model: ModelDef) {
        let id = ModelId::new(model.module_name.clone(), model.name.clone());
        if self.models.insert(id.clone(), model).is_some() {
            self.overwritten_model_ids.push(id.clone());
        }
        self.model_ids_by_name
            .entry(id.name.clone())
            .or_default()
            .insert(id);
    }

    #[cfg(test)]
    #[must_use]
    fn overwritten_model_ids(&self) -> &[ModelId] {
        &self.overwritten_model_ids
    }

    #[must_use]
    pub fn get_by_id(&self, id: &ModelId) -> Option<&ModelDef> {
        self.models.get(id)
    }

    #[must_use = "iterators are lazy and do nothing unless consumed"]
    pub fn models_named<'a>(
        &'a self,
        name: &'a str,
    ) -> impl Iterator<Item = (&'a ModelId, &'a ModelDef)> {
        self.model_ids_by_name
            .get(name)
            .into_iter()
            .flat_map(move |ids| ids.iter().filter_map(|id| self.models.get_key_value(id)))
    }

    pub(crate) fn models(&self) -> impl Iterator<Item = &ModelDef> {
        self.models.values()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.models.is_empty()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.models.len()
    }

    /// Look up a model by approximate Django app label and class name.
    ///
    /// Comparisons use Django-style lowercase normalization for both the app
    /// label and model name. The app label is derived from the model module
    /// path; it is not stored as model identity.
    #[must_use]
    pub fn lookup(&self, app_label: &str, name: &str) -> Option<&ModelDef> {
        self.lookup_entry(app_label, name).map(|(_id, model)| model)
    }

    fn lookup_entry(&self, app_label: &str, name: &str) -> Option<(&ModelId, &ModelDef)> {
        if let Some(entry) = self.lookup_entry_exact(app_label, name) {
            return Some(entry);
        }

        self.models.iter().find(|(id, _model)| {
            django_name_matches(id.name.as_str(), name)
                && app_label_from_module_name(id.module_name.as_str())
                    .is_some_and(|candidate| django_name_matches(candidate, app_label))
        })
    }

    fn lookup_entry_exact(&self, app_label: &str, name: &str) -> Option<(&ModelId, &ModelDef)> {
        self.model_ids_by_name.get(name)?.iter().find_map(|id| {
            let (id, model) = self.models.get_key_value(id)?;
            if app_label_from_module_name(id.module_name.as_str()) == Some(app_label) {
                Some((id, model))
            } else {
                None
            }
        })
    }

    /// Look up the target model for a forward relation field on `scope`.
    ///
    /// Skips `GenericForeignKey` relations (no static target).
    #[must_use]
    pub fn resolve_forward(&self, scope: &ModelId, field_name: &str) -> Option<&ModelDef> {
        let relation = self.forward_relation(scope, field_name)?;
        self.resolve_relation_target_entry(scope, relation)
            .map(|(_id, model)| model)
    }

    fn forward_relation(&self, scope: &ModelId, field_name: &str) -> Option<&Relation> {
        let model = self.get_by_id(scope)?;
        model
            .relations
            .iter()
            .find(|relation| relation.field_name.as_str() == field_name)
    }

    fn resolve_relation_target_entry(
        &self,
        scope: &ModelId,
        relation: &Relation,
    ) -> Option<(&ModelId, &ModelDef)> {
        match &relation.resolution {
            RelationTargetResolution::Resolved(id) => self.models.get_key_value(id),
            RelationTargetResolution::Unresolved {
                reason: RelationTargetUnresolvedReason::FileLocal,
            } => relation
                .target_model()
                .and_then(|target| self.resolve_target_entry(scope, target)),
            RelationTargetResolution::Ambiguous { .. }
            | RelationTargetResolution::Partial { .. }
            | RelationTargetResolution::Unresolved { .. } => None,
        }
    }

    fn resolve_target_entry(
        &self,
        scope: &ModelId,
        target: &RelationTarget,
    ) -> Option<(&ModelId, &ModelDef)> {
        match target {
            RelationTarget::SelfRef => self.models.get_key_value(scope),
            RelationTarget::Bare { name } => {
                let app_label = app_label_from_module_name(scope.module_name.as_str())?;
                self.lookup_entry(app_label, name.as_str())
            }
            RelationTarget::Qualified { app_label, name } => {
                self.lookup_entry(app_label, name.as_str())
            }
            RelationTarget::Attribute { .. } => None,
        }
    }

    /// Look up models that point at `scope` via a reverse relation.
    ///
    /// Returns `(source_model_id, effective_related_name)` pairs. Skips
    /// `GenericForeignKey` relations.
    #[cfg(test)]
    fn resolve_reverse<'a>(
        &'a self,
        scope: &'a ModelId,
    ) -> impl Iterator<Item = (&'a ModelId, String)> + 'a {
        self.models.iter().flat_map(move |(source_id, model)| {
            model.relations.iter().filter_map(move |relation| {
                let (target_id, _target_model) =
                    self.resolve_relation_target_entry(source_id, relation)?;
                if target_id != scope {
                    return None;
                }

                relation
                    .effective_related_name(model.name.as_str(), model.module_name.as_str())
                    .map(|name| (source_id, name))
            })
        })
    }

    /// Resolve a field access on a model — checks forward relations first,
    /// then reverse relations.
    #[must_use]
    pub fn resolve_relation<'a>(
        &'a self,
        scope: &'a ModelId,
        field_name: &str,
    ) -> Option<&'a ModelDef> {
        if let Some(relation) = self.forward_relation(scope, field_name) {
            return self
                .resolve_relation_target_entry(scope, relation)
                .map(|(_id, model)| model);
        }

        self.resolve_reverse_relation(scope, field_name)
            .and_then(|source_id| self.get_by_id(source_id))
    }

    fn resolve_reverse_relation(&self, scope: &ModelId, field_name: &str) -> Option<&ModelId> {
        for (source_id, model) in &self.models {
            for relation in &model.relations {
                let Some((target_id, _target_model)) =
                    self.resolve_relation_target_entry(source_id, relation)
                else {
                    continue;
                };
                if target_id == scope
                    && relation.effective_related_name_matches(
                        model.name.as_str(),
                        model.module_name.as_str(),
                        field_name,
                    )
                {
                    return Some(source_id);
                }
            }
        }

        None
    }

    pub(crate) fn resolve_relation_targets(
        &mut self,
        db: &dyn ProjectDb,
        project: Project,
        import_tables: &BTreeMap<PythonModuleName, &ImportTable>,
    ) {
        let mut updates = Vec::new();
        for (scope, model) in &self.models {
            for (index, relation) in model.relations.iter().enumerate() {
                updates.push((
                    scope.clone(),
                    index,
                    self.resolve_relation_target(db, project, scope, relation, import_tables),
                ));
            }
        }

        for (scope, index, resolution) in updates {
            if let Some(model) = self.models.get_mut(&scope)
                && let Some(relation) = model.relations.get_mut(index)
            {
                relation.resolution = resolution;
            }
        }
    }

    fn resolve_relation_target(
        &self,
        db: &dyn ProjectDb,
        project: Project,
        scope: &ModelId,
        relation: &Relation,
        import_tables: &BTreeMap<PythonModuleName, &ImportTable>,
    ) -> RelationTargetResolution {
        let Some(target) = relation.target_model() else {
            return RelationTargetResolution::Unresolved {
                reason: RelationTargetUnresolvedReason::NoStaticTarget,
            };
        };

        match target {
            RelationTarget::SelfRef => RelationTargetResolution::Resolved(scope.clone()),
            RelationTarget::Qualified { app_label, name } => self.resolve_app_label_target(
                app_label,
                name,
                RelationTargetUnresolvedReason::AppLabelTargetNotFound {
                    app_label: app_label.clone(),
                    name: name.clone(),
                },
            ),
            RelationTarget::Bare { name } => {
                let Some(import_table) = import_tables.get(&scope.module_name) else {
                    return self.resolve_same_app_target(scope, name);
                };
                match import_table.resolve_qualified_path(std::iter::once(name.as_str())) {
                    Ok(target) => self.resolve_imported_relation_target(db, project, &target),
                    Err(ImportPathResolutionError::MissingBinding { .. }) => {
                        self.resolve_same_app_target(scope, name)
                    }
                    Err(error) => RelationTargetResolution::Unresolved {
                        reason: unresolved_import_path_reason(error),
                    },
                }
            }
            RelationTarget::Attribute { path } => {
                let Some(root) = path.first() else {
                    return RelationTargetResolution::Unresolved {
                        reason: RelationTargetUnresolvedReason::InvalidImportedTarget {
                            target: String::new(),
                        },
                    };
                };
                let Some(import_table) = import_tables.get(&scope.module_name) else {
                    return RelationTargetResolution::Unresolved {
                        reason: RelationTargetUnresolvedReason::MissingImportBinding {
                            binding: root.clone(),
                        },
                    };
                };
                match import_table.resolve_qualified_path(path.iter().map(String::as_str)) {
                    Ok(target) => self.resolve_imported_relation_target(db, project, &target),
                    Err(error) => RelationTargetResolution::Unresolved {
                        reason: unresolved_import_path_reason(error),
                    },
                }
            }
        }
    }

    fn resolve_same_app_target(
        &self,
        scope: &ModelId,
        name: &ModelName,
    ) -> RelationTargetResolution {
        let Some(app_label) = app_label_from_module_name(scope.module_name.as_str()) else {
            return RelationTargetResolution::Unresolved {
                reason: RelationTargetUnresolvedReason::SameAppTargetNotFound {
                    app_label: String::new(),
                    name: name.clone(),
                },
            };
        };

        self.resolve_app_label_target(
            app_label,
            name,
            RelationTargetUnresolvedReason::SameAppTargetNotFound {
                app_label: app_label.to_string(),
                name: name.clone(),
            },
        )
    }

    fn resolve_app_label_target(
        &self,
        app_label: &str,
        name: &ModelName,
        not_found: RelationTargetUnresolvedReason,
    ) -> RelationTargetResolution {
        if let Some((id, _model)) = self.lookup_entry_exact(app_label, name.as_str()) {
            return RelationTargetResolution::Resolved(id.clone());
        }

        let candidates: Vec<ModelId> = self
            .models
            .iter()
            .filter(|(id, _model)| {
                django_name_matches(id.name.as_str(), name.as_str())
                    && app_label_from_module_name(id.module_name.as_str())
                        .is_some_and(|candidate| django_name_matches(candidate, app_label))
            })
            .map(|(id, _model)| id.clone())
            .collect();

        match candidates.as_slice() {
            [candidate] => RelationTargetResolution::Resolved(candidate.clone()),
            [] => RelationTargetResolution::Unresolved { reason: not_found },
            _ => RelationTargetResolution::Ambiguous {
                candidates,
                app_label: app_label.to_string(),
                name: name.clone(),
            },
        }
    }

    fn resolve_imported_relation_target(
        &self,
        db: &dyn ProjectDb,
        project: Project,
        target: &PythonModuleName,
    ) -> RelationTargetResolution {
        let resolved = resolve_prefix(db, project, target.as_str());
        let Some(module) = resolved.module else {
            return RelationTargetResolution::Unresolved {
                reason: RelationTargetUnresolvedReason::ImportNotFound {
                    requested: target.clone(),
                },
            };
        };

        if resolved.unresolved_tail.is_empty() {
            return RelationTargetResolution::Unresolved {
                reason: RelationTargetUnresolvedReason::ImportedTargetIsModule {
                    module: module.name().clone(),
                },
            };
        }

        let unresolved_tail = resolved.unresolved_tail;
        if unresolved_tail.len() == 1 {
            let candidate = ModelId::new(
                module.name().clone(),
                ModelName::new(unresolved_tail[0].clone()),
            );
            if self.models.contains_key(&candidate) {
                return RelationTargetResolution::Resolved(candidate);
            }
        }

        RelationTargetResolution::Partial {
            resolved_prefix: module.name().clone(),
            unresolved_tail,
        }
    }

    fn rebuild_model_ids_by_name(&mut self) {
        self.model_ids_by_name.clear();
        for id in self.models.keys() {
            self.model_ids_by_name
                .entry(id.name.clone())
                .or_default()
                .insert(id.clone());
        }
    }

    /// Merge another graph into this one.
    ///
    /// Models from `other` overwrite models with the same import identity in `self`.
    pub fn merge(&mut self, other: ModelGraph) {
        let ModelGraph {
            models,
            model_ids_by_name,
            overwritten_model_ids,
        } = other;
        self.overwritten_model_ids.extend(overwritten_model_ids);

        for (name, ids) in model_ids_by_name {
            self.model_ids_by_name.entry(name).or_default().extend(ids);
        }

        for (id, model) in models {
            match self.models.entry(id) {
                Entry::Occupied(mut entry) => {
                    self.overwritten_model_ids.push(entry.key().clone());
                    entry.insert(model);
                }
                Entry::Vacant(entry) => {
                    entry.insert(model);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn module_name(name: &str) -> PythonModuleName {
        PythonModuleName::parse(name).unwrap()
    }

    fn model_id<'a>(graph: &'a ModelGraph, name: &'a str) -> &'a ModelId {
        graph
            .models_named(name)
            .next()
            .map(|(id, _model)| id)
            .expect("model should exist")
    }

    fn user_order_graph() -> ModelGraph {
        let mut graph = ModelGraph::new();

        let user = ModelDef::new("User", module_name("auth.models"), 1);

        let mut order = ModelDef::new("Order", module_name("shop.models"), 1);
        order.relations.push(Relation::new(
            "user".into(),
            RelationType::ForeignKey {
                target: RelationTarget::Qualified {
                    app_label: "auth".into(),
                    name: ModelName::new("User"),
                },
                related_name: Some("orders".into()),
            },
        ));

        let mut profile = ModelDef::new("Profile", module_name("accounts.models"), 1);
        profile.relations.push(Relation::new(
            "user".into(),
            RelationType::OneToOne {
                target: RelationTarget::Qualified {
                    app_label: "auth".into(),
                    name: ModelName::new("User"),
                },
                related_name: None,
            },
        ));

        graph.add_model(user);
        graph.add_model(order);
        graph.add_model(profile);
        graph
    }

    #[test]
    fn forward_lookup() {
        let graph = user_order_graph();
        assert_eq!(
            graph
                .resolve_forward(model_id(&graph, "Order"), "user")
                .map(|model| model.name.as_str()),
            Some("User")
        );
        assert_eq!(
            graph
                .resolve_forward(model_id(&graph, "Profile"), "user")
                .map(|model| model.name.as_str()),
            Some("User")
        );
        assert_eq!(
            graph
                .resolve_forward(model_id(&graph, "User"), "user")
                .map(|model| model.name.as_str()),
            None
        );
    }

    #[test]
    fn reverse_lookup() {
        let graph = user_order_graph();
        let reverses: Vec<_> = graph
            .resolve_reverse(model_id(&graph, "User"))
            .map(|(src, name)| (src.name().to_string(), name))
            .collect();
        assert!(reverses.contains(&("Order".into(), "orders".into())));
        assert!(reverses.contains(&("Profile".into(), "profile".into())));
    }

    #[test]
    fn resolve_relation_forward_and_reverse() {
        let graph = user_order_graph();
        assert_eq!(
            graph
                .resolve_relation(model_id(&graph, "Order"), "user")
                .map(|model| model.name.as_str()),
            Some("User")
        );
        assert_eq!(
            graph
                .resolve_relation(model_id(&graph, "User"), "orders")
                .map(|model| model.name.as_str()),
            Some("Order")
        );
        assert_eq!(
            graph
                .resolve_relation(model_id(&graph, "User"), "profile")
                .map(|model| model.name.as_str()),
            Some("Profile")
        );
    }

    #[test]
    fn unresolved_forward_relation_does_not_fall_through_to_reverse_lookup() {
        let mut graph = ModelGraph::new();

        let mut user = ModelDef::new("User", module_name("auth.models"), 1);
        user.relations.push(Relation::new(
            "orders".into(),
            RelationType::ForeignKey {
                target: RelationTarget::Qualified {
                    app_label: "missing".into(),
                    name: ModelName::new("Order"),
                },
                related_name: None,
            },
        ));

        let mut order = ModelDef::new("Order", module_name("shop.models"), 1);
        order.relations.push(Relation::new(
            "user".into(),
            RelationType::ForeignKey {
                target: RelationTarget::Qualified {
                    app_label: "auth".into(),
                    name: ModelName::new("User"),
                },
                related_name: Some("orders".into()),
            },
        ));

        graph.add_model(user);
        graph.add_model(order);

        assert_eq!(
            graph
                .resolve_relation(model_id(&graph, "User"), "orders")
                .map(|model| model.name.as_str()),
            None
        );
    }

    #[test]
    fn default_related_name_fk() {
        let rel = Relation::new(
            "user".into(),
            RelationType::ForeignKey {
                target: RelationTarget::Bare {
                    name: ModelName::new("User"),
                },
                related_name: None,
            },
        );
        assert_eq!(
            rel.effective_related_name("Order", "shop.models"),
            Some("order_set".into())
        );
    }

    #[test]
    fn default_related_name_o2o() {
        let rel = Relation::new(
            "user".into(),
            RelationType::OneToOne {
                target: RelationTarget::Bare {
                    name: ModelName::new("User"),
                },
                related_name: None,
            },
        );
        assert_eq!(
            rel.effective_related_name("Profile", "accounts.models"),
            Some("profile".into())
        );
    }

    #[test]
    fn class_substitution_in_related_name() {
        let rel = Relation::new(
            "user".into(),
            RelationType::ForeignKey {
                target: RelationTarget::Bare {
                    name: ModelName::new("User"),
                },
                related_name: Some("%(class)s_orders".into()),
            },
        );
        assert_eq!(
            rel.effective_related_name("SpecialOrder", "shop.models"),
            Some("specialorder_orders".into())
        );
    }

    #[test]
    fn app_label_substitution_in_related_name() {
        let rel = Relation::new(
            "title".into(),
            RelationType::ForeignKey {
                target: RelationTarget::Bare {
                    name: ModelName::new("Title"),
                },
                related_name: Some("attached_%(app_label)s_%(class)s_set".into()),
            },
        );
        assert_eq!(
            rel.effective_related_name("Article", "blog.models"),
            Some("attached_blog_article_set".into())
        );
    }

    #[test]
    fn app_label_from_nested_module_name() {
        let rel = Relation::new(
            "user".into(),
            RelationType::ForeignKey {
                target: RelationTarget::Bare {
                    name: ModelName::new("User"),
                },
                related_name: Some("%(app_label)s_%(class)s_set".into()),
            },
        );
        assert_eq!(
            rel.effective_related_name("Permission", "django.contrib.auth.models"),
            Some("auth_permission_set".into())
        );
    }

    #[test]
    fn app_label_bare_models_path() {
        // A bare "models" module name has no valid app label — %(app_label)s
        // should substitute as empty rather than producing "models".
        let rel = Relation::new(
            "user".into(),
            RelationType::ForeignKey {
                target: RelationTarget::Bare {
                    name: ModelName::new("User"),
                },
                related_name: Some("%(app_label)s_%(class)s_set".into()),
            },
        );
        assert_eq!(
            rel.effective_related_name("Order", "models"),
            Some("_order_set".into())
        );
    }

    #[test]
    fn app_label_no_models_component() {
        // When "models" doesn't appear, falls back to first component.
        let rel = Relation::new(
            "user".into(),
            RelationType::ForeignKey {
                target: RelationTarget::Bare {
                    name: ModelName::new("User"),
                },
                related_name: Some("%(app_label)s_set".into()),
            },
        );
        assert_eq!(
            rel.effective_related_name("Order", "myapp"),
            Some("myapp_set".into())
        );
    }

    #[test]
    fn models_named_returns_same_name_models_in_module_order() {
        let mut graph = ModelGraph::new();
        graph.add_model(ModelDef::new("User", module_name("zeta.models"), 1));
        graph.add_model(ModelDef::new("Group", module_name("auth.models"), 2));
        graph.add_model(ModelDef::new("User", module_name("alpha.models"), 3));

        let users: Vec<_> = graph
            .models_named("User")
            .map(|(id, model)| (id.module_name().as_str(), model.name.as_str(), model.line))
            .collect();

        assert_eq!(
            users,
            vec![("alpha.models", "User", 3), ("zeta.models", "User", 1)]
        );
        assert!(graph.models_named("Missing").next().is_none());
    }

    #[test]
    fn lookup_entry_exact_requires_exact_app_label_and_model_name() {
        let mut graph = ModelGraph::new();
        graph.add_model(ModelDef::new("User", module_name("auth.models"), 1));

        let exact = graph
            .lookup_entry_exact("auth", "User")
            .expect("exact lookup should find an exact app label and model name");
        assert_eq!(exact.1.name.as_str(), "User");

        assert!(graph.lookup_entry_exact("auth", "user").is_none());
        assert!(graph.lookup_entry_exact("AUTH", "User").is_none());
        assert_eq!(
            graph.lookup("auth", "user").map(|model| model.line),
            Some(1)
        );
    }

    #[test]
    fn lookup_falls_back_to_django_name_matching() {
        let mut graph = ModelGraph::new();
        graph.add_model(ModelDef::new("User", module_name("auth.models"), 1));
        graph.add_model(ModelDef::new("Éclair", module_name("Café.models"), 2));

        let exact = graph
            .lookup("auth", "User")
            .expect("exact lookup should still work");
        assert_eq!(exact.module_name.as_str(), "auth.models");

        let ascii_case_insensitive = graph
            .lookup("AUTH", "user")
            .expect("lookup should fall back to ASCII-case-insensitive matching");
        assert_eq!(ascii_case_insensitive.module_name.as_str(), "auth.models");

        let lowercase_equal = graph
            .lookup("café", "éclair")
            .expect("lookup should fall back to lowercase-equal matching");
        assert_eq!(lowercase_equal.module_name.as_str(), "Café.models");
    }

    #[test]
    fn lookup_normalizes_app_label_and_model_name() {
        let mut graph = ModelGraph::new();
        graph.add_model(ModelDef::new("User", module_name("accounts.models"), 1));

        let model = graph
            .lookup("ACCOUNTS", "user")
            .expect("lookup should normalize app label and model name");
        assert_eq!(model.name.as_str(), "User");
        assert_eq!(model.module_name.as_str(), "accounts.models");
    }

    #[test]
    fn relation_target_policy_resolves_self_bare_and_qualified() {
        let mut graph = ModelGraph::new();
        graph.add_model(ModelDef::new("User", module_name("accounts.models"), 1));
        graph.add_model(ModelDef::new("User", module_name("blog.models"), 1));

        let mut post = ModelDef::new("Post", module_name("blog.models"), 1);
        post.relations.push(Relation::new(
            "author".into(),
            RelationType::ForeignKey {
                target: RelationTarget::Bare {
                    name: ModelName::new("User"),
                },
                related_name: None,
            },
        ));
        post.relations.push(Relation::new(
            "account_author".into(),
            RelationType::ForeignKey {
                target: RelationTarget::Qualified {
                    app_label: "accounts".into(),
                    name: ModelName::new("User"),
                },
                related_name: None,
            },
        ));
        post.relations.push(Relation::new(
            "parent".into(),
            RelationType::ForeignKey {
                target: RelationTarget::SelfRef,
                related_name: None,
            },
        ));
        graph.add_model(post);

        let post_id = model_id(&graph, "Post");
        assert_eq!(
            graph
                .resolve_forward(post_id, "author")
                .map(|model| model.module_name.as_str()),
            Some("blog.models")
        );
        assert_eq!(
            graph
                .resolve_forward(post_id, "account_author")
                .map(|model| model.module_name.as_str()),
            Some("accounts.models")
        );
        assert_eq!(
            graph
                .resolve_forward(post_id, "parent")
                .map(|model| model.module_name.as_str()),
            Some("blog.models")
        );
    }

    #[test]
    fn generic_foreign_key_has_no_related_name() {
        let rel = Relation::new(
            "content_object".into(),
            RelationType::GenericForeignKey {
                ct_field: "content_type".into(),
                fk_field: "object_id".into(),
            },
        );
        assert_eq!(
            rel.effective_related_name("TaggedItem", "tagging.models"),
            None
        );
        assert_eq!(rel.target_model(), None);
    }

    #[test]
    fn generic_foreign_key_skipped_in_forward_lookup() {
        let mut graph = ModelGraph::new();
        let mut model = ModelDef::new("TaggedItem", module_name("tagging.models"), 1);
        model.relations.push(Relation::new(
            "content_object".into(),
            RelationType::GenericForeignKey {
                ct_field: "content_type".into(),
                fk_field: "object_id".into(),
            },
        ));
        graph.add_model(model);

        assert_eq!(
            graph.resolve_forward(model_id(&graph, "TaggedItem"), "content_object"),
            None
        );
    }

    #[test]
    fn add_model_overwrites_same_identity() {
        let mut graph = ModelGraph::new();
        graph.add_model(ModelDef::new("User", module_name("auth.models"), 1));
        graph.add_model(ModelDef::new("User", module_name("auth.models"), 2));

        let expected_id = "auth.models.User".parse::<ModelId>().unwrap();
        let model = graph
            .lookup("auth", "User")
            .expect("overwritten model should exist");
        assert_eq!(graph.len(), 1);
        assert_eq!(graph.overwritten_model_ids(), &[expected_id]);
        assert_eq!(model.line, 2);
    }

    #[test]
    fn merge_overwrites_same_identity() {
        let mut g1 = ModelGraph::new();
        g1.add_model(ModelDef::new("User", module_name("auth.models"), 1));

        let mut g2 = ModelGraph::new();
        g2.add_model(ModelDef::new("User", module_name("auth.models"), 2));

        g1.merge(g2);
        let expected_id = "auth.models.User".parse::<ModelId>().unwrap();
        let model = g1
            .lookup("auth", "User")
            .expect("merged model should exist");
        assert_eq!(g1.len(), 1);
        assert_eq!(g1.overwritten_model_ids(), &[expected_id]);
        assert_eq!(model.line, 2);
    }

    #[test]
    fn merge_graphs() {
        let mut g1 = ModelGraph::new();
        g1.add_model(ModelDef::new("User", module_name("auth.models"), 1));

        let mut g2 = ModelGraph::new();
        g2.add_model(ModelDef::new("Order", module_name("shop.models"), 1));

        g1.merge(g2);
        assert_eq!(g1.len(), 2);
        assert!(g1.overwritten_model_ids().is_empty());
        assert!(g1.models_named("User").next().is_some());
        assert!(g1.models_named("Order").next().is_some());
    }

    #[test]
    fn same_named_models_in_different_modules_coexist() {
        let mut graph = ModelGraph::new();
        graph.add_model(ModelDef::new("Comment", module_name("blog.models"), 1));
        graph.add_model(ModelDef::new("Comment", module_name("news.models"), 1));

        let comments: Vec<_> = graph
            .models_named("Comment")
            .map(|(id, model)| (id.module_name().as_str(), model.module_name.as_str()))
            .collect();

        assert_eq!(graph.len(), 2);
        assert_eq!(
            comments,
            vec![
                ("blog.models", "blog.models"),
                ("news.models", "news.models")
            ]
        );
    }
}
