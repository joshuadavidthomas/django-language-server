use ruff_python_ast::Expr;
use ruff_python_ast::Stmt;
use ruff_python_ast::StmtClassDef;
use rustc_hash::FxHashSet;

use super::graph::FieldName;
use super::graph::GenericForeignKey;
use super::graph::ModelDef;
use super::graph::ModelGraph;
use super::graph::ModelKind;
use super::graph::ModelName;
use super::graph::Relation;
use super::graph::RelationType;

const RELATION_FIELDS: &[(&str, RelationType)] = &[
    ("ForeignKey", RelationType::ForeignKey),
    ("OneToOneField", RelationType::OneToOne),
    ("ManyToManyField", RelationType::ManyToMany),
];

/// Extract a model graph from Python source text.
///
/// Parses the source with Ruff's Python parser, walks the AST to find
/// `class Foo(models.Model):` definitions, extracts field declarations
/// and relation metadata, and builds a graph of models and their
/// relationships.
///
/// The `module_path` parameter is the dotted Python module path (e.g.,
/// `"myapp.models"`) recorded on each extracted `ModelDef`.
#[must_use]
pub fn extract_model_graph(source: &str, module_path: &str) -> ModelGraph {
    let Ok(parsed) = ruff_python_parser::parse_module(source) else {
        return ModelGraph::default();
    };
    let module = parsed.into_syntax();

    let aliases = collect_import_aliases(&module.body);
    extract_models_from_body(&module.body, module_path, source, &aliases)
}

fn extract_models_from_body(
    body: &[Stmt],
    module_path: &str,
    source: &str,
    aliases: &ImportAliases,
) -> ModelGraph {
    let mut graph = ModelGraph::new();
    let mut children: Vec<&StmtClassDef> = Vec::new();

    // First pass: extract direct model subclasses
    for stmt in body {
        let Stmt::ClassDef(class) = stmt else {
            continue;
        };

        let Some(ref args) = class.arguments else {
            continue;
        };

        if is_django_model(args.args.iter(), aliases) {
            let line = line_number(source, class.range.start().to_usize());
            let mut model = ModelDef::new(class.name.to_string(), module_path, line);

            for body_stmt in &class.body {
                process_class_body(body_stmt, &mut model);
            }

            graph.add_model(model);
        } else if !args.args.is_empty() {
            children.push(class);
        }
    }

    // Second pass: resolve children of abstract models
    resolve_children(&mut graph, &children, module_path, source);

    graph
}

fn resolve_children(
    graph: &mut ModelGraph,
    children: &[&StmtClassDef],
    module_path: &str,
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
        let abstract_data: Vec<(ModelName, Vec<Relation>, Vec<GenericForeignKey>)> = graph
            .models()
            .filter(|m| m.kind == ModelKind::Abstract)
            .map(|m| {
                (
                    m.name.clone(),
                    m.relations.clone(),
                    m.generic_foreign_keys.clone(),
                )
            })
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
            let mut model = ModelDef::new(class.name.to_string(), module_path, line);

            // Copy relations and GFKs from ALL abstract parents
            for arg in &args.args {
                let Some(parent_name) = base_class_name(arg) else {
                    continue;
                };
                if let Some((_, relations, gfks)) = abstract_data
                    .iter()
                    .find(|(name, _, _)| name.as_str() == parent_name)
                {
                    model.relations.extend(relations.iter().cloned());
                    model.generic_foreign_keys.extend(gfks.iter().cloned());
                }
            }

            for body_stmt in &class.body {
                process_class_body(body_stmt, &mut model);
            }

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
    match expr {
        Expr::Name(n) => Some(n.id.as_str()),
        Expr::Attribute(attr) => Some(attr.attr.as_str()),
        _ => None,
    }
}

/// Modules that parent `models` (for `from <parent> import models`).
const DJANGO_MODEL_PARENTS: &[&str] = &["django.db", "django.contrib.gis.db"];

/// Fully-qualified `models` modules (for `import <module> as ...`).
const DJANGO_MODELS_MODULES: &[&str] = &["django.db.models", "django.contrib.gis.db.models"];

