use std::borrow::Borrow;
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::fmt;
use std::str::FromStr;
use std::sync::Arc;

use djls_source::File;
use djls_source::Span;
use djls_source::Spanned;
use rustc_hash::FxHashMap;
use serde::Serialize;
use thiserror::Error;

use crate::db::Db as ProjectDb;
use crate::models::imports::ModelImportPathUnresolvedReason;
use crate::models::imports::ModelImportReference;
use crate::project::Project;
use crate::python::InvalidModuleName;
use crate::python::PythonModuleName;
use crate::python::resolve_prefix;

macro_rules! string_newtype {
    ($(#[doc = $doc:literal])* $vis:vis struct $Name:ident) => {
        $(#[doc = $doc])*
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
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
    /// A Python class name, whether or not the class is a Django model.
    pub(crate) struct ClassName
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
#[derive(Clone, Debug, Eq, PartialEq, PartialOrd, Ord, Hash, Serialize)]
#[serde(into = "String")]
pub struct ModelId {
    name: ModelName,
    module_name: PythonModuleName,
}

impl ModelId {
    #[must_use]
    pub(super) fn new(module_name: PythonModuleName, name: ModelName) -> Self {
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

/// The import identity of any extracted Python class.
#[derive(Clone, Debug, Eq, PartialEq, PartialOrd, Ord, Hash, Serialize)]
#[serde(into = "String")]
pub(crate) struct ClassId {
    name: ClassName,
    module_name: PythonModuleName,
}

impl ClassId {
    #[must_use]
    pub(super) fn new(module_name: PythonModuleName, name: impl Into<String>) -> Self {
        Self {
            name: ClassName::new(name),
            module_name,
        }
    }

    #[must_use]
    pub(crate) fn name(&self) -> &str {
        self.name.as_str()
    }

    #[must_use]
    pub(crate) fn module_name(&self) -> &PythonModuleName {
        &self.module_name
    }

    /// Promote this class identity after the resolver has admitted it as a
    /// Django model.
    #[must_use]
    pub(super) fn into_admitted_model_id(self) -> ModelId {
        ModelId::new(self.module_name, ModelName::new(self.name.as_str()))
    }

    /// Build the class identity used to look up a model in a mixed-class MRO.
    #[must_use]
    pub(crate) fn from_model_id(model: &ModelId) -> Self {
        Self::new(model.module_name.clone(), model.name.as_str())
    }
}

impl From<ClassId> for String {
    fn from(value: ClassId) -> Self {
        format!("{}.{}", value.module_name.as_str(), value.name.as_str())
    }
}

/// A Django model relation reference as it appears in source.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(tag = "kind")]
pub(crate) enum RelationTarget {
    SelfRef,
    Qualified {
        app_label: String,
        name: ModelName,
    },
    Bare {
        name: ModelName,
        // String targets have no import semantics; expression targets capture
        // the alias state at this source occurrence.
        #[serde(skip)]
        import_reference: Option<ModelImportReference>,
    },
    Attribute {
        path: Vec<String>,
        // Attribute targets always come from expressions, so their
        // occurrence-local import evidence is required.
        #[serde(skip)]
        import_reference: ModelImportReference,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
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
    fn file_local() -> Self {
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

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub(crate) enum RelationTargetUnresolvedReason {
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

/// The declared `related_name` for a relation field.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(untagged)]
pub(crate) enum RelatedName {
    /// An explicit accessor-name template, before placeholder substitution.
    Named(String),
    /// Django's trailing-`+` convention: no reverse accessor exists.
    Suppressed,
}

/// The kind of relation a Django model field represents.
///
/// Each variant carries its own data, so fields that don't apply to a given
/// relation kind (e.g., `target` on a `GenericForeignKey`) are simply
/// absent rather than wrapped in `Option`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(tag = "relation_type")]
pub(crate) enum RelationType {
    ForeignKey {
        target: Spanned<RelationTarget>,
        related_name: Option<RelatedName>,
    },
    OneToOne {
        target: Spanned<RelationTarget>,
        related_name: Option<RelatedName>,
    },
    ManyToMany {
        target: Spanned<RelationTarget>,
        related_name: Option<RelatedName>,
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
        target: Spanned<RelationTarget>,
        related_name: Option<String>,
    ) -> Option<Self> {
        let related_name = related_name.map(|name| {
            if name.ends_with('+') {
                RelatedName::Suppressed
            } else {
                RelatedName::Named(name)
            }
        });
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
// `relation_type` is serialized fact schema across corpus snapshots.
#[allow(clippy::struct_field_names)]
pub(crate) struct Relation {
    #[serde(skip)]
    pub(crate) file: File,
    pub(crate) field_name: Spanned<FieldName>,
    #[serde(flatten)]
    pub(crate) relation_type: RelationType,
}

impl Relation {
    #[must_use]
    pub(crate) fn new(
        file: File,
        field_name: Spanned<FieldName>,
        relation_type: RelationType,
    ) -> Self {
        Self {
            file,
            field_name,
            relation_type,
        }
    }

    #[must_use]
    pub(crate) fn target_span(&self) -> Option<Span> {
        match &self.relation_type {
            RelationType::ForeignKey { target, .. }
            | RelationType::OneToOne { target, .. }
            | RelationType::ManyToMany { target, .. } => Some(target.span()),
            RelationType::GenericForeignKey { .. } => None,
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
            | RelationType::ManyToMany { target, .. } => Some(target.value()),
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
    #[must_use]
    pub(crate) fn effective_related_name(
        &self,
        source_model: &str,
        module_name: &str,
    ) -> Option<String> {
        let lower = source_model.to_lowercase();
        match &self.relation_type {
            RelationType::ForeignKey { related_name, .. }
            | RelationType::ManyToMany { related_name, .. } => match related_name {
                Some(RelatedName::Named(name)) => {
                    Some(substitute_related_name(name, &lower, module_name))
                }
                Some(RelatedName::Suppressed) => None,
                None => Some(format!("{lower}_set")),
            },
            RelationType::OneToOne { related_name, .. } => match related_name {
                Some(RelatedName::Named(name)) => {
                    Some(substitute_related_name(name, &lower, module_name))
                }
                Some(RelatedName::Suppressed) => None,
                None => Some(lower),
            },
            RelationType::GenericForeignKey { .. } => None,
        }
    }

    #[cfg(test)]
    fn effective_related_name_matches(
        &self,
        source_model: &str,
        module_name: &str,
        field_name: &str,
    ) -> bool {
        match &self.relation_type {
            RelationType::ForeignKey { related_name, .. }
            | RelationType::ManyToMany { related_name, .. } => match related_name {
                Some(RelatedName::Named(name)) => {
                    template_related_name_matches(name, source_model, module_name, field_name)
                }
                Some(RelatedName::Suppressed) => false,
                None => field_name
                    .strip_suffix("_set")
                    .is_some_and(|prefix| source_model.to_lowercase() == prefix),
            },
            RelationType::OneToOne { related_name, .. } => match related_name {
                Some(RelatedName::Named(name)) => {
                    template_related_name_matches(name, source_model, module_name, field_name)
                }
                Some(RelatedName::Suppressed) => false,
                None => source_model.to_lowercase() == field_name,
            },
            RelationType::GenericForeignKey { .. } => false,
        }
    }
}

#[cfg(test)]
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

    let app_label = app_label_from_module_name(module_name)
        .unwrap_or_default()
        .to_lowercase();
    substituted.replace("%(app_label)s", &app_label)
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
    if candidate.is_ascii() && query.is_ascii() {
        candidate.eq_ignore_ascii_case(query)
    } else {
        candidate.to_lowercase() == query.to_lowercase()
    }
}

fn unresolved_import_path_reason(
    reason: ModelImportPathUnresolvedReason,
    binding: Option<&str>,
) -> RelationTargetUnresolvedReason {
    match reason {
        ModelImportPathUnresolvedReason::MissingBinding
        | ModelImportPathUnresolvedReason::ShadowedBinding => {
            RelationTargetUnresolvedReason::MissingImportBinding {
                binding: binding.unwrap_or_default().to_string(),
            }
        }
        ModelImportPathUnresolvedReason::InvalidTarget { target } => {
            RelationTargetUnresolvedReason::InvalidImportedTarget { target }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ModelDef {
    #[serde(skip)]
    pub(crate) file: File,
    pub(crate) name: Spanned<ModelName>,
    pub(crate) module_name: PythonModuleName,
    pub(crate) relations: Vec<Relation>,
    pub(crate) kind: ModelKind,
}

impl ModelDef {
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        module_name: PythonModuleName,
        file: File,
        name_span: Span,
    ) -> Self {
        Self {
            file,
            name: Spanned::new(ModelName::new(name), name_span),
            module_name,
            relations: Vec::new(),
            kind: ModelKind::Concrete,
        }
    }

    #[cfg(test)]
    fn push_local_relation(&mut self, relation: Relation) {
        self.relations.push(relation);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) enum BaseOutcome {
    DjangoModelRoot {
        span: Span,
    },
    Model {
        model: ModelId,
        span: Span,
    },
    NonModelClass {
        class: ClassId,
        span: Span,
    },
    Unresolved {
        span: Span,
        reason: BaseUnresolvedReason,
    },
}

impl BaseOutcome {
    #[must_use]
    pub(crate) fn span(&self) -> Span {
        match self {
            Self::DjangoModelRoot { span }
            | Self::Model { span, .. }
            | Self::NonModelClass { span, .. }
            | Self::Unresolved { span, .. } => *span,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) enum BaseUnresolvedReason {
    UnsupportedExpression,
    MissingImportBinding {
        path: Vec<String>,
    },
    ShadowedImportBinding {
        path: Vec<String>,
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
    PartialImport {
        resolved_prefix: PythonModuleName,
        unresolved_tail: Vec<String>,
    },
    ClassNotFound {
        class: ClassId,
    },
    ReboundLocalBase {
        class: ClassId,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) enum AncestryOutcome {
    Complete { mro: Vec<ClassId> },
    Partial,
    Invalid { reason: InvalidAncestryReason },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) enum InvalidAncestryReason {
    Cycle,
    DuplicateDjangoModelRoot,
    DuplicateClassBase { class: ClassId },
    InconsistentMethodResolutionOrder,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InheritanceRecord {
    pub(crate) bases: Vec<BaseOutcome>,
    pub(crate) ancestry: AncestryOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ModelRecord {
    definition: ModelDef,
    inheritance: InheritanceRecord,
    local_bindings: BTreeMap<FieldName, crate::models::extract::LocalBinding>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct RelationDeclarationId {
    model: ModelId,
    index: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct RelationBindingId {
    owner: ModelId,
    declaration: RelationDeclarationId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RelationBinding {
    owner: ModelId,
    declaration: RelationDeclarationId,
    resolution: RelationTargetResolution,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ModelRelationBindings {
    local_relation_bindings: BTreeMap<FieldName, RelationBindingId>,
    local_bound_names: BTreeSet<FieldName>,
    effective_forward_bindings: BTreeMap<FieldName, RelationBindingId>,
}

/// Dependency graph of Django models and their relations.
///
/// Models are keyed by deterministic import identity (`ModelId`) so
/// identically-named models from different importable modules can coexist.
#[derive(Debug, Clone, Default)]
pub struct ModelGraph {
    records: BTreeMap<ModelId, ModelRecord>,
    model_ids_by_name: FxHashMap<ModelName, BTreeSet<ModelId>>,
    model_ids_by_class: BTreeMap<ClassId, ModelId>,
    relation_bindings: BTreeMap<RelationBindingId, RelationBinding>,
    model_relation_bindings: BTreeMap<ModelId, ModelRelationBindings>,
    forward_relation_targets: FxHashMap<ModelId, FxHashMap<FieldName, Option<ModelId>>>,
    reverse_relation_bindings: FxHashMap<ModelId, FxHashMap<FieldName, ModelId>>,
    non_model_class_bindings: BTreeMap<ClassId, BTreeSet<FieldName>>,
}

impl PartialEq for ModelGraph {
    fn eq(&self, other: &Self) -> bool {
        self.records == other.records
            && self.relation_bindings == other.relation_bindings
            && self.model_relation_bindings == other.model_relation_bindings
            && self.forward_relation_targets == other.forward_relation_targets
            && self.reverse_relation_bindings == other.reverse_relation_bindings
            && self.non_model_class_bindings == other.non_model_class_bindings
    }
}

impl Eq for ModelGraph {}

#[derive(Serialize)]
struct SerializedRelation<'a> {
    #[serde(flatten)]
    declaration: &'a Relation,
    #[serde(skip_serializing_if = "RelationTargetResolution::is_file_local_placeholder")]
    resolution: &'a RelationTargetResolution,
}

#[derive(Serialize)]
struct SerializedModel<'a> {
    name: &'a Spanned<ModelName>,
    module_name: &'a PythonModuleName,
    relations: Vec<SerializedRelation<'a>>,
    kind: ModelKind,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    unresolved_bases: Vec<&'a BaseOutcome>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ancestry: Option<&'a AncestryOutcome>,
}

#[derive(Serialize)]
struct SerializedGraph<'a> {
    models: BTreeMap<&'a ModelId, SerializedModel<'a>>,
}

impl Serialize for ModelGraph {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let models: BTreeMap<&ModelId, SerializedModel<'_>> = self
            .records
            .iter()
            .map(|(id, record)| {
                let relations = self
                    .owned_relation_bindings(id)
                    .map(|(binding, relation)| SerializedRelation {
                        declaration: relation,
                        resolution: &binding.resolution,
                    })
                    .collect();
                let model = &record.definition;
                let unresolved_bases = record
                    .inheritance
                    .bases
                    .iter()
                    .filter(|base| matches!(base, BaseOutcome::Unresolved { .. }))
                    .collect();
                let ancestry = match &record.inheritance.ancestry {
                    AncestryOutcome::Complete { .. } => None,
                    AncestryOutcome::Partial | AncestryOutcome::Invalid { .. } => {
                        Some(&record.inheritance.ancestry)
                    }
                };
                (
                    id,
                    SerializedModel {
                        name: &model.name,
                        module_name: &model.module_name,
                        relations,
                        kind: model.kind,
                        unresolved_bases,
                        ancestry,
                    },
                )
            })
            .collect();
        SerializedGraph { models }.serialize(serializer)
    }
}

impl ModelGraph {
    #[must_use]
    pub fn empty_ref() -> &'static Self {
        static EMPTY: std::sync::LazyLock<ModelGraph> = std::sync::LazyLock::new(ModelGraph::new);
        &EMPTY
    }

    #[must_use]
    pub(crate) fn new() -> Self {
        Self::default()
    }

    #[cfg(debug_assertions)]
    pub(super) fn debug_assert_no_file_local_placeholders(&self) {
        let placeholders: Vec<String> = self
            .relation_bindings
            .values()
            .filter(|binding| binding.resolution.is_file_local_placeholder())
            .map(|binding| {
                format!(
                    "{}.{}.{}",
                    binding.owner.module_name.as_str(),
                    binding.owner.name.as_str(),
                    self.relation_declaration(&binding.declaration)
                        .map_or("<missing>", |relation| relation.field_name.value().as_str())
                )
            })
            .collect();
        debug_assert!(
            placeholders.is_empty(),
            "model graph still has file-local relation target placeholders: {}",
            placeholders.join(", ")
        );
    }

    pub(super) fn insert_resolved_model(
        &mut self,
        definition: ModelDef,
        inheritance: InheritanceRecord,
        local_bindings: BTreeMap<FieldName, crate::models::extract::LocalBinding>,
    ) {
        let id = ModelId::new(
            definition.module_name.clone(),
            definition.name.value().clone(),
        );
        assert!(
            self.records
                .insert(
                    id.clone(),
                    ModelRecord {
                        definition,
                        inheritance,
                        local_bindings,
                    },
                )
                .is_none(),
            "resolved model inserted more than once: {id:?}",
        );
        self.model_ids_by_name
            .entry(id.name.clone())
            .or_default()
            .insert(id.clone());
        self.model_ids_by_class
            .insert(ClassId::from_model_id(&id), id);
    }

    #[cfg(test)]
    fn add_standalone_model(&mut self, definition: ModelDef) {
        let id = ModelId::new(
            definition.module_name.clone(),
            definition.name.value().clone(),
        );
        let class = ClassId::from_model_id(&id);
        let local_bindings = definition
            .relations
            .iter()
            .enumerate()
            .map(|(index, relation)| {
                (
                    relation.field_name.value().clone(),
                    crate::models::extract::LocalBinding::Relation(index),
                )
            })
            .collect();
        self.insert_resolved_model(
            definition,
            InheritanceRecord {
                bases: Vec::new(),
                ancestry: AncestryOutcome::Complete { mro: vec![class] },
            },
            local_bindings,
        );
        self.build_effective_relation_bindings();
    }

    fn install_local_relation_bindings(
        &mut self,
        id: &ModelId,
        local_bindings: BTreeMap<FieldName, crate::models::extract::LocalBinding>,
    ) {
        let local_bound_names = local_bindings.keys().cloned().collect();
        let mut local_relation_bindings = BTreeMap::new();
        for (field_name, binding) in local_bindings {
            let crate::models::extract::LocalBinding::Relation(index) = binding else {
                continue;
            };
            let declaration = RelationDeclarationId {
                model: id.clone(),
                index,
            };
            let binding_id = RelationBindingId {
                owner: id.clone(),
                declaration: declaration.clone(),
            };
            self.relation_bindings.insert(
                binding_id.clone(),
                RelationBinding {
                    owner: id.clone(),
                    declaration,
                    resolution: RelationTargetResolution::file_local(),
                },
            );
            local_relation_bindings.insert(field_name, binding_id);
        }
        let effective_forward_bindings = local_relation_bindings.clone();
        assert!(
            self.model_relation_bindings
                .insert(
                    id.clone(),
                    ModelRelationBindings {
                        local_relation_bindings,
                        local_bound_names,
                        effective_forward_bindings,
                    },
                )
                .is_none(),
            "relation bindings installed more than once: {id:?}",
        );
    }

    pub(super) fn add_non_model_class(
        &mut self,
        id: &ClassId,
        local_bindings: BTreeMap<FieldName, crate::models::extract::LocalBinding>,
    ) {
        assert!(
            self.non_model_class_bindings
                .insert(id.clone(), local_bindings.into_keys().collect())
                .is_none(),
            "non-model class inserted more than once: {id:?}",
        );
    }

    #[cfg(test)]
    pub(super) fn contains_model(&self, id: &ModelId) -> bool {
        self.records.contains_key(id)
    }

    pub(crate) fn inheritance(&self, id: &ModelId) -> Option<&InheritanceRecord> {
        self.records.get(id).map(|record| &record.inheritance)
    }

    pub(super) fn build_effective_relation_bindings(&mut self) {
        self.relation_bindings.clear();
        let records = &self.records;
        self.model_relation_bindings
            .retain(|id, _bindings| !records.contains_key(id));

        let local_bindings: Vec<_> = self
            .records
            .iter()
            .map(|(id, record)| (id.clone(), record.local_bindings.clone()))
            .collect();
        for (id, bindings) in local_bindings {
            self.install_local_relation_bindings(&id, bindings);
        }

        let mut complete: Vec<(ModelId, Vec<ClassId>)> = self
            .records
            .iter()
            .filter_map(|(id, record)| match &record.inheritance.ancestry {
                AncestryOutcome::Complete { mro } => Some((id.clone(), mro.clone())),
                AncestryOutcome::Partial | AncestryOutcome::Invalid { .. } => None,
            })
            .collect();
        complete.sort_by_key(|(_id, mro)| mro.len());

        for (id, mro) in &complete {
            self.clone_abstract_relation_bindings(id, mro);
        }
        for (id, mro) in complete {
            self.install_effective_relation_bindings(id, &mro);
        }
        self.rebuild_reverse_relation_bindings();
    }

    fn model_id_for_class(&self, class: &ClassId) -> Option<&ModelId> {
        self.model_ids_by_class.get(class)
    }

    fn class_local_bound_names(&self, class: &ClassId) -> BTreeSet<FieldName> {
        self.model_id_for_class(class)
            .and_then(|model| self.model_relation_bindings.get(model))
            .map(|bindings| bindings.local_bound_names.clone())
            .or_else(|| self.non_model_class_bindings.get(class).cloned())
            .unwrap_or_default()
    }

    fn class_local_relation_bindings(
        &self,
        class: &ClassId,
    ) -> BTreeMap<FieldName, RelationBindingId> {
        self.model_id_for_class(class)
            .and_then(|model| self.model_relation_bindings.get(model))
            .map(|bindings| bindings.local_relation_bindings.clone())
            .unwrap_or_default()
    }

    fn clone_abstract_relation_bindings(&mut self, id: &ModelId, mro: &[ClassId]) {
        let mut occupied = self
            .model_relation_bindings
            .get(id)
            .map(|bindings| bindings.local_bound_names.clone())
            .unwrap_or_default();
        let mut local_relations = self
            .model_relation_bindings
            .get(id)
            .map(|bindings| bindings.local_relation_bindings.clone())
            .unwrap_or_default();
        let mut cloned_names = BTreeSet::new();

        for ancestor_id in mro.iter().skip(1) {
            let ancestor_names = self.class_local_bound_names(ancestor_id);
            let ancestor_relations = self.class_local_relation_bindings(ancestor_id);
            let ancestor_is_abstract = self
                .model_id_for_class(ancestor_id)
                .and_then(|model| self.records.get(model))
                .is_some_and(|record| record.definition.kind == ModelKind::Abstract);

            for name in ancestor_names {
                if !occupied.insert(name.clone()) || !ancestor_is_abstract {
                    continue;
                }
                let Some(ancestor_binding_id) = ancestor_relations.get(&name) else {
                    continue;
                };
                let Some(ancestor_binding) =
                    self.relation_bindings.get(ancestor_binding_id).cloned()
                else {
                    continue;
                };
                let binding_id = RelationBindingId {
                    owner: id.clone(),
                    declaration: ancestor_binding.declaration.clone(),
                };
                self.relation_bindings.insert(
                    binding_id.clone(),
                    RelationBinding {
                        owner: id.clone(),
                        declaration: ancestor_binding.declaration,
                        resolution: RelationTargetResolution::file_local(),
                    },
                );
                local_relations.insert(name.clone(), binding_id);
                cloned_names.insert(name);
            }
        }

        let bindings = self.model_relation_bindings.entry(id.clone()).or_default();
        bindings.local_bound_names.extend(cloned_names);
        bindings.local_relation_bindings = local_relations;
    }

    fn install_effective_relation_bindings(&mut self, id: ModelId, mro: &[ClassId]) {
        let mut occupied = self
            .model_relation_bindings
            .get(&id)
            .map(|bindings| bindings.local_bound_names.clone())
            .unwrap_or_default();
        let mut forward = self
            .model_relation_bindings
            .get(&id)
            .map(|bindings| bindings.local_relation_bindings.clone())
            .unwrap_or_default();

        for ancestor_id in mro.iter().skip(1) {
            let ancestor_names = self.class_local_bound_names(ancestor_id);
            let ancestor_relations = self.class_local_relation_bindings(ancestor_id);
            for name in ancestor_names {
                if occupied.insert(name.clone())
                    && let Some(binding_id) = ancestor_relations.get(&name)
                {
                    forward.insert(name, binding_id.clone());
                }
            }
        }

        self.model_relation_bindings
            .entry(id)
            .or_default()
            .effective_forward_bindings = forward;
    }

    fn relation_declaration(&self, id: &RelationDeclarationId) -> Option<&Relation> {
        self.records
            .get(&id.model)?
            .definition
            .relations
            .get(id.index)
    }

    fn owned_relation_bindings<'a>(
        &'a self,
        owner: &'a ModelId,
    ) -> impl Iterator<Item = (&'a RelationBinding, &'a Relation)> + 'a {
        let mut bindings: Vec<_> = self
            .model_relation_bindings
            .get(owner)
            .into_iter()
            .flat_map(|bindings| bindings.local_relation_bindings.values())
            .filter_map(|binding_id| {
                let binding = self.relation_bindings.get(binding_id)?;
                self.relation_declaration(&binding.declaration)
                    .map(|relation| (binding, relation))
            })
            .collect();
        bindings.sort_by_key(|(binding, _relation)| {
            let ancestry_order = if binding.declaration.model == *owner {
                usize::MAX
            } else {
                self.records
                    .get(owner)
                    .and_then(|record| match &record.inheritance.ancestry {
                        AncestryOutcome::Complete { mro } => mro.iter().position(|class| {
                            class == &ClassId::from_model_id(&binding.declaration.model)
                        }),
                        AncestryOutcome::Partial | AncestryOutcome::Invalid { .. } => None,
                    })
                    .unwrap_or(usize::MAX - 1)
            };
            (ancestry_order, binding.declaration.index)
        });
        bindings.into_iter()
    }

    pub(crate) fn owned_relation_entries<'a>(
        &'a self,
        owner: &'a ModelId,
    ) -> impl Iterator<Item = (&'a Relation, &'a RelationTargetResolution)> + 'a {
        self.owned_relation_bindings(owner)
            .map(|(binding, relation)| (relation, &binding.resolution))
    }

    #[must_use]
    pub fn get_by_id(&self, id: &ModelId) -> Option<&ModelDef> {
        self.records.get(id).map(|record| &record.definition)
    }

    #[must_use = "iterators are lazy and do nothing unless consumed"]
    pub fn models_named<'a>(
        &'a self,
        name: &'a str,
    ) -> impl Iterator<Item = (&'a ModelId, &'a ModelDef)> {
        self.model_ids_by_name
            .get(name)
            .into_iter()
            .flat_map(move |ids| {
                ids.iter().filter_map(|id| {
                    self.records
                        .get_key_value(id)
                        .map(|(id, record)| (id, &record.definition))
                })
            })
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.records.len()
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

        self.records.iter().find_map(|(id, record)| {
            (django_name_matches(id.name.as_str(), name)
                && app_label_from_module_name(id.module_name.as_str())
                    .is_some_and(|candidate| django_name_matches(candidate, app_label)))
            .then_some((id, &record.definition))
        })
    }

    fn lookup_entry_exact(&self, app_label: &str, name: &str) -> Option<(&ModelId, &ModelDef)> {
        self.model_ids_by_name.get(name)?.iter().find_map(|id| {
            let (id, record) = self.records.get_key_value(id)?;
            if app_label_from_module_name(id.module_name.as_str()) == Some(app_label) {
                Some((id, &record.definition))
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
        let target = self
            .forward_relation_targets
            .get(scope)?
            .get(field_name)?
            .as_ref()?;
        self.records.get(target).map(|record| &record.definition)
    }

    fn resolve_relation_target_entry(
        &self,
        binding: &RelationBinding,
        relation: &Relation,
    ) -> Option<(&ModelId, &ModelDef)> {
        match &binding.resolution {
            RelationTargetResolution::Resolved(id) => self
                .records
                .get_key_value(id)
                .map(|(id, record)| (id, &record.definition)),
            RelationTargetResolution::Unresolved {
                reason: RelationTargetUnresolvedReason::FileLocal,
            } => relation.target_model().and_then(|target| {
                self.resolve_target_entry(&binding.owner, &binding.declaration.model, target)
            }),
            RelationTargetResolution::Ambiguous { .. }
            | RelationTargetResolution::Partial { .. }
            | RelationTargetResolution::Unresolved { .. } => None,
        }
    }

    fn resolve_target_entry(
        &self,
        recipient_scope: &ModelId,
        declaration_scope: &ModelId,
        target: &RelationTarget,
    ) -> Option<(&ModelId, &ModelDef)> {
        match target {
            RelationTarget::SelfRef => self
                .records
                .get_key_value(recipient_scope)
                .map(|(id, record)| (id, &record.definition)),
            RelationTarget::Bare {
                name,
                import_reference: None,
            } => {
                let app_label = app_label_from_module_name(recipient_scope.module_name.as_str())?;
                self.lookup_entry(app_label, name.as_str())
            }
            RelationTarget::Bare {
                name,
                import_reference:
                    Some(ModelImportReference::Unresolved(
                        ModelImportPathUnresolvedReason::MissingBinding,
                    )),
            } => {
                let app_label = app_label_from_module_name(declaration_scope.module_name.as_str())?;
                self.lookup_entry(app_label, name.as_str())
            }
            RelationTarget::Bare {
                import_reference: Some(_),
                ..
            }
            | RelationTarget::Attribute { .. } => None,
            RelationTarget::Qualified { app_label, name } => {
                self.lookup_entry(app_label, name.as_str())
            }
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
        self.records.iter().flat_map(move |(source_id, record)| {
            let model = &record.definition;
            (model.kind == ModelKind::Concrete)
                .then(|| {
                    self.owned_relation_bindings(source_id).filter_map(
                        move |(binding, relation)| {
                            let (target_id, _target_model) =
                                self.resolve_relation_target_entry(binding, relation)?;
                            if target_id != scope {
                                return None;
                            }

                            relation
                                .effective_related_name(
                                    model.name.value().as_str(),
                                    model.module_name.as_str(),
                                )
                                .map(|name| (source_id, name))
                        },
                    )
                })
                .into_iter()
                .flatten()
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
        if let Some(target) = self
            .forward_relation_targets
            .get(scope)
            .and_then(|relations| relations.get(field_name))
        {
            return target
                .as_ref()
                .and_then(|target| self.records.get(target))
                .map(|record| &record.definition);
        }

        self.resolve_reverse_relation(scope, field_name)
            .and_then(|source_id| self.get_by_id(source_id))
    }

    fn resolve_reverse_relation(&self, scope: &ModelId, field_name: &str) -> Option<&ModelId> {
        self.reverse_relation_bindings.get(scope)?.get(field_name)
    }

    fn rebuild_reverse_relation_bindings(&mut self) {
        let mut forward_targets: FxHashMap<ModelId, FxHashMap<FieldName, Option<ModelId>>> =
            FxHashMap::default();
        for (owner, bindings) in &self.model_relation_bindings {
            for (name, binding_id) in &bindings.effective_forward_bindings {
                let target = self
                    .relation_bindings
                    .get(binding_id)
                    .and_then(|binding| {
                        self.relation_declaration(&binding.declaration)
                            .map(|relation| (binding, relation))
                    })
                    .and_then(|(binding, relation)| {
                        self.resolve_relation_target_entry(binding, relation)
                    })
                    .map(|(target_id, _target_model)| target_id.clone());
                forward_targets
                    .entry(owner.clone())
                    .or_default()
                    .insert(name.clone(), target);
            }
        }

        let mut reverse_bindings: FxHashMap<ModelId, FxHashMap<FieldName, ModelId>> =
            FxHashMap::default();
        for (source_id, record) in &self.records {
            let model = &record.definition;
            if model.kind == ModelKind::Abstract {
                continue;
            }
            for (binding, relation) in self.owned_relation_bindings(source_id) {
                let Some((target_id, _target_model)) =
                    self.resolve_relation_target_entry(binding, relation)
                else {
                    continue;
                };
                let Some(name) = relation.effective_related_name(
                    model.name.value().as_str(),
                    model.module_name.as_str(),
                ) else {
                    continue;
                };
                reverse_bindings
                    .entry(target_id.clone())
                    .or_default()
                    .entry(FieldName::new(name))
                    .or_insert_with(|| source_id.clone());
            }
        }
        self.forward_relation_targets = forward_targets;
        self.reverse_relation_bindings = reverse_bindings;
    }

    pub(crate) fn resolve_relation_targets(&mut self, db: &dyn ProjectDb, project: Project) {
        let updates: Vec<_> = self
            .relation_bindings
            .iter()
            .filter_map(|(binding_id, binding)| {
                let relation = self.relation_declaration(&binding.declaration)?;
                Some((
                    binding_id.clone(),
                    self.resolve_relation_target(
                        db,
                        project,
                        &binding.owner,
                        &binding.declaration.model,
                        relation,
                    ),
                ))
            })
            .collect();

        for (binding_id, resolution) in updates {
            if let Some(binding) = self.relation_bindings.get_mut(&binding_id) {
                binding.resolution = resolution;
            }
        }
        self.rebuild_reverse_relation_bindings();
    }

    fn resolve_relation_target(
        &self,
        db: &dyn ProjectDb,
        project: Project,
        recipient_scope: &ModelId,
        declaration_scope: &ModelId,
        relation: &Relation,
    ) -> RelationTargetResolution {
        let Some(target) = relation.target_model() else {
            return RelationTargetResolution::Unresolved {
                reason: RelationTargetUnresolvedReason::NoStaticTarget,
            };
        };

        match target {
            RelationTarget::SelfRef => RelationTargetResolution::Resolved(recipient_scope.clone()),
            RelationTarget::Qualified { app_label, name } => self.resolve_app_label_target(
                app_label,
                name,
                RelationTargetUnresolvedReason::AppLabelTargetNotFound {
                    app_label: app_label.clone(),
                    name: name.clone(),
                },
            ),
            RelationTarget::Bare {
                name,
                import_reference,
            } => match import_reference {
                Some(ModelImportReference::Qualified(target)) => {
                    self.resolve_imported_relation_target(db, project, target)
                }
                Some(ModelImportReference::Unresolved(
                    ModelImportPathUnresolvedReason::MissingBinding,
                )) => self.resolve_same_app_target(declaration_scope, name),
                None => self.resolve_same_app_target(recipient_scope, name),
                Some(ModelImportReference::Unresolved(error)) => {
                    RelationTargetResolution::Unresolved {
                        reason: unresolved_import_path_reason(error.clone(), Some(name.as_str())),
                    }
                }
            },
            RelationTarget::Attribute {
                path,
                import_reference,
            } => match import_reference {
                ModelImportReference::Qualified(target) => {
                    self.resolve_imported_relation_target(db, project, target)
                }
                ModelImportReference::Unresolved(error) => RelationTargetResolution::Unresolved {
                    reason: unresolved_import_path_reason(
                        error.clone(),
                        path.first().map(String::as_str),
                    ),
                },
            },
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
            .records
            .iter()
            .filter(|(id, _record)| {
                django_name_matches(id.name.as_str(), name.as_str())
                    && app_label_from_module_name(id.module_name.as_str())
                        .is_some_and(|candidate| django_name_matches(candidate, app_label))
            })
            .map(|(id, _record)| id.clone())
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
            if self.records.contains_key(&candidate) {
                return RelationTargetResolution::Resolved(candidate);
            }
        }

        RelationTargetResolution::Partial {
            resolved_prefix: module.name().clone(),
            unresolved_tail,
        }
    }
}

#[cfg(test)]
mod tests {
    use camino::Utf8Path;
    use djls_testing::TestDatabase;

    use super::*;

    fn module_name(name: &str) -> PythonModuleName {
        PythonModuleName::parse(name).expect("test Python module name should be valid")
    }

    fn test_file() -> File {
        let db = TestDatabase::new();
        db.add_file("/test.py", "")
            .expect("model graph fixture should be added to the test database");
        db.file(Utf8Path::new("/test.py"))
            .expect("model graph fixture should exist in the test database")
    }

    fn test_span(start: u32) -> Span {
        Span::new(start, 1)
    }

    fn model_def(name: &str, module_name: PythonModuleName, line_hint: u32) -> ModelDef {
        ModelDef::new(name, module_name, test_file(), test_span(line_hint))
    }

    fn relation(field_name: &str, relation_type: RelationType) -> Relation {
        Relation::new(
            test_file(),
            Spanned::new(FieldName::new(field_name), test_span(0)),
            relation_type,
        )
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

        let user = model_def("User", module_name("auth.models"), 1);

        let mut order = model_def("Order", module_name("shop.models"), 1);
        order.push_local_relation(relation(
            "user",
            RelationType::ForeignKey {
                target: Spanned::new(
                    RelationTarget::Qualified {
                        app_label: "auth".into(),
                        name: ModelName::new("User"),
                    },
                    test_span(10),
                ),
                related_name: Some(RelatedName::Named("orders".into())),
            },
        ));

        let mut profile = model_def("Profile", module_name("accounts.models"), 1);
        profile.push_local_relation(relation(
            "user",
            RelationType::OneToOne {
                target: Spanned::new(
                    RelationTarget::Qualified {
                        app_label: "auth".into(),
                        name: ModelName::new("User"),
                    },
                    test_span(10),
                ),
                related_name: None,
            },
        ));

        graph.add_standalone_model(user);
        graph.add_standalone_model(order);
        graph.add_standalone_model(profile);
        graph
    }

    #[test]
    fn django_name_matching_preserves_unicode_case_folding() {
        assert!(django_name_matches("UserProfile", "userprofile"));
        assert!(django_name_matches("Äccount", "äccount"));
        assert!(!django_name_matches("User", "Group"));
    }

    #[test]
    fn forward_lookup() {
        let graph = user_order_graph();
        assert_eq!(
            graph
                .resolve_forward(model_id(&graph, "Order"), "user")
                .map(|model| model.name.value().as_str()),
            Some("User")
        );
        assert_eq!(
            graph
                .resolve_forward(model_id(&graph, "Profile"), "user")
                .map(|model| model.name.value().as_str()),
            Some("User")
        );
        assert_eq!(
            graph
                .resolve_forward(model_id(&graph, "User"), "user")
                .map(|model| model.name.value().as_str()),
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
                .map(|model| model.name.value().as_str()),
            Some("User")
        );
        assert_eq!(
            graph
                .resolve_relation(model_id(&graph, "User"), "orders")
                .map(|model| model.name.value().as_str()),
            Some("Order")
        );
        assert_eq!(
            graph
                .resolve_relation(model_id(&graph, "User"), "profile")
                .map(|model| model.name.value().as_str()),
            Some("Profile")
        );
    }

    #[test]
    fn unresolved_forward_relation_does_not_fall_through_to_reverse_lookup() {
        let mut graph = ModelGraph::new();

        let mut user = model_def("User", module_name("auth.models"), 1);
        user.push_local_relation(relation(
            "orders",
            RelationType::ForeignKey {
                target: Spanned::new(
                    RelationTarget::Qualified {
                        app_label: "missing".into(),
                        name: ModelName::new("Order"),
                    },
                    test_span(10),
                ),
                related_name: None,
            },
        ));

        let mut order = model_def("Order", module_name("shop.models"), 1);
        order.push_local_relation(relation(
            "user",
            RelationType::ForeignKey {
                target: Spanned::new(
                    RelationTarget::Qualified {
                        app_label: "auth".into(),
                        name: ModelName::new("User"),
                    },
                    test_span(10),
                ),
                related_name: Some(RelatedName::Named("orders".into())),
            },
        ));

        graph.add_standalone_model(user);
        graph.add_standalone_model(order);

        assert_eq!(
            graph
                .resolve_relation(model_id(&graph, "User"), "orders")
                .map(|model| model.name.value().as_str()),
            None
        );
    }

    #[test]
    fn default_related_name_fk() {
        let rel = relation(
            "user",
            RelationType::ForeignKey {
                target: Spanned::new(
                    RelationTarget::Bare {
                        name: ModelName::new("User"),
                        import_reference: None,
                    },
                    test_span(10),
                ),
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
        let rel = relation(
            "user",
            RelationType::OneToOne {
                target: Spanned::new(
                    RelationTarget::Bare {
                        name: ModelName::new("User"),
                        import_reference: None,
                    },
                    test_span(10),
                ),
                related_name: None,
            },
        );
        assert_eq!(
            rel.effective_related_name("Profile", "accounts.models"),
            Some("profile".into())
        );
    }

    #[test]
    fn suppressed_related_names_have_no_reverse_accessor() {
        for declared_name in ["+", "order+", "%(class)s+"] {
            let relation_type = RelationType::from_field_class(
                "ForeignKey",
                Spanned::new(
                    RelationTarget::Bare {
                        name: ModelName::new("User"),
                        import_reference: None,
                    },
                    test_span(10),
                ),
                Some(declared_name.into()),
            )
            .expect("ForeignKey should produce a relation type");
            assert!(matches!(
                relation_type,
                RelationType::ForeignKey {
                    related_name: Some(RelatedName::Suppressed),
                    ..
                }
            ));
            let relation = relation("user", relation_type);

            assert_eq!(
                relation.effective_related_name("Order", "shop.models"),
                None
            );
            for candidate in ["+", "order+", "foo+"] {
                assert!(!relation.effective_related_name_matches(
                    "Order",
                    "shop.models",
                    candidate
                ));
            }
        }
    }

    #[test]
    fn named_related_name_still_matches_after_substitution() {
        let relation_type = RelationType::from_field_class(
            "ForeignKey",
            Spanned::new(
                RelationTarget::Bare {
                    name: ModelName::new("User"),
                    import_reference: None,
                },
                test_span(10),
            ),
            Some("%(class)s_orders".into()),
        )
        .expect("ForeignKey should produce a relation type");
        assert!(matches!(
            relation_type,
            RelationType::ForeignKey {
                related_name: Some(RelatedName::Named(ref name)),
                ..
            } if name == "%(class)s_orders"
        ));
        let relation = relation("user", relation_type);

        assert!(relation.effective_related_name_matches(
            "SpecialOrder",
            "shop.models",
            "specialorder_orders"
        ));
        assert!(!relation.effective_related_name_matches(
            "SpecialOrder",
            "shop.models",
            "specialorder_set"
        ));
    }

    #[test]
    fn class_substitution_in_related_name() {
        let rel = relation(
            "user",
            RelationType::ForeignKey {
                target: Spanned::new(
                    RelationTarget::Bare {
                        name: ModelName::new("User"),
                        import_reference: None,
                    },
                    test_span(10),
                ),
                related_name: Some(RelatedName::Named("%(class)s_orders".into())),
            },
        );
        assert_eq!(
            rel.effective_related_name("SpecialOrder", "shop.models"),
            Some("specialorder_orders".into())
        );
    }

    #[test]
    fn app_label_substitution_in_related_name() {
        let rel = relation(
            "title",
            RelationType::ForeignKey {
                target: Spanned::new(
                    RelationTarget::Bare {
                        name: ModelName::new("Title"),
                        import_reference: None,
                    },
                    test_span(10),
                ),
                related_name: Some(RelatedName::Named(
                    "attached_%(app_label)s_%(class)s_set".into(),
                )),
            },
        );
        assert_eq!(
            rel.effective_related_name("Article", "blog.models"),
            Some("attached_blog_article_set".into())
        );
    }

    #[test]
    fn app_label_from_nested_module_name() {
        let rel = relation(
            "user",
            RelationType::ForeignKey {
                target: Spanned::new(
                    RelationTarget::Bare {
                        name: ModelName::new("User"),
                        import_reference: None,
                    },
                    test_span(10),
                ),
                related_name: Some(RelatedName::Named("%(app_label)s_%(class)s_set".into())),
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
        let rel = relation(
            "user",
            RelationType::ForeignKey {
                target: Spanned::new(
                    RelationTarget::Bare {
                        name: ModelName::new("User"),
                        import_reference: None,
                    },
                    test_span(10),
                ),
                related_name: Some(RelatedName::Named("%(app_label)s_%(class)s_set".into())),
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
        let rel = relation(
            "user",
            RelationType::ForeignKey {
                target: Spanned::new(
                    RelationTarget::Bare {
                        name: ModelName::new("User"),
                        import_reference: None,
                    },
                    test_span(10),
                ),
                related_name: Some(RelatedName::Named("%(app_label)s_set".into())),
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
        graph.add_standalone_model(model_def("User", module_name("zeta.models"), 1));
        graph.add_standalone_model(model_def("Group", module_name("auth.models"), 2));
        graph.add_standalone_model(model_def("User", module_name("alpha.models"), 3));

        let users: Vec<_> = graph
            .models_named("User")
            .map(|(id, model)| {
                (
                    id.module_name().as_str(),
                    model.name.value().as_str(),
                    model.name.span().start(),
                )
            })
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
        graph.add_standalone_model(model_def("User", module_name("auth.models"), 1));

        let exact = graph
            .lookup_entry_exact("auth", "User")
            .expect("exact lookup should find an exact app label and model name");
        assert_eq!(exact.1.name.value().as_str(), "User");

        assert!(graph.lookup_entry_exact("auth", "user").is_none());
        assert!(graph.lookup_entry_exact("AUTH", "User").is_none());
        assert_eq!(
            graph
                .lookup("auth", "user")
                .map(|model| model.name.span().start()),
            Some(1)
        );
    }

    #[test]
    fn lookup_falls_back_to_django_name_matching() {
        let mut graph = ModelGraph::new();
        graph.add_standalone_model(model_def("User", module_name("auth.models"), 1));
        graph.add_standalone_model(model_def("Éclair", module_name("Café.models"), 2));

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
        graph.add_standalone_model(model_def("User", module_name("accounts.models"), 1));

        let model = graph
            .lookup("ACCOUNTS", "user")
            .expect("lookup should normalize app label and model name");
        assert_eq!(model.name.value().as_str(), "User");
        assert_eq!(model.module_name.as_str(), "accounts.models");
    }

    #[test]
    fn relation_target_policy_resolves_self_bare_and_qualified() {
        let mut graph = ModelGraph::new();
        graph.add_standalone_model(model_def("User", module_name("accounts.models"), 1));
        graph.add_standalone_model(model_def("User", module_name("blog.models"), 1));

        let mut post = model_def("Post", module_name("blog.models"), 1);
        post.push_local_relation(relation(
            "author",
            RelationType::ForeignKey {
                target: Spanned::new(
                    RelationTarget::Bare {
                        name: ModelName::new("User"),
                        import_reference: None,
                    },
                    test_span(10),
                ),
                related_name: None,
            },
        ));
        post.push_local_relation(relation(
            "account_author",
            RelationType::ForeignKey {
                target: Spanned::new(
                    RelationTarget::Qualified {
                        app_label: "accounts".into(),
                        name: ModelName::new("User"),
                    },
                    test_span(10),
                ),
                related_name: None,
            },
        ));
        post.push_local_relation(relation(
            "parent",
            RelationType::ForeignKey {
                target: Spanned::new(RelationTarget::SelfRef, test_span(10)),
                related_name: None,
            },
        ));
        graph.add_standalone_model(post);

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
        let rel = relation(
            "content_object",
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
        let mut model = model_def("TaggedItem", module_name("tagging.models"), 1);
        model.push_local_relation(relation(
            "content_object",
            RelationType::GenericForeignKey {
                ct_field: "content_type".into(),
                fk_field: "object_id".into(),
            },
        ));
        graph.add_standalone_model(model);

        assert_eq!(
            graph.resolve_forward(model_id(&graph, "TaggedItem"), "content_object"),
            None
        );
    }

    #[test]
    fn standalone_model_helper_installs_complete_self_ancestry() {
        let mut graph = ModelGraph::new();
        graph.add_standalone_model(model_def("User", module_name("auth.models"), 1));

        let id = model_id(&graph, "User");
        let inheritance = graph
            .inheritance(id)
            .expect("added model should have inheritance");
        assert!(inheritance.bases.is_empty());
        assert_eq!(
            inheritance.ancestry,
            AncestryOutcome::Complete {
                mro: vec![ClassId::from_model_id(id)]
            }
        );
    }

    #[test]
    fn same_named_models_in_different_modules_coexist() {
        let mut graph = ModelGraph::new();
        graph.add_standalone_model(model_def("Comment", module_name("blog.models"), 1));
        graph.add_standalone_model(model_def("Comment", module_name("news.models"), 1));

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
