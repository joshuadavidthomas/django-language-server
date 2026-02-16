use std::borrow::Borrow;
use std::collections::BTreeMap;
use std::fmt;

use serde::Deserialize;
use serde::Serialize;

macro_rules! string_newtype {
    ($(#[doc = $doc:expr])* pub struct $Name:ident) => {
        $(#[doc = $doc])*
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $Name(String);

        impl $Name {
            #[must_use]
            pub fn new(value: impl Into<String>) -> Self {
                Self(value.into())
            }

            #[must_use]
            pub fn as_str(&self) -> &str {
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
                Self(s.to_owned())
            }
        }

        impl From<String> for $Name {
            fn from(s: String) -> Self {
                Self(s)
            }
        }

        impl fmt::Display for $Name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }
    };
}

string_newtype! {
    /// A Django model class name (e.g., `"User"`, `"Article"`).
    pub struct ModelName
}

string_newtype! {
    /// A dotted Python module path (e.g., `"myapp.models"`,
    /// `"django.contrib.auth.models"`).
    pub struct ModulePath
}

string_newtype! {
    /// A Python field/attribute name on a Django model (e.g., `"user"`,
    /// `"content_type"`).
    pub struct FieldName
}

/// The kind of relation a Django model field represents.
///
/// Each variant carries its own data, so fields that don't apply to a given
/// relation kind (e.g., `target_model` on a `GenericForeignKey`) are simply
/// absent rather than wrapped in `Option`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "relation_type")]
pub enum RelationType {
    ForeignKey {
        target_model: ModelName,
        related_name: Option<String>,
    },
    OneToOne {
        target_model: ModelName,
        related_name: Option<String>,
    },
    ManyToMany {
        target_model: ModelName,
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
    pub fn from_field_class(
        name: &str,
        target_model: ModelName,
        related_name: Option<String>,
    ) -> Option<Self> {
        match name {
            "ForeignKey" => Some(Self::ForeignKey {
                target_model,
                related_name,
            }),
            "OneToOneField" => Some(Self::OneToOne {
                target_model,
                related_name,
            }),
            "ManyToManyField" => Some(Self::ManyToMany {
                target_model,
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
pub enum ModelKind {
    Concrete,
    Abstract,
}

/// A relation field on a Django model.
///
/// The `relation_type` variant determines what data is available:
/// concrete relations (FK, O2O, M2M) carry a `target_model` and optional
/// `related_name`, while `GenericForeignKey` carries `ct_field`/`fk_field`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Relation {
    pub field_name: FieldName,
    #[serde(flatten)]
    pub relation_type: RelationType,
}

impl Relation {
    /// Get the target model if this is a concrete relation (FK, O2O, M2M).
    ///
    /// Returns `None` for `GenericForeignKey` since its target is determined
    /// at runtime via the content type.
    #[must_use]
    pub fn target_model(&self) -> Option<&ModelName> {
        match &self.relation_type {
            RelationType::ForeignKey { target_model, .. }
            | RelationType::OneToOne { target_model, .. }
            | RelationType::ManyToMany { target_model, .. } => Some(target_model),
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
    /// `module_path` is the dotted Python module path (e.g., `"myapp.models"`);
    /// the app label is derived as the component before `models`.
    #[must_use]
    pub fn effective_related_name(&self, source_model: &str, module_path: &str) -> Option<String> {
        let lower = source_model.to_lowercase();
        match &self.relation_type {
            RelationType::ForeignKey { related_name, .. }
            | RelationType::ManyToMany { related_name, .. } => Some(match related_name {
                Some(name) => substitute_related_name(name, &lower, module_path),
                None => format!("{lower}_set"),
            }),
            RelationType::OneToOne { related_name, .. } => Some(match related_name {
                Some(name) => substitute_related_name(name, &lower, module_path),
                None => lower,
            }),
            RelationType::GenericForeignKey { .. } => None,
        }
    }
}

fn substitute_related_name(template: &str, lower_model: &str, module_path: &str) -> String {
    let app_label = app_label_from_module_path(module_path).unwrap_or_default();
    template
        .replace("%(class)s", lower_model)
        .replace("%(app_label)s", &app_label)
}

/// Derive the app label from a dotted module path.
///
/// Mirrors Django's convention: the component immediately before `models`
/// in the module path. Returns `None` when no valid app label can be
/// determined (e.g., bare `"models"` with no package prefix, or an empty
/// path).
fn app_label_from_module_path(module_path: &str) -> Option<String> {
    let parts: Vec<&str> = module_path.split('.').collect();
    // Find the component right before "models"
    for (i, part) in parts.iter().enumerate() {
        if *part == "models" && i > 0 {
            return Some(parts[i - 1].to_lowercase());
        }
    }
    // Fallback: first component, but only if it isn't "models" itself
    // (which would mean we have a bare "models" path with no package).
    match parts.first() {
        Some(&"models" | &"") | None => None,
        Some(first) => Some(first.to_lowercase()),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelDef {
    pub name: ModelName,
    pub module_path: ModulePath,
    pub line: usize,
    pub relations: Vec<Relation>,
    pub kind: ModelKind,
}

impl ModelDef {
    #[must_use]
    pub fn new(name: impl Into<String>, module_path: impl Into<String>, line: usize) -> Self {
        Self {
            name: ModelName::new(name),
            module_path: ModulePath::new(module_path),
            line,
            relations: Vec::new(),
            kind: ModelKind::Concrete,
        }
    }
}

/// Dependency graph of Django models and their relations.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelGraph {
    models: BTreeMap<ModelName, ModelDef>,
}

impl ModelGraph {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_model(&mut self, model: ModelDef) {
        self.models.insert(model.name.clone(), model);
    }

    #[must_use]
    pub fn get(&self, name: &str) -> Option<&ModelDef> {
        self.models.get(name)
    }

    pub fn models(&self) -> impl Iterator<Item = &ModelDef> {
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

    /// Look up the target model for a forward relation field on `model_name`.
    ///
    /// Skips `GenericForeignKey` relations (no static target).
    #[must_use]
    pub fn resolve_forward(&self, model_name: &str, field_name: &str) -> Option<&str> {
        let model = self.models.get(model_name)?;
        model
            .relations
            .iter()
            .find(|r| r.field_name.as_str() == field_name)
            .and_then(|r| r.target_model())
            .map(ModelName::as_str)
    }

    /// Look up models that point at `model_name` via a reverse relation.
    ///
    /// Returns `(source_model_name, effective_related_name, relation)` triples.
    /// Skips `GenericForeignKey` relations.
    pub fn resolve_reverse<'a>(
        &'a self,
        model_name: &'a str,
    ) -> impl Iterator<Item = (&'a str, String, &'a Relation)> {
        self.models.values().flat_map(move |m| {
            m.relations
                .iter()
                .filter(move |r| r.target_model().is_some_and(|t| t.as_str() == model_name))
                .filter_map(move |r| {
                    r.effective_related_name(m.name.as_str(), m.module_path.as_str())
                        .map(|name| (m.name.as_str(), name, r))
                })
        })
    }

    /// Resolve a field access on a model — checks forward relations first,
    /// then reverse relations. Returns the resolved model name.
    #[must_use]
    pub fn resolve_relation<'a>(
        &'a self,
        model_name: &'a str,
        field_name: &str,
    ) -> Option<&'a str> {
        // Forward
        if let Some(target) = self.resolve_forward(model_name, field_name) {
            return Some(target);
        }

        // Reverse
        for (source_name, related_name, _) in self.resolve_reverse(model_name) {
            if related_name == field_name {
                return Some(source_name);
            }
        }

        None
    }

    /// Merge another graph into this one.
    ///
    /// Models from `other` overwrite models with the same name in `self`.
    pub fn merge(&mut self, other: ModelGraph) {
        self.models.extend(other.models);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user_order_graph() -> ModelGraph {
        let mut graph = ModelGraph::new();

        let user = ModelDef::new("User", "auth.models", 1);

        let mut order = ModelDef::new("Order", "shop.models", 1);
        order.relations.push(Relation {
            field_name: "user".into(),
            relation_type: RelationType::ForeignKey {
                target_model: ModelName::new("User"),
                related_name: Some("orders".into()),
            },
        });

        let mut profile = ModelDef::new("Profile", "accounts.models", 1);
        profile.relations.push(Relation {
            field_name: "user".into(),
            relation_type: RelationType::OneToOne {
                target_model: ModelName::new("User"),
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
        assert_eq!(graph.resolve_forward("Order", "user"), Some("User"));
        assert_eq!(graph.resolve_forward("Profile", "user"), Some("User"));
        assert_eq!(graph.resolve_forward("User", "user"), None);
    }

    #[test]
    fn reverse_lookup() {
        let graph = user_order_graph();
        let reverses: Vec<_> = graph
            .resolve_reverse("User")
            .map(|(src, name, _)| (src.to_string(), name))
            .collect();
        assert!(reverses.contains(&("Order".into(), "orders".into())));
        assert!(reverses.contains(&("Profile".into(), "profile".into())));
    }

    #[test]
    fn resolve_relation_forward_and_reverse() {
        let graph = user_order_graph();
        // Forward
        assert_eq!(graph.resolve_relation("Order", "user"), Some("User"));
        // Reverse (explicit related_name)
        assert_eq!(graph.resolve_relation("User", "orders"), Some("Order"));
        // Reverse (default related_name for O2O)
        assert_eq!(graph.resolve_relation("User", "profile"), Some("Profile"));
    }

    #[test]
    fn default_related_name_fk() {
        let rel = Relation {
            field_name: "user".into(),
            relation_type: RelationType::ForeignKey {
                target_model: ModelName::new("User"),
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
                target_model: ModelName::new("User"),
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
                target_model: ModelName::new("User"),
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
                target_model: ModelName::new("Title"),
                related_name: Some("attached_%(app_label)s_%(class)s_set".into()),
            },
        };
        assert_eq!(
            rel.effective_related_name("Article", "blog.models"),
            Some("attached_blog_article_set".into())
        );
    }

    #[test]
    fn app_label_from_nested_module_path() {
        let rel = Relation {
            field_name: "user".into(),
            relation_type: RelationType::ForeignKey {
                target_model: ModelName::new("User"),
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
        // A bare "models" module path has no valid app label — %(app_label)s
        // should substitute as empty rather than producing "models".
        let rel = Relation {
            field_name: "user".into(),
            relation_type: RelationType::ForeignKey {
                target_model: ModelName::new("User"),
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
                target_model: ModelName::new("User"),
                related_name: Some("%(app_label)s_set".into()),
            },
        };
        assert_eq!(
            rel.effective_related_name("Order", "myapp"),
            Some("myapp_set".into())
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
        let mut model = ModelDef::new("TaggedItem", "tagging.models", 1);
        model.relations.push(Relation {
            field_name: "content_object".into(),
            relation_type: RelationType::GenericForeignKey {
                ct_field: "content_type".into(),
                fk_field: "object_id".into(),
            },
        });
        graph.add_model(model);

        assert_eq!(graph.resolve_forward("TaggedItem", "content_object"), None);
    }

    #[test]
    fn merge_graphs() {
        let mut g1 = ModelGraph::new();
        g1.add_model(ModelDef::new("User", "auth.models", 1));

        let mut g2 = ModelGraph::new();
        g2.add_model(ModelDef::new("Order", "shop.models", 1));

        g1.merge(g2);
        assert_eq!(g1.len(), 2);
        assert!(g1.get("User").is_some());
        assert!(g1.get("Order").is_some());
    }
}
