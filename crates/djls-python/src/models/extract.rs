use ruff_python_ast::Expr;
use ruff_python_ast::Stmt;
use ruff_python_ast::StmtClassDef;

use super::graph::ModelDef;
use super::graph::ModelGraph;
use super::graph::Relation;
use super::graph::RelationType;

const RELATION_FIELDS: &[(&str, RelationType)] = &[
    ("ForeignKey", RelationType::ForeignKey),
    ("OneToOneField", RelationType::OneToOne),
    ("ManyToManyField", RelationType::ManyToMany),
    ("GenericForeignKey", RelationType::GenericForeignKey),
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

    extract_models_from_body(&module.body, module_path, source)
}

fn extract_models_from_body(body: &[Stmt], module_path: &str, source: &str) -> ModelGraph {
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

        if is_django_model(args.args.iter()) {
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
    // Collect abstract model data before mutating graph
    let abstracts: Vec<(String, Vec<Relation>)> = graph
        .models()
        .filter(|m| m.is_abstract)
        .map(|m| (m.name.clone(), m.relations.clone()))
        .collect();

    for class in children {
        let Some(ref args) = class.arguments else {
            continue;
        };

        // Find a parent that's an abstract model in this graph
        let parent = args.args.iter().find_map(|arg| {
            let name = match arg {
                Expr::Name(n) => n.id.as_str(),
                _ => return None,
            };
            abstracts.iter().find(|(model_name, _)| model_name == name)
        });

        let Some((_, parent_relations)) = parent else {
            continue;
        };

        let line = line_number(source, class.range.start().to_usize());
        let mut model = ModelDef::new(class.name.to_string(), module_path, line);

        // Copy parent relations
        model.relations.extend(parent_relations.iter().cloned());

        // Parse child's own body
        for body_stmt in &class.body {
            process_class_body(body_stmt, &mut model);
        }

        graph.add_model(model);
    }
}

fn is_django_model<'a>(bases: impl Iterator<Item = &'a Expr>) -> bool {
    for base in bases {
        match base {
            // models.Model
            Expr::Attribute(attr) => {
                if attr.attr.as_str() == "Model" {
                    if let Expr::Name(name) = attr.value.as_ref() {
                        if name.id.as_str() == "models" {
                            return true;
                        }
                    }
                }
            }
            // Model (direct import)
            Expr::Name(name) => {
                if name.id.as_str() == "Model" {
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
                    model.is_abstract = true;
                    return;
                }
            }
        }
    }

    // Extract relation fields
    if let Some(relation) = extract_relation(stmt) {
        model.relations.push(relation);
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
        field_name: target.id.to_string(),
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

fn extract_target_model(call: &ruff_python_ast::ExprCall) -> Option<String> {
    let first_arg = call.arguments.args.first()?;

    match first_arg {
        // String reference: ForeignKey("User") or ForeignKey("app.User")
        Expr::StringLiteral(s) => {
            let value = s.value.to_string();
            Some(value.split('.').next_back().unwrap_or(&value).to_string())
        }
        // Direct reference: ForeignKey(User)
        Expr::Name(name) => Some(name.id.to_string()),
        // Attribute: ForeignKey(auth.User)
        Expr::Attribute(attr) => Some(attr.attr.to_string()),
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

fn line_number(source: &str, offset: usize) -> usize {
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
        assert_eq!(user.module_path, "auth.models");
        assert_eq!(user.line, 2);
        assert!(user.relations.is_empty());
        assert!(!user.is_abstract);
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
        assert_eq!(rel.field_name, "user");
        assert_eq!(rel.target_model, "User");
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
        assert_eq!(rel.effective_related_name("Order"), "orders");
    }

    #[test]
    fn string_ref_with_app_label() {
        let source = r#"
class Order(models.Model):
    user = models.ForeignKey("accounts.User", on_delete=models.CASCADE)
"#;
        let graph = extract_model_graph(source, "shop.models");
        assert_eq!(
            graph.get("Order").unwrap().relations[0].target_model,
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
        assert!(graph.get("BaseModel").unwrap().is_abstract);
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
        assert!(!concrete.is_abstract);
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
            rel.effective_related_name("SpecialOrder"),
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
        assert_eq!(
            graph.resolve_relation("User", "orders"),
            Some("Order".to_string())
        );

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
        assert_eq!(
            graph.resolve_relation("User", "order_set"),
            Some("Order".to_string())
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
            graph.resolve_relation("User", "posts"),
            Some("Post".to_string())
        );
        assert_eq!(
            graph.resolve_relation("Post", "comments"),
            Some("Comment".to_string())
        );
        assert_eq!(
            graph.resolve_relation("Comment", "author"),
            Some("User".to_string())
        );
    }
}
