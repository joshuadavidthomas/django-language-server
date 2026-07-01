use std::borrow::Borrow;
use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use serde::Deserialize;
use serde::Serialize;

use crate::python::PythonModuleName;

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
    // Keep `name` first: `ModelGraph::models_named` and exact lookups rely
    // on the derived `Ord` grouping same-named models together.
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

impl TryFrom<String> for ModelId {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        let (module_name, name) = value
            .rsplit_once('.')
            .ok_or_else(|| "model id must include a module name and model name".to_string())?;
        if name.is_empty() {
            return Err("model id must include a model name".to_string());
        }

        let module_name = PythonModuleName::parse(module_name).map_err(|err| err.to_string())?;
        Ok(Self::new(module_name, ModelName::new(name)))
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
pub(crate) struct Relation {
    pub(crate) field_name: FieldName,
    #[serde(flatten)]
    pub(crate) relation_type: RelationType,
}

impl Relation {
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

    #[must_use]
    fn id(&self) -> ModelId {
        ModelId::new(self.module_name.clone(), self.name.clone())
    }
}

/// Dependency graph of Django models and their relations.
///
/// Models are keyed by deterministic import identity (`ModelId`) so
/// identically-named models from different importable modules can coexist.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelGraph {
    models: BTreeMap<ModelId, ModelDef>,
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

    pub(crate) fn add_model(&mut self, model: ModelDef) {
        self.models.insert(model.id(), model);
    }

    #[must_use]
    pub fn get_by_id(&self, id: &ModelId) -> Option<&ModelDef> {
        self.models.get(id)
    }

