use std::ops::ControlFlow;

use ruff_python_ast::Expr;
use ruff_python_ast::Stmt;
use ruff_python_ast::StmtClassDef;
use rustc_hash::FxHashSet;

use crate::ast::ExprExt;
use crate::ast::Recurse;
use crate::ast::walk_stmts;
use crate::models::graph::FieldName;
use crate::models::graph::ModelDef;
use crate::models::graph::ModelGraph;
use crate::models::graph::ModelKind;
use crate::models::graph::ModelName;
use crate::models::graph::Relation;
use crate::models::graph::RelationTarget;
use crate::models::graph::RelationType;
use crate::python::PythonModuleName;

pub(super) struct ModelCollector<'a> {
    module_name: PythonModuleName,
    source: &'a str,
    aliases: ImportAliases,
    graph: ModelGraph,
    children: Vec<&'a StmtClassDef>,
}

impl<'a> ModelCollector<'a> {
    pub(super) fn new(module_name: PythonModuleName, source: &'a str) -> Self {
        Self {
            module_name,
            source,
            aliases: ImportAliases::new(),
            graph: ModelGraph::new(),
            children: Vec::new(),
        }
    }

    pub(super) fn finish(mut self) -> ModelGraph {
        resolve_children(
            &mut self.graph,
            &self.children,
            &self.module_name,
            self.source,
        );
        self.graph
    }

    pub(super) fn scan_stmt(&mut self, stmt: &'a Stmt) {
        match stmt {
            Stmt::ImportFrom(import) => {
                let Some(module) = import
                    .module
                    .as_ref()
                    .map(ruff_python_ast::Identifier::as_str)
                else {
                    return;
                };

                if is_django_model_parent(module) {
                    for name in &import.names {
                        if name.name.as_str() == "models" {
                            let alias = name.asname.as_ref().map_or("models", |a| a.as_str());
                            self.aliases.module_aliases.insert(alias.to_string());
                        }
                    }
                }

                if is_django_models_module(module) {
                    for name in &import.names {
                        if name.name.as_str() == "Model" {
                            let alias = name.asname.as_ref().map_or("Model", |a| a.as_str());
                            self.aliases.class_aliases.insert(alias.to_string());
                        }
                    }
                }
            }
            Stmt::Import(import) => {
                for name in &import.names {
                    if is_django_models_module(name.name.as_str())
                        && let Some(alias) = &name.asname
                    {
                        self.aliases.module_aliases.insert(alias.to_string());
                    }
                }
            }
            Stmt::ClassDef(class) => {
                let Some(ref args) = class.arguments else {
                    return;
                };

                if is_django_model(args.args.iter(), &self.aliases) {
                    let line = line_number(self.source, class.range.start().to_usize());
                    let mut model =
                        ModelDef::new(class.name.to_string(), self.module_name.clone(), line);

                    walk_stmts(&class.body, Recurse::Flat, |stmt| {
                        process_class_body(stmt, &mut model);
                        ControlFlow::Continue(())
                    });

                    self.graph.add_model(model);
                } else if !args.args.is_empty() {
                    self.children.push(class);
                }
            }
            _ => {}
        }
    }
}