/// Import aliases discovered from a module's import statements.
///
/// Tracks two kinds of aliases:
/// - **module aliases**: names that refer to `django.db.models` (or the
///   `GeoDjango` equivalent), used to match the `x.Model` pattern.
/// - **class aliases**: names that refer to the `Model` class directly,
///   used to match bare-name patterns like `class Foo(M):`.
struct ImportAliases {
    /// Names aliasing `django.db.models` (always includes `"models"`).
    module_aliases: FxHashSet<String>,
    /// Names aliasing the `Model` class (always includes `"Model"`).
    class_aliases: FxHashSet<String>,
}

/// Scan import statements for names that alias Django's `models` module
/// or the `Model` class.
///
/// Module aliases (for `x.Model` pattern):
/// - `from django.db import models [as m]`
/// - `from django.contrib.gis.db import models [as m]`
/// - `import django.db.models as m`
/// - `import django.contrib.gis.db.models as m`
///
/// Class aliases (for bare `M` pattern):
/// - `from django.db.models import Model [as M]`
/// - `from django.contrib.gis.db.models import Model [as M]`
fn collect_import_aliases(body: &[Stmt]) -> ImportAliases {
    let mut module_aliases = FxHashSet::default();
    module_aliases.insert("models".to_string());

    let mut class_aliases = FxHashSet::default();
    class_aliases.insert("Model".to_string());

    for stmt in body {
        match stmt {
            Stmt::ImportFrom(import) => {
                let Some(module) = import
                    .module
                    .as_ref()
                    .map(ruff_python_ast::Identifier::as_str)
                else {
                    continue;
                };

                // `from django.db import models [as m]`
                if DJANGO_MODEL_PARENTS.contains(&module) {
                    for name in &import.names {
                        if name.name.as_str() == "models" {
                            let alias = name.asname.as_ref().map_or("models", |a| a.as_str());
                            module_aliases.insert(alias.to_string());
                        }
                    }
                }

                // `from django.db.models import Model [as M]`
                if DJANGO_MODELS_MODULES.contains(&module) {
                    for name in &import.names {
                        if name.name.as_str() == "Model" {
                            let alias = name.asname.as_ref().map_or("Model", |a| a.as_str());
                            class_aliases.insert(alias.to_string());
                        }
                    }
                }
            }
            // `import django.db.models as m`
            Stmt::Import(import) => {
                for name in &import.names {
                    if DJANGO_MODELS_MODULES.contains(&name.name.as_str()) {
                        if let Some(alias) = &name.asname {
                            module_aliases.insert(alias.to_string());
                        }
                    }
                }
            }
            _ => {}
        }
    }

    ImportAliases {
        module_aliases,
        class_aliases,
    }
}