    pub fn models_named<'a>(
        &'a self,
        name: &'a str,
    ) -> impl Iterator<Item = (&'a ModelId, &'a ModelDef)> {
        self.models
            .iter()
            .skip_while(move |(id, _model)| id.name.as_str() < name)
            .take_while(move |(id, _model)| id.name.as_str() == name)
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
        for (id, model) in &self.models {
            let candidate_name = id.name.as_str();
            if candidate_name < name {
                continue;
            }
            if candidate_name != name {
                break;
            }
            if app_label_from_module_name(id.module_name.as_str()) == Some(app_label) {
                return Some((id, model));
            }
        }

        None
    }

    /// Look up the target model for a forward relation field on `scope`.
    ///
    /// Skips `GenericForeignKey` relations (no static target).
    #[must_use]
    pub fn resolve_forward(&self, scope: &ModelId, field_name: &str) -> Option<&ModelDef> {
        let target = self.forward_relation(scope, field_name)?.target_model()?;
        self.resolve_target(scope, target)
    }

    fn forward_relation(&self, scope: &ModelId, field_name: &str) -> Option<&Relation> {
        let model = self.get_by_id(scope)?;
        model
            .relations
            .iter()
            .find(|relation| relation.field_name.as_str() == field_name)
    }

    fn resolve_target(&self, scope: &ModelId, target: &RelationTarget) -> Option<&ModelDef> {
        self.resolve_target_entry(scope, target)
            .map(|(_id, model)| model)
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
                let target = relation.target_model()?;
                let (target_id, _target_model) = self.resolve_target_entry(source_id, target)?;
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
            let target = relation.target_model()?;
            return self.resolve_target(scope, target);
        }

        self.resolve_reverse_relation(scope, field_name)
            .and_then(|source_id| self.get_by_id(source_id))
    }

    fn resolve_reverse_relation(&self, scope: &ModelId, field_name: &str) -> Option<&ModelId> {
        for (source_id, model) in &self.models {
            for relation in &model.relations {
                let Some(target) = relation.target_model() else {
                    continue;
                };
                let Some((target_id, _target_model)) = self.resolve_target_entry(source_id, target)
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

    /// Merge another graph into this one.
    ///
    /// Models from `other` overwrite models with the same import identity in `self`.
    pub fn merge(&mut self, other: ModelGraph) {
        self.models.extend(other.models);
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

    fn resolved_model_name(model: Option<&ModelDef>) -> Option<&str> {
        model.map(|model| model.name.as_str())
    }

    fn user_order_graph() -> ModelGraph {
        let mut graph = ModelGraph::new();

        let user = ModelDef::new("User", module_name("auth.models"), 1);

        let mut order = ModelDef::new("Order", module_name("shop.models"), 1);
        order.relations.push(Relation {
            field_name: "user".into(),
            relation_type: RelationType::ForeignKey {
                target: RelationTarget::Qualified {
                    app_label: "auth".into(),
                    name: ModelName::new("User"),
                },
                related_name: Some("orders".into()),
            },
        });

        let mut profile = ModelDef::new("Profile", module_name("accounts.models"), 1);
        profile.relations.push(Relation {
            field_name: "user".into(),
            relation_type: RelationType::OneToOne {
                target: RelationTarget::Qualified {
                    app_label: "auth".into(),
                    name: ModelName::new("User"),
                },
                related_name: None,
            },
        });

        graph.add_model(user);
        graph.add_model(order);
        graph.add_model(profile);
        graph
    }

    #[test]
    fn forward_lookup() {
        let graph = user_order_graph();
        assert_eq!(
            resolved_model_name(graph.resolve_forward(model_id(&graph, "Order"), "user")),
            Some("User")
        );
        assert_eq!(
            resolved_model_name(graph.resolve_forward(model_id(&graph, "Profile"), "user")),
            Some("User")
        );
        assert_eq!(
            resolved_model_name(graph.resolve_forward(model_id(&graph, "User"), "user")),
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
            resolved_model_name(graph.resolve_relation(model_id(&graph, "Order"), "user")),
            Some("User")
        );
        assert_eq!(
            resolved_model_name(graph.resolve_relation(model_id(&graph, "User"), "orders")),
            Some("Order")
        );
        assert_eq!(
            resolved_model_name(graph.resolve_relation(model_id(&graph, "User"), "profile")),
            Some("Profile")
        );
    }

    #[test]
    fn unresolved_forward_relation_does_not_fall_through_to_reverse_lookup() {
        let mut graph = ModelGraph::new();

        let mut user = ModelDef::new("User", module_name("auth.models"), 1);
        user.relations.push(Relation {
            field_name: "orders".into(),
            relation_type: RelationType::ForeignKey {
                target: RelationTarget::Qualified {
                    app_label: "missing".into(),
                    name: ModelName::new("Order"),
                },
                related_name: None,
            },
        });

        let mut order = ModelDef::new("Order", module_name("shop.models"), 1);
        order.relations.push(Relation {
            field_name: "user".into(),
            relation_type: RelationType::ForeignKey {
                target: RelationTarget::Qualified {
                    app_label: "auth".into(),
                    name: ModelName::new("User"),
                },
                related_name: Some("orders".into()),
            },
        });

        graph.add_model(user);
        graph.add_model(order);

        assert_eq!(
            resolved_model_name(graph.resolve_relation(model_id(&graph, "User"), "orders")),
            None
        );
    }

    #[test]
    fn default_related_name_fk() {
        let rel = Relation {
            field_name: "user".into(),
            relation_type: RelationType::ForeignKey {
                target: RelationTarget::Bare {
                    name: ModelName::new("User"),
                },
                related_name: None,
            },
        };
        assert_eq!(
            rel.effective_related_name("Order", "shop.models"),
            Some("order_set".into())
        );
    }

    #[test]
    fn default_related_name_o2o() {
        let rel = Relation {
            field_name: "user".into(),
            relation_type: RelationType::OneToOne {
                target: RelationTarget::Bare {
                    name: ModelName::new("User"),
                },
                related_name: None,
            },
        };
        assert_eq!(
            rel.effective_related_name("Profile", "accounts.models"),
            Some("profile".into())
        );
    }

    #[test]
    fn class_substitution_in_related_name() {
        let rel = Relation {
            field_name: "user".into(),
            relation_type: RelationType::ForeignKey {
                target: RelationTarget::Bare {
                    name: ModelName::new("User"),
                },
                related_name: Some("%(class)s_orders".into()),
            },
        };
        assert_eq!(
            rel.effective_related_name("SpecialOrder", "shop.models"),
            Some("specialorder_orders".into())
        );
    }

    #[test]
    fn app_label_substitution_in_related_name() {
        let rel = Relation {
            field_name: "title".into(),
            relation_type: RelationType::ForeignKey {
                target: RelationTarget::Bare {
                    name: ModelName::new("Title"),
                },
                related_name: Some("attached_%(app_label)s_%(class)s_set".into()),
            },
        };
        assert_eq!(
            rel.effective_related_name("Article", "blog.models"),
            Some("attached_blog_article_set".into())
        );
    }

    #[test]
    fn app_label_from_nested_module_name() {
        let rel = Relation {
            field_name: "user".into(),
            relation_type: RelationType::ForeignKey {
                target: RelationTarget::Bare {
                    name: ModelName::new("User"),
                },
                related_name: Some("%(app_label)s_%(class)s_set".into()),
            },
        };
        assert_eq!(
            rel.effective_related_name("Permission", "django.contrib.auth.models"),
            Some("auth_permission_set".into())
        );
    }

    #[test]
    fn app_label_bare_models_path() {
        // A bare "models" module name has no valid app label — %(app_label)s
        // should substitute as empty rather than producing "models".
        let rel = Relation {
            field_name: "user".into(),
            relation_type: RelationType::ForeignKey {
                target: RelationTarget::Bare {
                    name: ModelName::new("User"),
                },
                related_name: Some("%(app_label)s_%(class)s_set".into()),
            },
        };
        assert_eq!(
            rel.effective_related_name("Order", "models"),
            Some("_order_set".into())
        );
    }

    #[test]
    fn app_label_no_models_component() {
        // When "models" doesn't appear, falls back to first component.
        let rel = Relation {
            field_name: "user".into(),
            relation_type: RelationType::ForeignKey {
                target: RelationTarget::Bare {
                    name: ModelName::new("User"),
                },
                related_name: Some("%(app_label)s_set".into()),
            },
        };
        assert_eq!(
            rel.effective_related_name("Order", "myapp"),
            Some("myapp_set".into())
        );
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
        post.relations.push(Relation {
            field_name: "author".into(),
            relation_type: RelationType::ForeignKey {
                target: RelationTarget::Bare {
                    name: ModelName::new("User"),
                },
                related_name: None,
            },
        });
        post.relations.push(Relation {
            field_name: "account_author".into(),
            relation_type: RelationType::ForeignKey {
                target: RelationTarget::Qualified {
                    app_label: "accounts".into(),
                    name: ModelName::new("User"),
                },
                related_name: None,
            },
        });
        post.relations.push(Relation {
            field_name: "parent".into(),
            relation_type: RelationType::ForeignKey {
                target: RelationTarget::SelfRef,
                related_name: None,
            },
        });
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
        let rel = Relation {
            field_name: "content_object".into(),
            relation_type: RelationType::GenericForeignKey {
                ct_field: "content_type".into(),
                fk_field: "object_id".into(),
            },
        };
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
        model.relations.push(Relation {
            field_name: "content_object".into(),
            relation_type: RelationType::GenericForeignKey {
                ct_field: "content_type".into(),
                fk_field: "object_id".into(),
            },
        });
        graph.add_model(model);

        assert_eq!(
            graph.resolve_forward(model_id(&graph, "TaggedItem"), "content_object"),
            None
        );
    }

    #[test]
    fn merge_graphs() {
        let mut g1 = ModelGraph::new();
        g1.add_model(ModelDef::new("User", module_name("auth.models"), 1));

        let mut g2 = ModelGraph::new();
        g2.add_model(ModelDef::new("Order", module_name("shop.models"), 1));

        g1.merge(g2);
        assert_eq!(g1.len(), 2);
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