fn resolve_children(
    graph: &mut ModelGraph,
    children: &[&StmtClassDef],
    module_name: &PythonModuleName,
    source: &str,
) {
    let mut remaining: Vec<&StmtClassDef> = children.to_vec();

    // Fixed-point loop: each iteration may resolve new models, which in turn
    // unblock children that inherit from them (e.g., User -> AbstractUser ->
    // PermissionsMixin). Converges when no progress is made.
    loop {
        let before = remaining.len();
        let mut unresolved = Vec::new();

        // Snapshot model state at the start of each iteration so newly resolved
        // models become visible to the next iteration.
        let abstract_data: Vec<(ModelName, Vec<Relation>)> = graph
            .models()
            .filter(|m| m.kind == ModelKind::Abstract)
            .map(|m| (m.name.clone(), m.relations.clone()))
            .collect();
        let known_names: Vec<ModelName> = graph.models().map(|m| m.name.clone()).collect();

        for class in &remaining {
            let Some(ref args) = class.arguments else {
                continue;
            };

            let has_model_parent = args.args.iter().any(|arg| {
                let Some(name) = base_class_name(arg) else {
                    return false;
                };
                known_names.iter().any(|m| m.as_str() == name)
            });

            if !has_model_parent {
                unresolved.push(*class);
                continue;
            }

            let line = line_number(source, class.range.start().to_usize());
            let mut model = ModelDef::new(class.name.to_string(), module_name.clone(), line);

            // Copy relations from ALL abstract parents
            for arg in &args.args {
                let Some(parent_name) = base_class_name(arg) else {
                    continue;
                };
                if let Some((_, relations)) = abstract_data
                    .iter()
                    .find(|(name, _)| name.as_str() == parent_name)
                {
                    model.relations.extend(relations.iter().cloned());
                }
            }

            walk_stmts(&class.body, Recurse::Flat, |stmt| {
                process_class_body(stmt, &mut model);
                ControlFlow::Continue(())
            });

            graph.add_model(model);
        }

        remaining = unresolved;
        if remaining.len() == before {
            break;
        }
    }
}

/// Extract the simple class name from a base class expression.
///
/// Handles both bare names (`Parent`) and qualified names (`mod.Parent`),
/// returning the rightmost identifier.
fn base_class_name(expr: &Expr) -> Option<&str> {
    if let Some(name) = expr.name_target() {
        return Some(name);
    }

    match expr {
        Expr::Attribute(attr) => Some(attr.attr.as_str()),
        _ => None,
    }
}

/// Check if a module name is a known Django models parent module.
///
/// These are the modules from which `models` can be imported
/// (e.g., `from django.db import models`).
fn is_django_model_parent(module: &str) -> bool {
    matches!(module, "django.db" | "django.contrib.gis.db")
}

/// Check if a module name is a known Django `models` module.
///
/// These are the fully-qualified paths to Django's model modules
/// (for `import django.db.models as ...` or `from django.db.models import Model`).
fn is_django_models_module(module: &str) -> bool {
    matches!(module, "django.db.models" | "django.contrib.gis.db.models")
}

/// Import aliases discovered from a module's import statements.
///
/// Tracks two kinds of aliases:
/// - **module aliases**: names that refer to `django.db.models` (or the
///   `GeoDjango` equivalent), used to match the `x.Model` pattern.
///   Recognized imports:
///   - `from django.db import models [as m]`
///   - `from django.contrib.gis.db import models [as m]`
///   - `import django.db.models as m`
///   - `import django.contrib.gis.db.models as m`
/// - **class aliases**: names that refer to the `Model` class directly,
///   used to match bare-name patterns like `class Foo(M):`.
///   Recognized imports:
///   - `from django.db.models import Model [as M]`
///   - `from django.contrib.gis.db.models import Model [as M]`
struct ImportAliases {
    /// Names aliasing `django.db.models` (always includes `"models"`).
    module_aliases: FxHashSet<String>,
    /// Names aliasing the `Model` class (always includes `"Model"`).
    class_aliases: FxHashSet<String>,
}

impl ImportAliases {
    fn new() -> Self {
        Self {
            module_aliases: FxHashSet::from_iter(["models".to_string()]),
            class_aliases: FxHashSet::from_iter(["Model".to_string()]),
        }
    }
}

fn is_django_model<'a>(bases: impl Iterator<Item = &'a Expr>, aliases: &ImportAliases) -> bool {
    for base in bases {
        // Model / M (where M aliases django.db.models.Model)
        if base
            .name_target()
            .is_some_and(|name| aliases.class_aliases.contains(name))
        {
            return true;
        }

        // models.Model / m.Model (where m aliases django.db.models)
        if let Expr::Attribute(attr) = base
            && attr.attr.as_str() == "Model"
            && attr
                .value
                .name_target()
                .is_some_and(|name| aliases.module_aliases.contains(name))
        {
            return true;
        }
    }
    false
}