fn is_django_model<'a>(bases: impl Iterator<Item = &'a Expr>, aliases: &ImportAliases) -> bool {
    for base in bases {
        match base {
            // models.Model / m.Model (where m aliases django.db.models)
            Expr::Attribute(attr) => {
                if attr.attr.as_str() == "Model" {
                    if let Expr::Name(name) = attr.value.as_ref() {
                        if aliases.module_aliases.contains(name.id.as_str()) {
                            return true;
                        }
                    }
                }
            }
            // Model / M (where M aliases django.db.models.Model)
            Expr::Name(name) => {
                if aliases.class_aliases.contains(name.id.as_str()) {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

fn process_class_body(stmt: &Stmt, model: &mut ModelDef) {
    // Check for Meta.abstract
    if let Stmt::ClassDef(meta) = stmt {
        if meta.name.as_str() == "Meta" {
            for meta_stmt in &meta.body {
                if is_abstract_assignment(meta_stmt) {
                    model.kind = ModelKind::Abstract;
                    return;
                }
            }
        }
    }

    // Extract relation fields
    if let Some(relation) = extract_relation(stmt) {
        model.relations.push(relation);
        return;
    }

    // Extract GenericForeignKey fields
    if let Some(gfk) = extract_generic_foreign_key(stmt) {
        model.generic_foreign_keys.push(gfk);
    }
}

fn is_abstract_assignment(stmt: &Stmt) -> bool {
    let Stmt::Assign(assign) = stmt else {
        return false;
    };
    let Some(Expr::Name(name)) = assign.targets.first() else {
        return false;
    };
    if name.id.as_str() != "abstract" {
        return false;
    }
    matches!(assign.value.as_ref(), Expr::BooleanLiteral(b) if b.value)
}

fn extract_relation(stmt: &Stmt) -> Option<Relation> {
    let Stmt::Assign(assign) = stmt else {
        return None;
    };

    let Some(Expr::Name(target)) = assign.targets.first() else {
        return None;
    };

    let Expr::Call(call) = assign.value.as_ref() else {
        return None;
    };

    let relation_type = match call.func.as_ref() {
        Expr::Attribute(attr) => lookup_relation_type(attr.attr.as_str()),
        Expr::Name(name) => lookup_relation_type(name.id.as_str()),
        _ => None,
    }?;

    let target_model = extract_target_model(call)?;
    let related_name = extract_related_name(call);

    Some(Relation {
        field_name: FieldName::new(target.id.as_str()),
        target_model,
        relation_type,
        related_name,
    })
}

fn lookup_relation_type(name: &str) -> Option<RelationType> {
    RELATION_FIELDS
        .iter()
        .find(|(field_name, _)| *field_name == name)
        .map(|(_, rt)| *rt)
}

fn extract_target_model(call: &ruff_python_ast::ExprCall) -> Option<ModelName> {
    let first_arg = call.arguments.args.first()?;

    match first_arg {
        // String reference: ForeignKey("User") or ForeignKey("app.User")
        Expr::StringLiteral(s) => {
            let value = s.value.to_string();
            let name = value.split('.').next_back().unwrap_or(&value);
            Some(ModelName::new(name))
        }
        // Direct reference: ForeignKey(User)
        Expr::Name(name) => Some(ModelName::new(name.id.as_str())),
        // Attribute: ForeignKey(auth.User)
        Expr::Attribute(attr) => Some(ModelName::new(attr.attr.as_str())),
        _ => None,
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

fn extract_generic_foreign_key(stmt: &Stmt) -> Option<GenericForeignKey> {
    let Stmt::Assign(assign) = stmt else {
        return None;
    };

    let Some(Expr::Name(target)) = assign.targets.first() else {
        return None;
    };

    let Expr::Call(call) = assign.value.as_ref() else {
        return None;
    };

    let is_gfk = match call.func.as_ref() {
        Expr::Attribute(attr) => attr.attr.as_str() == "GenericForeignKey",
        Expr::Name(name) => name.id.as_str() == "GenericForeignKey",
        _ => false,
    };

    if !is_gfk {
        return None;
    }

    let ct_field =
        extract_gfk_arg(call, 0, "ct_field").unwrap_or_else(|| "content_type".to_string());
    let fk_field = extract_gfk_arg(call, 1, "fk_field").unwrap_or_else(|| "object_id".to_string());

    Some(GenericForeignKey {
        field_name: FieldName::new(target.id.as_str()),
        ct_field: FieldName::new(ct_field),
        fk_field: FieldName::new(fk_field),
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
        let source = r#"
class User(models.Model):
    name = models.CharField(max_length=100)
"#;
        let graph = extract_model_graph(source, "auth.models");
        assert_eq!(graph.len(), 1);

        let user = graph.get("User").unwrap();
        assert_eq!(user.module_path.as_str(), "auth.models");
        assert_eq!(user.line, 2);
        assert!(user.relations.is_empty());
        assert_eq!(user.kind, ModelKind::Concrete);
    }

    #[test]
    fn direct_model_import() {
        let source = r#"
from django.db.models import Model

class User(Model):
    name = models.CharField(max_length=100)
"#;
        let graph = extract_model_graph(source, "auth.models");
        assert_eq!(graph.len(), 1);
        assert!(graph.get("User").is_some());
    }

    #[test]
    fn aliased_models_import() {
        let source = r#"
from django.db import models as m

class User(m.Model):
    name = m.CharField(max_length=100)
"#;
        let graph = extract_model_graph(source, "auth.models");
        assert_eq!(graph.len(), 1);
        assert!(graph.get("User").is_some());
    }

    #[test]
    fn aliased_absolute_import() {
        let source = r#"
import django.db.models as db_models

class User(db_models.Model):
    pass
"#;
        let graph = extract_model_graph(source, "auth.models");
        assert_eq!(graph.len(), 1);
        assert!(graph.get("User").is_some());
    }

    #[test]
    fn aliased_model_class_import() {
        let source = r#"
from django.db.models import Model as BaseModel

class User(BaseModel):
    pass
"#;
        let graph = extract_model_graph(source, "auth.models");
        assert_eq!(graph.len(), 1);
        assert!(graph.get("User").is_some());
    }

    #[test]
    fn geodjango_models_import() {
        let source = r#"
from django.contrib.gis.db import models

class Location(models.Model):
    pass
"#;
        let graph = extract_model_graph(source, "geo.models");
        assert_eq!(graph.len(), 1);
        assert!(graph.get("Location").is_some());
    }

    #[test]
    fn geodjango_aliased_import() {
        let source = r#"
from django.contrib.gis.db import models as gis

class Location(gis.Model):
    pass
"#;
        let graph = extract_model_graph(source, "geo.models");
        assert_eq!(graph.len(), 1);
        assert!(graph.get("Location").is_some());
    }

    #[test]
    fn geodjango_model_class_import() {
        let source = r#"
from django.contrib.gis.db.models import Model as GeoModel

class Location(GeoModel):
    pass
"#;
        let graph = extract_model_graph(source, "geo.models");
        assert_eq!(graph.len(), 1);
        assert!(graph.get("Location").is_some());
    }

    #[test]
    fn unrelated_alias_not_matched() {
        // foo.Model should NOT be detected as a Django model
        let source = r#"
import foo

class NotAModel(foo.Model):
    pass
"#;
        let graph = extract_model_graph(source, "app.models");
        assert!(graph.is_empty());
    }

    #[test]
    fn unrelated_model_name_not_matched() {
        // A bare name that happens to not be "Model" should not match
        let source = r#"
from pydantic import BaseModel

class NotDjango(BaseModel):
    pass
"#;
        let graph = extract_model_graph(source, "app.models");
        assert!(graph.is_empty());
    }

    #[test]
    fn foreign_key() {
        let source = r#"
class User(models.Model):
    pass

class Order(models.Model):
    user = models.ForeignKey(User, on_delete=models.CASCADE)
"#;
        let graph = extract_model_graph(source, "shop.models");

        let order = graph.get("Order").unwrap();
        assert_eq!(order.relations.len(), 1);

        let rel = &order.relations[0];
        assert_eq!(rel.field_name.as_str(), "user");
        assert_eq!(rel.target_model.as_str(), "User");
        assert_eq!(rel.relation_type, RelationType::ForeignKey);
        assert_eq!(rel.related_name, None);
    }

    #[test]
    fn explicit_related_name() {
        let source = r#"
class Order(models.Model):
    user = models.ForeignKey(User, on_delete=models.CASCADE, related_name="orders")
"#;
        let graph = extract_model_graph(source, "shop.models");

        let rel = &graph.get("Order").unwrap().relations[0];
        assert_eq!(rel.related_name, Some("orders".into()));
        assert_eq!(rel.effective_related_name("Order", "shop.models"), "orders");
    }

    #[test]
    fn string_ref_with_app_label() {
        let source = r#"
class Order(models.Model):
    user = models.ForeignKey("accounts.User", on_delete=models.CASCADE)
"#;
        let graph = extract_model_graph(source, "shop.models");
        assert_eq!(
            graph.get("Order").unwrap().relations[0]
                .target_model
                .as_str(),
            "User"
        );
    }

    #[test]
    fn all_relation_types() {
        let source = r#"
class Profile(models.Model):
    user = models.OneToOneField(User, on_delete=models.CASCADE)

class Article(models.Model):
    author = models.ForeignKey(User, on_delete=models.CASCADE)
    tags = models.ManyToManyField(Tag)
"#;
        let graph = extract_model_graph(source, "app.models");

        let profile = graph.get("Profile").unwrap();
        assert_eq!(profile.relations[0].relation_type, RelationType::OneToOne);

        let article = graph.get("Article").unwrap();
        assert_eq!(article.relations.len(), 2);
        assert_eq!(article.relations[0].relation_type, RelationType::ForeignKey);
        assert_eq!(article.relations[1].relation_type, RelationType::ManyToMany);
    }

    #[test]
    fn abstract_model() {
        let source = r#"
class BaseModel(models.Model):
    class Meta:
        abstract = True
"#;
        let graph = extract_model_graph(source, "app.models");
        assert_eq!(graph.get("BaseModel").unwrap().kind, ModelKind::Abstract);
    }

    #[test]
    fn abstract_inheritance() {
        let source = r#"
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
"#;
        let graph = extract_model_graph(source, "shop.models");

        let concrete = graph.get("ConcreteOrder").unwrap();
        assert_eq!(concrete.kind, ModelKind::Concrete);
        assert_eq!(concrete.relations.len(), 2);

        let targets: Vec<&str> = concrete
            .relations
            .iter()
            .map(|r| r.target_model.as_str())
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

        let special = graph.get("SpecialOrder").unwrap();
        let rel = &special.relations[0];
        assert_eq!(
            rel.effective_related_name("SpecialOrder", "shop.models"),
            "specialorder_set"
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
        assert_eq!(graph.resolve_forward("Order", "user"), Some("User"));

        // Reverse
        assert_eq!(graph.resolve_relation("User", "orders"), Some("Order"));

        // Non-existent
        assert_eq!(graph.resolve_relation("User", "nope"), None);
    }

    #[test]
    fn default_reverse_name() {
        let source = r#"
class User(models.Model):
    pass

class Order(models.Model):
    user = models.ForeignKey(User, on_delete=models.CASCADE)
"#;
        let graph = extract_model_graph(source, "shop.models");

        // Default FK reverse name is <model>_set
        assert_eq!(graph.resolve_relation("User", "order_set"), Some("Order"));
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
        assert_eq!(graph.resolve_relation("User", "posts"), Some("Post"));
        assert_eq!(graph.resolve_relation("Post", "comments"), Some("Comment"));
        assert_eq!(graph.resolve_relation("Comment", "author"), Some("User"));
    }

    #[test]
    fn multiple_abstract_parents() {
        let source = r#"
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
"#;
        let graph = extract_model_graph(source, "app.models");

        let doc = graph.get("Document").unwrap();
        assert_eq!(doc.relations.len(), 2);

        let targets: Vec<&str> = doc
            .relations
            .iter()
            .map(|r| r.target_model.as_str())
            .collect();
        assert!(targets.contains(&"User"));
        assert!(targets.contains(&"Approver"));
    }

    #[test]
    fn concrete_model_inheritance() {
        let source = r#"
class User(models.Model):
    pass

class Place(models.Model):
    name = models.CharField(max_length=50)

class Restaurant(Place):
    owner = models.ForeignKey(User, on_delete=models.CASCADE)
"#;
        let graph = extract_model_graph(source, "app.models");

        let restaurant = graph.get("Restaurant").unwrap();
        assert_eq!(restaurant.relations.len(), 1);
        assert_eq!(restaurant.relations[0].field_name.as_str(), "owner");
        assert_eq!(restaurant.relations[0].target_model.as_str(), "User");
    }

    #[test]
    fn qualified_base_class_inheritance() {
        let source = r#"
class User(models.Model):
    pass

class BaseOrder(models.Model):
    user = models.ForeignKey(User, on_delete=models.CASCADE)

    class Meta:
        abstract = True

class ConcreteOrder(some_module.BaseOrder):
    pass
"#;
        let graph = extract_model_graph(source, "shop.models");

        let concrete = graph.get("ConcreteOrder").unwrap();
        assert_eq!(concrete.relations.len(), 1);
        assert_eq!(concrete.relations[0].target_model.as_str(), "User");
    }

    #[test]
    fn multi_level_inheritance_chain() {
        let source = r#"
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
"#;
        let graph = extract_model_graph(source, "app.models");

        // MiddleMixin inherits BaseMixin's FK to User
        let middle = graph.get("MiddleMixin").unwrap();
        assert_eq!(middle.kind, ModelKind::Abstract);
        assert_eq!(middle.relations.len(), 1);
        assert_eq!(middle.relations[0].target_model.as_str(), "User");

        // Concrete inherits through MiddleMixin
        let concrete = graph.get("Concrete").unwrap();
        assert_eq!(concrete.kind, ModelKind::Concrete);
        assert_eq!(concrete.relations.len(), 1);
        assert_eq!(concrete.relations[0].target_model.as_str(), "User");
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

        let tagged = graph.get("TaggedItem").unwrap();
        assert_eq!(tagged.relations.len(), 1);
        assert_eq!(tagged.relations[0].field_name.as_str(), "content_type");

        assert_eq!(tagged.generic_foreign_keys.len(), 1);
        let gfk = &tagged.generic_foreign_keys[0];
        assert_eq!(gfk.field_name.as_str(), "content_object");
        assert_eq!(gfk.ct_field.as_str(), "content_type");
        assert_eq!(gfk.fk_field.as_str(), "object_id");
    }

    #[test]
    fn generic_foreign_key_defaults() {
        let source = r#"
class TaggedItem(models.Model):
    content_object = GenericForeignKey()
"#;
        let graph = extract_model_graph(source, "tagging.models");

        let gfk = &graph.get("TaggedItem").unwrap().generic_foreign_keys[0];
        assert_eq!(gfk.field_name.as_str(), "content_object");
        assert_eq!(gfk.ct_field.as_str(), "content_type");
        assert_eq!(gfk.fk_field.as_str(), "object_id");
    }

    #[test]
    fn generic_foreign_key_keyword_args() {
        let source = r#"
class ObjectLog(models.Model):
    parent = GenericForeignKey(ct_field='object_type', fk_field='object_id')
"#;
        let graph = extract_model_graph(source, "logs.models");

        let gfk = &graph.get("ObjectLog").unwrap().generic_foreign_keys[0];
        assert_eq!(gfk.field_name.as_str(), "parent");
        assert_eq!(gfk.ct_field.as_str(), "object_type");
        assert_eq!(gfk.fk_field.as_str(), "object_id");
    }

    #[test]
    fn generic_foreign_key_module_prefix() {
        let source = r#"
from django.contrib.contenttypes import fields

class TaggedItem(models.Model):
    content_object = fields.GenericForeignKey("content_type", "object_id")
"#;
        let graph = extract_model_graph(source, "tagging.models");

        assert_eq!(
            graph.get("TaggedItem").unwrap().generic_foreign_keys.len(),
            1
        );
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

        let tagged = graph.get("TaggedItem").unwrap();
        assert_eq!(tagged.generic_foreign_keys.len(), 1);
        assert_eq!(
            tagged.generic_foreign_keys[0].field_name.as_str(),
            "content_object"
        );
    }

    #[test]
    fn multiple_generic_foreign_keys() {
        let source = r#"
class Action(models.Model):
    actor = GenericForeignKey('actor_content_type', 'actor_object_id')
    target = GenericForeignKey('target_content_type', 'target_object_id')
"#;
        let graph = extract_model_graph(source, "activity.models");

        let action = graph.get("Action").unwrap();
        assert_eq!(action.generic_foreign_keys.len(), 2);
        assert_eq!(action.generic_foreign_keys[0].field_name.as_str(), "actor");
        assert_eq!(
            action.generic_foreign_keys[0].ct_field.as_str(),
            "actor_content_type"
        );
        assert_eq!(action.generic_foreign_keys[1].field_name.as_str(), "target");
        assert_eq!(
            action.generic_foreign_keys[1].ct_field.as_str(),
            "target_content_type"
        );
    }
}
