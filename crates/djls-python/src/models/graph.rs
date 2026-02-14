use std::collections::BTreeMap;

use serde::Deserialize;
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RelationType {
    ForeignKey,
    OneToOne,
    ManyToMany,
    GenericForeignKey,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Relation {
    pub field_name: String,
    pub target_model: String,
    pub relation_type: RelationType,
    pub related_name: Option<String>,
}

impl Relation {
    /// Resolve the effective `related_name` for reverse lookups.
    ///
    /// If an explicit `related_name` was provided, uses that (with `%(class)s`
    /// substitution applied). Otherwise synthesizes Django's default: `<model>_set`
    /// for FK/M2M/GenericFK, or `<model>` for `OneToOne`.
    #[must_use]
    pub fn effective_related_name(&self, source_model: &str) -> String {
        let lower = source_model.to_lowercase();
        match &self.related_name {
            Some(name) => name.replace("%(class)s", &lower),
            None => match self.relation_type {
                RelationType::OneToOne => lower,
                RelationType::ForeignKey
                | RelationType::ManyToMany
                | RelationType::GenericForeignKey => format!("{lower}_set"),
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelDef {
    pub name: String,
    pub module_path: String,
    pub line: usize,
    pub relations: Vec<Relation>,
    pub is_abstract: bool,
}

impl ModelDef {
    #[must_use]
    pub fn new(name: impl Into<String>, module_path: impl Into<String>, line: usize) -> Self {
        Self {
            name: name.into(),
            module_path: module_path.into(),
            line,
            relations: Vec::new(),
            is_abstract: false,
        }
    }
}

/// Dependency graph of Django models and their relations.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelGraph {
    models: BTreeMap<String, ModelDef>,
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
    #[must_use]
    pub fn resolve_forward(&self, model_name: &str, field_name: &str) -> Option<&str> {
        let model = self.models.get(model_name)?;
        model
            .relations
            .iter()
            .find(|r| r.field_name == field_name)
            .map(|r| r.target_model.as_str())
    }

    /// Look up models that point at `model_name` via a reverse relation.
    ///
    /// Returns `(source_model_name, effective_related_name, relation)` triples.
    pub fn resolve_reverse<'a>(
        &'a self,
        model_name: &'a str,
    ) -> impl Iterator<Item = (&'a str, String, &'a Relation)> {
        self.models.values().flat_map(move |m| {
            m.relations
                .iter()
                .filter(move |r| r.target_model == model_name)
                .map(move |r| (m.name.as_str(), r.effective_related_name(&m.name), r))
        })
    }

    /// Resolve a field access on a model — checks forward relations first,
    /// then reverse relations. Returns the resolved model name.
    #[must_use]
    pub fn resolve_relation(&self, model_name: &str, field_name: &str) -> Option<String> {
        // Forward
        if let Some(target) = self.resolve_forward(model_name, field_name) {
            return Some(target.to_string());
        }

        // Reverse
        for (source_name, related_name, _) in self.resolve_reverse(model_name) {
            if related_name == field_name {
                return Some(source_name.to_string());
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
            target_model: "User".into(),
            relation_type: RelationType::ForeignKey,
            related_name: Some("orders".into()),
        });

        let mut profile = ModelDef::new("Profile", "accounts.models", 1);
        profile.relations.push(Relation {
            field_name: "user".into(),
            target_model: "User".into(),
            relation_type: RelationType::OneToOne,
            related_name: None,
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
        assert_eq!(
            graph.resolve_relation("Order", "user"),
            Some("User".to_string())
        );
        // Reverse (explicit related_name)
        assert_eq!(
            graph.resolve_relation("User", "orders"),
            Some("Order".to_string())
        );
        // Reverse (default related_name for O2O)
        assert_eq!(
            graph.resolve_relation("User", "profile"),
            Some("Profile".to_string())
        );
    }

    #[test]
    fn default_related_name_fk() {
        let rel = Relation {
            field_name: "user".into(),
            target_model: "User".into(),
            relation_type: RelationType::ForeignKey,
            related_name: None,
        };
        assert_eq!(rel.effective_related_name("Order"), "order_set");
    }

    #[test]
    fn default_related_name_o2o() {
        let rel = Relation {
            field_name: "user".into(),
            target_model: "User".into(),
            relation_type: RelationType::OneToOne,
            related_name: None,
        };
        assert_eq!(rel.effective_related_name("Profile"), "profile");
    }

    #[test]
    fn class_substitution_in_related_name() {
        let rel = Relation {
            field_name: "user".into(),
            target_model: "User".into(),
            relation_type: RelationType::ForeignKey,
            related_name: Some("%(class)s_orders".into()),
        };
        assert_eq!(
            rel.effective_related_name("SpecialOrder"),
            "specialorder_orders"
        );
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