fn process_class_body(stmt: &Stmt, model: &mut ModelDef) {
    // Check for Meta.abstract
    if let Stmt::ClassDef(meta) = stmt
        && meta.name.as_str() == "Meta"
    {
        for meta_stmt in &meta.body {
            if is_abstract_assignment(meta_stmt) {
                model.kind = ModelKind::Abstract;
                return;
            }
        }
    }

    // Extract relation fields (FK, O2O, M2M)
    if let Some(relation) = extract_relation(stmt) {
        model.relations.push(relation);
        return;
    }

    // Extract GenericForeignKey fields
    if let Some(gfk) = extract_generic_foreign_key(stmt) {
        model.relations.push(gfk);
    }
}

fn is_abstract_assignment(stmt: &Stmt) -> bool {
    let Stmt::Assign(assign) = stmt else {
        return false;
    };
    let Some(target) = assign.targets.first() else {
        return false;
    };
    if target.name_target() != Some("abstract") {
        return false;
    }
    matches!(assign.value.as_ref(), Expr::BooleanLiteral(b) if b.value)
}

fn extract_relation(stmt: &Stmt) -> Option<Relation> {
    let Stmt::Assign(assign) = stmt else {
        return None;
    };

    let target_name = assign.targets.first()?.name_target()?;

    let Expr::Call(call) = assign.value.as_ref() else {
        return None;
    };

    let field_class_name = match call.func.as_ref() {
        Expr::Attribute(attr) => attr.attr.as_str(),
        expr => expr.name_target()?,
    };

    let target = extract_target_model(call)?;
    let related_name = extract_related_name(call);

    let relation_type = RelationType::from_field_class(field_class_name, target, related_name)?;

    Some(Relation {
        field_name: FieldName::new(target_name),
        relation_type,
    })
}

fn extract_target_model(call: &ruff_python_ast::ExprCall) -> Option<RelationTarget> {
    let first_arg = call.arguments.args.first()?;

    // Direct reference: ForeignKey(User)
    if let Some(name) = first_arg.name_target() {
        return Some(RelationTarget::Bare {
            name: ModelName::new(name),
        });
    }

    match first_arg {
        // String reference: ForeignKey("self"), ForeignKey("User"), or ForeignKey("app.User")
        Expr::StringLiteral(s) => {
            let value = s.value.to_string();
            Some(relation_target_from_string(&value))
        }
        // Attribute: ForeignKey(auth.User). Keep today's behavior and drop the qualifier.
        Expr::Attribute(attr) => Some(RelationTarget::Bare {
            name: ModelName::new(attr.attr.as_str()),
        }),
        _ => None,
    }
}

fn relation_target_from_string(value: &str) -> RelationTarget {
    if value == "self" {
        return RelationTarget::SelfRef;
    }

    if let Some((app_label, name)) = value.rsplit_once('.') {
        return RelationTarget::Qualified {
            app_label: app_label.to_string(),
            name: ModelName::new(name),
        };
    }

    RelationTarget::Bare {
        name: ModelName::new(value),
    }
}

fn extract_related_name(call: &ruff_python_ast::ExprCall) -> Option<String> {
    call.arguments
        .keywords
        .iter()
        .find(|kw| {
            kw.arg
                .as_ref()
                .is_some_and(|a| a.as_str() == "related_name")
        })
        .and_then(|kw| match &kw.value {
            Expr::StringLiteral(s) => Some(s.value.to_string()),
            _ => None,
        })
}

fn extract_generic_foreign_key(stmt: &Stmt) -> Option<Relation> {
    let Stmt::Assign(assign) = stmt else {
        return None;
    };

    let target_name = assign.targets.first()?.name_target()?;

    let Expr::Call(call) = assign.value.as_ref() else {
        return None;
    };

    let is_gfk = match call.func.as_ref() {
        Expr::Attribute(attr) => attr.attr.as_str() == "GenericForeignKey",
        expr => expr.name_target() == Some("GenericForeignKey"),
    };

    if !is_gfk {
        return None;
    }

    let ct_field =
        extract_gfk_arg(call, 0, "ct_field").unwrap_or_else(|| "content_type".to_string());
    let fk_field = extract_gfk_arg(call, 1, "fk_field").unwrap_or_else(|| "object_id".to_string());

    Some(Relation {
        field_name: FieldName::new(target_name),
        relation_type: RelationType::GenericForeignKey {
            ct_field: FieldName::new(ct_field),
            fk_field: FieldName::new(fk_field),
        },
    })
}

/// Extract a string argument from a GFK constructor call by positional index
/// or keyword name.
fn extract_gfk_arg(call: &ruff_python_ast::ExprCall, pos: usize, keyword: &str) -> Option<String> {
    // Try keyword first
    if let Some(value) = call
        .arguments
        .keywords
        .iter()
        .find(|kw| kw.arg.as_ref().is_some_and(|a| a.as_str() == keyword))
        .and_then(|kw| match &kw.value {
            Expr::StringLiteral(s) => Some(s.value.to_string()),
            _ => None,
        })
    {
        return Some(value);
    }

    // Fall back to positional
    call.arguments.args.get(pos).and_then(|arg| match arg {
        Expr::StringLiteral(s) => Some(s.value.to_string()),
        _ => None,
    })
}

fn line_number(source: &str, offset: usize) -> usize {
    let offset = offset.min(source.len());
    source[..offset].bytes().filter(|&b| b == b'\n').count() + 1
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::graph::ModelId;

    fn extract_model_graph(source: &str, module_name: &str) -> ModelGraph {
        super::super::extract_model_graph_impl(
            source,
            PythonModuleName::parse(module_name).unwrap(),
        )
    }

    fn model<'a>(graph: &'a ModelGraph, name: &'a str) -> &'a ModelDef {
        graph
            .models_named(name)
            .next()
            .map(|(_id, model)| model)
            .expect("model should exist")
    }

    fn model_id<'a>(graph: &'a ModelGraph, name: &'a str) -> &'a ModelId {
        graph
            .models_named(name)
            .next()
            .map(|(id, _model)| id)
            .expect("model should exist")
    }

    fn has_model(graph: &ModelGraph, name: &str) -> bool {
        graph.models_named(name).next().is_some()
    }

    fn bare_target_name(relation: &Relation) -> Option<&str> {
        match relation.target_model()? {
            RelationTarget::Bare { name } => Some(name.as_str()),
            RelationTarget::SelfRef | RelationTarget::Qualified { .. } => None,
        }
    }

    fn resolved_model_name(model: Option<&ModelDef>) -> Option<&str> {
        model.map(|model| model.name.as_str())
    }

    #[test]
    fn empty_source() {
        let graph = extract_model_graph("", "test");
        assert!(graph.is_empty());
    }

    #[test]
    fn parse_error_returns_empty() {
        let graph = extract_model_graph("def def def", "test");
        assert!(graph.is_empty());
    }

    #[test]
    fn plain_class_ignored() {
        let graph = extract_model_graph("class Foo:\n    pass\n", "test");
        assert!(graph.is_empty());
    }

    #[test]
    fn simple_model() {
        let source = r"
class User(models.Model):
    name = models.CharField(max_length=100)
";
        let graph = extract_model_graph(source, "auth.models");
        assert_eq!(graph.len(), 1);

        let user = model(&graph, "User");
        assert_eq!(user.module_name.as_str(), "auth.models");
        assert_eq!(user.line, 2);
        assert!(user.relations.is_empty());
        assert_eq!(user.kind, ModelKind::Concrete);
    }

    #[test]
    fn direct_model_import() {
        let source = r"
from django.db.models import Model

class User(Model):
    name = models.CharField(max_length=100)
";
        let graph = extract_model_graph(source, "auth.models");
        assert_eq!(graph.len(), 1);
        assert!(has_model(&graph, "User"));
    }

    #[test]
    fn aliased_models_import() {
        let source = r"
from django.db import models as m

class User(m.Model):
    name = m.CharField(max_length=100)
";
        let graph = extract_model_graph(source, "auth.models");
        assert_eq!(graph.len(), 1);
        assert!(has_model(&graph, "User"));
    }

    #[test]
    fn aliased_absolute_import() {
        let source = r"
import django.db.models as db_models

class User(db_models.Model):
    pass
";
        let graph = extract_model_graph(source, "auth.models");
        assert_eq!(graph.len(), 1);
        assert!(has_model(&graph, "User"));
    }

    #[test]
    fn aliased_model_class_import() {
        let source = r"
from django.db.models import Model as BaseModel

class User(BaseModel):
    pass
";
        let graph = extract_model_graph(source, "auth.models");
        assert_eq!(graph.len(), 1);
        assert!(has_model(&graph, "User"));
    }

    #[test]
    fn geodjango_models_import() {
        let source = r"
from django.contrib.gis.db import models

class Location(models.Model):
    pass
";
        let graph = extract_model_graph(source, "geo.models");
        assert_eq!(graph.len(), 1);
        assert!(has_model(&graph, "Location"));
    }

    #[test]
    fn geodjango_aliased_import() {
        let source = r"
from django.contrib.gis.db import models as gis

class Location(gis.Model):
    pass
";
        let graph = extract_model_graph(source, "geo.models");
        assert_eq!(graph.len(), 1);
        assert!(has_model(&graph, "Location"));
    }

    #[test]
    fn geodjango_model_class_import() {
        let source = r"
from django.contrib.gis.db.models import Model as GeoModel

class Location(GeoModel):
    pass
";
        let graph = extract_model_graph(source, "geo.models");
        assert_eq!(graph.len(), 1);
        assert!(has_model(&graph, "Location"));
    }

    #[test]
    fn unrelated_alias_not_matched() {
        // foo.Model should NOT be detected as a Django model
        let source = r"
import foo

class NotAModel(foo.Model):
    pass
";
        let graph = extract_model_graph(source, "app.models");
        assert!(graph.is_empty());
    }

    #[test]
    fn unrelated_model_name_not_matched() {
        // A bare name that happens to not be "Model" should not match
        let source = r"
from pydantic import BaseModel

class NotDjango(BaseModel):
    pass
";
        let graph = extract_model_graph(source, "app.models");
        assert!(graph.is_empty());
    }

    #[test]
    fn foreign_key() {
        let source = r"
class User(models.Model):
    pass

class Order(models.Model):
    user = models.ForeignKey(User, on_delete=models.CASCADE)
";
        let graph = extract_model_graph(source, "shop.models");

        let order = model(&graph, "Order");
        assert_eq!(order.relations.len(), 1);

        let rel = &order.relations[0];
        assert_eq!(rel.field_name.as_str(), "user");
        assert_eq!(bare_target_name(rel), Some("User"));
        assert!(matches!(
            rel.relation_type,
            RelationType::ForeignKey { ref related_name, .. } if related_name.is_none()
        ));
    }

    #[test]
    fn explicit_related_name() {
        let source = r#"
class Order(models.Model):
    user = models.ForeignKey(User, on_delete=models.CASCADE, related_name="orders")
"#;
        let graph = extract_model_graph(source, "shop.models");

        let rel = &model(&graph, "Order").relations[0];
        assert!(matches!(
            rel.relation_type,
            RelationType::ForeignKey { ref related_name, .. } if related_name.as_deref() == Some("orders")
        ));
        assert_eq!(
            rel.effective_related_name("Order", "shop.models"),
            Some("orders".into())
        );
    }

    #[test]
    fn string_ref_with_app_label_preserves_qualified_target() {
        let source = r#"
class Order(models.Model):
    user = models.ForeignKey("accounts.User", on_delete=models.CASCADE)
"#;
        let graph = extract_model_graph(source, "shop.models");
        assert!(matches!(
            model(&graph, "Order").relations[0].target_model(),
            Some(RelationTarget::Qualified { app_label, name })
                if app_label == "accounts" && name.as_str() == "User"
        ));
    }

    #[test]
    fn string_ref_self_preserves_self_target() {
        let source = r#"
class Category(models.Model):
    parent = models.ForeignKey("self", on_delete=models.CASCADE)
"#;
        let graph = extract_model_graph(source, "catalog.models");
        assert!(matches!(
            model(&graph, "Category").relations[0].target_model(),
            Some(RelationTarget::SelfRef)
        ));
    }

    #[test]
    fn all_relation_types() {
        let source = r"
class Profile(models.Model):
    user = models.OneToOneField(User, on_delete=models.CASCADE)

class Article(models.Model):
    author = models.ForeignKey(User, on_delete=models.CASCADE)
    tags = models.ManyToManyField(Tag)
";
        let graph = extract_model_graph(source, "app.models");

        let profile = model(&graph, "Profile");
        assert!(matches!(
            profile.relations[0].relation_type,
            RelationType::OneToOne { .. }
        ));

        let article = model(&graph, "Article");
        assert_eq!(article.relations.len(), 2);
        assert!(matches!(
            article.relations[0].relation_type,
            RelationType::ForeignKey { .. }
        ));
        assert!(matches!(
            article.relations[1].relation_type,
            RelationType::ManyToMany { .. }
        ));
    }

    #[test]
    fn abstract_model() {
        let source = r"
class BaseModel(models.Model):
    class Meta:
        abstract = True
";
        let graph = extract_model_graph(source, "app.models");
        assert_eq!(model(&graph, "BaseModel").kind, ModelKind::Abstract);
    }

    #[test]
    fn abstract_inheritance() {
        let source = r"
class User(models.Model):
    pass

class Seller(models.Model):
    pass

class BaseOrder(models.Model):
    user = models.ForeignKey(User, on_delete=models.CASCADE)

    class Meta:
        abstract = True

class ConcreteOrder(BaseOrder):
    seller = models.ForeignKey(Seller, on_delete=models.CASCADE)
";
        let graph = extract_model_graph(source, "shop.models");

        let concrete = model(&graph, "ConcreteOrder");
        assert_eq!(concrete.kind, ModelKind::Concrete);
        assert_eq!(concrete.relations.len(), 2);

        let targets: Vec<&str> = concrete
            .relations
            .iter()
            .filter_map(bare_target_name)
            .collect();
        assert!(targets.contains(&"User"));
        assert!(targets.contains(&"Seller"));
    }

    #[test]
    fn class_substitution_in_inherited_related_name() {
        let source = r#"
class User(models.Model):
    pass

class BaseOrder(models.Model):
    user = models.ForeignKey(User, related_name="%(class)s_set")

    class Meta:
        abstract = True

class SpecialOrder(BaseOrder):
    pass
"#;
        let graph = extract_model_graph(source, "shop.models");

        let special = model(&graph, "SpecialOrder");
        let rel = &special.relations[0];
        assert_eq!(
            rel.effective_related_name("SpecialOrder", "shop.models"),
            Some("specialorder_set".into())
        );
    }

    #[test]
    fn forward_and_reverse_lookups() {
        let source = r#"
class User(models.Model):
    pass

class Order(models.Model):
    user = models.ForeignKey(User, on_delete=models.CASCADE, related_name="orders")
"#;
        let graph = extract_model_graph(source, "shop.models");

        // Forward
        assert_eq!(
            resolved_model_name(graph.resolve_forward(model_id(&graph, "Order"), "user")),
            Some("User")
        );

        // Reverse
        assert_eq!(
            resolved_model_name(graph.resolve_relation(model_id(&graph, "User"), "orders")),
            Some("Order")
        );

        // Non-existent
        assert_eq!(
            resolved_model_name(graph.resolve_relation(model_id(&graph, "User"), "nope")),
            None
        );
    }

    #[test]
    fn default_reverse_name() {
        let source = r"
class User(models.Model):
    pass

class Order(models.Model):
    user = models.ForeignKey(User, on_delete=models.CASCADE)
";
        let graph = extract_model_graph(source, "shop.models");

        // Default FK reverse name is <model>_set
        assert_eq!(
            resolved_model_name(graph.resolve_relation(model_id(&graph, "User"), "order_set")),
            Some("Order")
        );
    }

    #[test]
    fn multiple_models_multiple_relations() {
        let source = r#"
class User(models.Model):
    pass

class Tag(models.Model):
    pass

class Post(models.Model):
    author = models.ForeignKey(User, on_delete=models.CASCADE, related_name="posts")
    tags = models.ManyToManyField(Tag, related_name="posts")

class Comment(models.Model):
    post = models.ForeignKey(Post, on_delete=models.CASCADE, related_name="comments")
    author = models.ForeignKey(User, on_delete=models.CASCADE, related_name="comments")
"#;
        let graph = extract_model_graph(source, "blog.models");
        assert_eq!(graph.len(), 4);

        // Chain: User -> posts -> comments
        assert_eq!(
            resolved_model_name(graph.resolve_relation(model_id(&graph, "User"), "posts")),
            Some("Post")
        );
        assert_eq!(
            resolved_model_name(graph.resolve_relation(model_id(&graph, "Post"), "comments")),
            Some("Comment")
        );
        assert_eq!(
            resolved_model_name(graph.resolve_relation(model_id(&graph, "Comment"), "author")),
            Some("User")
        );
    }

    #[test]
    fn multiple_abstract_parents() {
        let source = r"
class User(models.Model):
    pass

class Approver(models.Model):
    pass

class TimestampMixin(models.Model):
    created_by = models.ForeignKey(User, on_delete=models.CASCADE)

    class Meta:
        abstract = True

class AuditMixin(models.Model):
    approved_by = models.ForeignKey(Approver, on_delete=models.CASCADE)

    class Meta:
        abstract = True

class Document(TimestampMixin, AuditMixin):
    pass
";
        let graph = extract_model_graph(source, "app.models");

        let doc = model(&graph, "Document");
        assert_eq!(doc.relations.len(), 2);

        let targets: Vec<&str> = doc.relations.iter().filter_map(bare_target_name).collect();
        assert!(targets.contains(&"User"));
        assert!(targets.contains(&"Approver"));
    }

    #[test]
    fn concrete_model_inheritance() {
        let source = r"
class User(models.Model):
    pass

class Place(models.Model):
    name = models.CharField(max_length=50)

class Restaurant(Place):
    owner = models.ForeignKey(User, on_delete=models.CASCADE)
";
        let graph = extract_model_graph(source, "app.models");

        let restaurant = model(&graph, "Restaurant");
        assert_eq!(restaurant.relations.len(), 1);
        assert_eq!(restaurant.relations[0].field_name.as_str(), "owner");
        assert_eq!(bare_target_name(&restaurant.relations[0]), Some("User"));
    }

    #[test]
    fn qualified_base_class_inheritance() {
        let source = r"
class User(models.Model):
    pass

class BaseOrder(models.Model):
    user = models.ForeignKey(User, on_delete=models.CASCADE)

    class Meta:
        abstract = True

class ConcreteOrder(some_module.BaseOrder):
    pass
";
        let graph = extract_model_graph(source, "shop.models");

        let concrete = model(&graph, "ConcreteOrder");
        assert_eq!(concrete.relations.len(), 1);
        assert_eq!(bare_target_name(&concrete.relations[0]), Some("User"));
    }

    #[test]
    fn multi_level_inheritance_chain() {
        let source = r"
class User(models.Model):
    pass

class BaseMixin(models.Model):
    user = models.ForeignKey(User, on_delete=models.CASCADE)

    class Meta:
        abstract = True

class MiddleMixin(BaseMixin):
    class Meta:
        abstract = True

class Concrete(MiddleMixin):
    pass
";
        let graph = extract_model_graph(source, "app.models");

        // MiddleMixin inherits BaseMixin's FK to User
        let middle = model(&graph, "MiddleMixin");
        assert_eq!(middle.kind, ModelKind::Abstract);
        assert_eq!(middle.relations.len(), 1);
        assert_eq!(bare_target_name(&middle.relations[0]), Some("User"));

        // Concrete inherits through MiddleMixin
        let concrete = model(&graph, "Concrete");
        assert_eq!(concrete.kind, ModelKind::Concrete);
        assert_eq!(concrete.relations.len(), 1);
        assert_eq!(bare_target_name(&concrete.relations[0]), Some("User"));
    }

    #[test]
    fn generic_foreign_key_extracted() {
        let source = r#"
class TaggedItem(models.Model):
    content_type = models.ForeignKey("ContentType", on_delete=models.CASCADE)
    object_id = models.PositiveIntegerField()
    content_object = GenericForeignKey("content_type", "object_id")
"#;
        let graph = extract_model_graph(source, "tagging.models");

        let tagged = model(&graph, "TaggedItem");
        // Both FK and GFK are in the same relations list
        assert_eq!(tagged.relations.len(), 2);

        // First relation: FK to ContentType
        assert_eq!(tagged.relations[0].field_name.as_str(), "content_type");
        assert!(matches!(
            tagged.relations[0].relation_type,
            RelationType::ForeignKey { .. }
        ));

        // Second relation: GFK
        assert_eq!(tagged.relations[1].field_name.as_str(), "content_object");
        assert!(matches!(
            tagged.relations[1].relation_type,
            RelationType::GenericForeignKey {
                ref ct_field,
                ref fk_field,
            } if ct_field.as_str() == "content_type" && fk_field.as_str() == "object_id"
        ));
    }

    #[test]
    fn generic_foreign_key_defaults() {
        let source = r"
class TaggedItem(models.Model):
    content_object = GenericForeignKey()
";
        let graph = extract_model_graph(source, "tagging.models");

        let rel = &model(&graph, "TaggedItem").relations[0];
        assert_eq!(rel.field_name.as_str(), "content_object");
        assert!(matches!(
            rel.relation_type,
            RelationType::GenericForeignKey {
                ref ct_field,
                ref fk_field,
            } if ct_field.as_str() == "content_type" && fk_field.as_str() == "object_id"
        ));
    }

    #[test]
    fn generic_foreign_key_keyword_args() {
        let source = r"
class ObjectLog(models.Model):
    parent = GenericForeignKey(ct_field='object_type', fk_field='object_id')
";
        let graph = extract_model_graph(source, "logs.models");

        let rel = &model(&graph, "ObjectLog").relations[0];
        assert_eq!(rel.field_name.as_str(), "parent");
        assert!(matches!(
            rel.relation_type,
            RelationType::GenericForeignKey {
                ref ct_field,
                ref fk_field,
            } if ct_field.as_str() == "object_type" && fk_field.as_str() == "object_id"
        ));
    }

    #[test]
    fn generic_foreign_key_module_prefix() {
        let source = r#"
from django.contrib.contenttypes import fields

class TaggedItem(models.Model):
    content_object = fields.GenericForeignKey("content_type", "object_id")
"#;
        let graph = extract_model_graph(source, "tagging.models");

        assert_eq!(model(&graph, "TaggedItem").relations.len(), 1);
        assert!(matches!(
            model(&graph, "TaggedItem").relations[0].relation_type,
            RelationType::GenericForeignKey { .. }
        ));
    }

    #[test]
    fn generic_foreign_key_inherited_from_abstract() {
        let source = r#"
class GenericMixin(models.Model):
    content_object = GenericForeignKey("content_type", "object_id")

    class Meta:
        abstract = True

class TaggedItem(GenericMixin):
    pass
"#;
        let graph = extract_model_graph(source, "tagging.models");

        let tagged = model(&graph, "TaggedItem");
        assert_eq!(tagged.relations.len(), 1);
        assert_eq!(tagged.relations[0].field_name.as_str(), "content_object");
        assert!(matches!(
            tagged.relations[0].relation_type,
            RelationType::GenericForeignKey { .. }
        ));
    }

    #[test]
    fn multiple_generic_foreign_keys() {
        let source = r"
class Action(models.Model):
    actor = GenericForeignKey('actor_content_type', 'actor_object_id')
    target = GenericForeignKey('target_content_type', 'target_object_id')
";
        let graph = extract_model_graph(source, "activity.models");

        let action = model(&graph, "Action");
        assert_eq!(action.relations.len(), 2);
        assert_eq!(action.relations[0].field_name.as_str(), "actor");
        assert!(matches!(
            action.relations[0].relation_type,
            RelationType::GenericForeignKey {
                ref ct_field,
                ..
            } if ct_field.as_str() == "actor_content_type"
        ));
        assert_eq!(action.relations[1].field_name.as_str(), "target");
        assert!(matches!(
            action.relations[1].relation_type,
            RelationType::GenericForeignKey {
                ref ct_field,
                ..
            } if ct_field.as_str() == "target_content_type"
        ));
    }
}
