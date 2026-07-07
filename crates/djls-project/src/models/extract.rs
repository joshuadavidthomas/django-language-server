use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::VecDeque;
use std::ops::ControlFlow;

use djls_source::File;
use djls_source::Spanned;
use ruff_python_ast::Expr;
use ruff_python_ast::Stmt;
use ruff_python_ast::StmtClassDef;

use crate::ExtractionStatus;
use crate::ast::ExprExt;
use crate::ast::RangedExt;
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
use crate::python::ImportTable;
#[cfg(test)]
use crate::python::ModuleKind;
use crate::python::PythonModuleName;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ModelExtraction {
    pub(super) graph: ModelGraph,
    pub(super) deferred: Vec<DeferredModel>,
    status: ExtractionStatus,
}

impl ModelExtraction {
    pub(crate) fn unparseable() -> Self {
        Self {
            graph: ModelGraph::new(),
            deferred: Vec::new(),
            status: ExtractionStatus::Unparseable,
        }
    }

    #[must_use]
    pub(crate) fn graph(&self) -> &ModelGraph {
        &self.graph
    }

    #[must_use]
    pub(crate) fn status(&self) -> ExtractionStatus {
        self.status
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct DeferredModel {
    pub(super) model: ModelDef,
    pub(super) bases: Vec<DeferredBaseRef>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum DeferredBaseRef {
    Qualified(PythonModuleName),
    SameModule(ModelName),
}

struct DeferredCandidate<'a> {
    class: &'a StmtClassDef,
    bases: Vec<DeferredBaseRef>,
}

impl<'a> DeferredCandidate<'a> {
    fn from_class(class: &'a StmtClassDef, imports: &ImportTable) -> Option<Self> {
        let args = class.arguments.as_ref()?;
        let bases: Vec<_> = args
            .args
            .iter()
            .filter_map(|arg| DeferredBaseRef::from_expr(arg, imports))
            .collect();

        if bases.is_empty() {
            return None;
        }

        Some(Self { class, bases })
    }
}

struct ModelCollector<'a> {
    module_name: PythonModuleName,
    file: File,
    imports: &'a ImportTable,
    graph: ModelGraph,
    children: Vec<&'a StmtClassDef>,
}

impl<'a> ModelCollector<'a> {
    fn new(module_name: PythonModuleName, file: File, imports: &'a ImportTable) -> Self {
        Self {
            module_name,
            file,
            imports,
            graph: ModelGraph::new(),
            children: Vec::new(),
        }
    }

    fn scan_stmt(&mut self, stmt: &'a Stmt) {
        if let Stmt::ClassDef(class) = stmt {
            let Some(ref args) = class.arguments else {
                return;
            };

            if is_django_model(args.args.iter(), self.imports) {
                let mut model = ModelDef::new(
                    class.name.to_string(),
                    self.module_name.clone(),
                    self.file,
                    class.name.span(),
                );

                walk_stmts(&class.body, Recurse::Flat, |stmt| {
                    process_class_body(stmt, self.file, &mut model);
                    ControlFlow::Continue(())
                });

                self.graph.add_model(model);
            } else if !args.args.is_empty() {
                self.children.push(class);
            }
        }
    }
}

fn resolve_children<'a>(
    graph: &mut ModelGraph,
    children: &[&'a StmtClassDef],
    module_name: &PythonModuleName,
    file: File,
) -> Vec<&'a StmtClassDef> {
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
            .map(|m| (m.name.value().clone(), m.relations.clone()))
            .collect();
        let known_names: Vec<ModelName> = graph.models().map(|m| m.name.value().clone()).collect();

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

            let mut model = ModelDef::new(
                class.name.to_string(),
                module_name.clone(),
                file,
                class.name.span(),
            );

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
                process_class_body(stmt, file, &mut model);
                ControlFlow::Continue(())
            });

            graph.add_model(model);
        }

        remaining = unresolved;
        if remaining.len() == before {
            break;
        }
    }

    remaining
}

pub(super) fn extract_models_impl(
    stmts: &[Stmt],
    module_name: PythonModuleName,
    file: File,
    imports: &ImportTable,
) -> ModelExtraction {
    let mut collector = ModelCollector::new(module_name, file, imports);
    walk_stmts(stmts, Recurse::Flat, |stmt| {
        collector.scan_stmt(stmt);
        ControlFlow::Continue(())
    });
    let remaining = resolve_children(
        &mut collector.graph,
        &collector.children,
        &collector.module_name,
        file,
    );
    let candidates: Vec<_> = remaining
        .into_iter()
        .filter_map(|class| DeferredCandidate::from_class(class, imports))
        .collect();
    let deferred = viable_deferred_candidates(candidates)
        .into_iter()
        .map(|candidate| DeferredModel::from_candidate(candidate, &collector.module_name, file))
        .collect();
    ModelExtraction {
        graph: collector.graph,
        deferred,
        // Model extraction skips unsupported bases and dynamic constructs in v1.
        status: ExtractionStatus::Partial,
    }
}

fn viable_deferred_candidates(
    candidates: Vec<DeferredCandidate<'_>>,
) -> Vec<DeferredCandidate<'_>> {
    let mut viable_names = BTreeSet::new();
    let mut queue = VecDeque::new();
    let mut children_by_base: BTreeMap<&str, Vec<usize>> = BTreeMap::new();

    // Same-module models can only come from this file, so a deferred child that
    // reaches no qualified base through the same-file deferred graph can never
    // resolve during the project pass. Build the reverse same-file inheritance
    // graph once, then mark viability from qualified-base roots in one walk.
    for (index, candidate) in candidates.iter().enumerate() {
        let candidate_name = candidate.class.name.as_str();
        if candidate
            .bases
            .iter()
            .any(|base| matches!(base, DeferredBaseRef::Qualified(_)))
            && viable_names.insert(candidate_name)
        {
            queue.push_back(candidate_name);
        }

        for base in &candidate.bases {
            if let DeferredBaseRef::SameModule(name) = base {
                children_by_base
                    .entry(name.as_str())
                    .or_default()
                    .push(index);
            }
        }
    }

    while let Some(base_name) = queue.pop_front() {
        let Some(children) = children_by_base.get(base_name) else {
            continue;
        };

        for &index in children {
            let candidate_name = candidates[index].class.name.as_str();
            if viable_names.insert(candidate_name) {
                queue.push_back(candidate_name);
            }
        }
    }

    candidates
        .into_iter()
        .filter(|candidate| viable_names.contains(candidate.class.name.as_str()))
        .collect()
}

impl DeferredModel {
    fn from_candidate(
        candidate: DeferredCandidate<'_>,
        module_name: &PythonModuleName,
        file: File,
    ) -> Self {
        let mut model = ModelDef::new(
            candidate.class.name.to_string(),
            module_name.clone(),
            file,
            candidate.class.name.span(),
        );
        walk_stmts(&candidate.class.body, Recurse::Flat, |stmt| {
            process_class_body(stmt, file, &mut model);
            ControlFlow::Continue(())
        });

        Self {
            model,
            bases: candidate.bases,
        }
    }
}

impl DeferredBaseRef {
    fn from_expr(expr: &Expr, imports: &ImportTable) -> Option<Self> {
        let path = expr.path_segments()?;
        if let Ok(path) = imports.resolve_qualified_path(path.iter().map(String::as_str)) {
            return Some(Self::Qualified(path));
        }

        base_class_name(expr).map(|name| Self::SameModule(ModelName::new(name)))
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

fn is_django_model<'a>(bases: impl Iterator<Item = &'a Expr>, imports: &ImportTable) -> bool {
    bases
        .filter_map(ExprExt::path_segments)
        .filter_map(|path| {
            imports
                .resolve_qualified_path(path.iter().map(String::as_str))
                .ok()
        })
        .any(|path| {
            matches!(
                path.as_str(),
                "django.db.models.Model" | "django.contrib.gis.db.models.Model"
            )
        })
}

fn process_class_body(stmt: &Stmt, file: File, model: &mut ModelDef) {
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
    if let Some(relation) = extract_relation(stmt, file) {
        model.relations.push(relation);
        return;
    }

    // Extract GenericForeignKey fields
    if let Some(gfk) = extract_generic_foreign_key(stmt, file) {
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

fn extract_relation(stmt: &Stmt, file: File) -> Option<Relation> {
    let Stmt::Assign(assign) = stmt else {
        return None;
    };

    let target_expr = assign.targets.first()?;
    let Expr::Name(target_name) = target_expr else {
        return None;
    };

    let Expr::Call(call) = assign.value.as_ref() else {
        return None;
    };

    let field_class_name = match call.func.as_ref() {
        Expr::Attribute(attr) => attr.attr.as_str(),
        expr => expr.name_target()?,
    };

    let first_arg = call.arguments.args.first()?;
    let target = match first_arg {
        Expr::StringLiteral(s) => {
            let value = s.value.to_string();
            if value == "self" {
                RelationTarget::SelfRef
            } else if let Some((app_label, name)) = value.rsplit_once('.') {
                RelationTarget::Qualified {
                    app_label: app_label.to_string(),
                    name: ModelName::new(name),
                }
            } else {
                RelationTarget::Bare {
                    name: ModelName::new(value),
                }
            }
        }
        expr => {
            let path = expr.path_segments()?;
            if path.len() == 1 {
                RelationTarget::Bare {
                    name: ModelName::new(path[0].clone()),
                }
            } else {
                RelationTarget::Attribute { path }
            }
        }
    };
    let related_name = extract_related_name(call);

    let relation_type = RelationType::from_field_class(
        field_class_name,
        Spanned::new(target, first_arg.span()),
        related_name,
    )?;

    Some(Relation::new(
        file,
        Spanned::new(
            FieldName::new(target_name.id.to_string()),
            target_name.span(),
        ),
        relation_type,
    ))
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

fn extract_generic_foreign_key(stmt: &Stmt, file: File) -> Option<Relation> {
    let Stmt::Assign(assign) = stmt else {
        return None;
    };

    let target_expr = assign.targets.first()?;
    let Expr::Name(target_name) = target_expr else {
        return None;
    };

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

    Some(Relation::new(
        file,
        Spanned::new(
            FieldName::new(target_name.id.to_string()),
            target_name.span(),
        ),
        RelationType::GenericForeignKey {
            ct_field: FieldName::new(ct_field),
            fk_field: FieldName::new(fk_field),
        },
    ))
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

#[cfg(test)]
mod tests {
    use camino::Utf8Path;
    use djls_source::Span;
    use djls_testing::TestDatabase;

    use super::*;
    use crate::models::graph::ModelId;

    fn extract_model_graph(source: &str, module_name: &str) -> ModelGraph {
        let db = TestDatabase::new();
        db.add_file("/test.py", source);
        let file = db.file(Utf8Path::new("/test.py"));
        let module_name = PythonModuleName::parse(module_name).unwrap();
        let imports = crate::python::extract_import_table_for_source(
            source,
            &module_name,
            ModuleKind::Module,
        );
        let Ok(parsed) = ruff_python_parser::parse_module(source) else {
            return ModelGraph::default();
        };
        let module = parsed.into_syntax();
        super::extract_models_impl(&module.body, module_name, file, &imports).graph
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
            RelationTarget::SelfRef
            | RelationTarget::Qualified { .. }
            | RelationTarget::Attribute { .. } => None,
        }
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
        let source = "from django.db import models\nclass User(models.Model):\n    name = models.CharField(max_length=100)\n";
        let graph = extract_model_graph(source, "auth.models");
        assert_eq!(graph.len(), 1);

        let user = model(&graph, "User");
        assert_eq!(user.module_name.as_str(), "auth.models");
        assert_eq!(user.name.span(), Span::new(35, 4));
        assert!(user.relations.is_empty());
        assert_eq!(user.kind, ModelKind::Concrete);
    }

    #[test]
    fn direct_model_import() {
        let source = r"
from django.db import models
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
from django.db import models

class User(models.Model):
    pass

class Order(models.Model):
    user = models.ForeignKey(User, on_delete=models.CASCADE)
";
        let graph = extract_model_graph(source, "shop.models");

        let order = model(&graph, "Order");
        assert_eq!(order.relations.len(), 1);

        let rel = &order.relations[0];
        assert_eq!(rel.field_name.value().as_str(), "user");
        assert_eq!(bare_target_name(rel), Some("User"));
        assert!(matches!(
            rel.relation_type,
            RelationType::ForeignKey { ref related_name, .. } if related_name.is_none()
        ));
    }

    #[test]
    fn explicit_related_name() {
        let source = r#"
from django.db import models

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
from django.db import models

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
from django.db import models

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
from django.db import models

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
from django.db import models

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
from django.db import models

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
from django.db import models

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
from django.db import models

class User(models.Model):
    pass

class Order(models.Model):
    user = models.ForeignKey(User, on_delete=models.CASCADE, related_name="orders")
"#;
        let graph = extract_model_graph(source, "shop.models");

        // Forward
        assert_eq!(
            graph
                .resolve_forward(model_id(&graph, "Order"), "user")
                .map(|model| model.name.value().as_str()),
            Some("User")
        );

        // Reverse
        assert_eq!(
            graph
                .resolve_relation(model_id(&graph, "User"), "orders")
                .map(|model| model.name.value().as_str()),
            Some("Order")
        );

        // Non-existent
        assert_eq!(
            graph
                .resolve_relation(model_id(&graph, "User"), "nope")
                .map(|model| model.name.value().as_str()),
            None
        );
    }

    #[test]
    fn default_reverse_name() {
        let source = r"
from django.db import models

class User(models.Model):
    pass

class Order(models.Model):
    user = models.ForeignKey(User, on_delete=models.CASCADE)
";
        let graph = extract_model_graph(source, "shop.models");

        // Default FK reverse name is <model>_set
        assert_eq!(
            graph
                .resolve_relation(model_id(&graph, "User"), "order_set")
                .map(|model| model.name.value().as_str()),
            Some("Order")
        );
    }

    #[test]
    fn multiple_models_multiple_relations() {
        let source = r#"
from django.db import models

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
            graph
                .resolve_relation(model_id(&graph, "User"), "posts")
                .map(|model| model.name.value().as_str()),
            Some("Post")
        );
        assert_eq!(
            graph
                .resolve_relation(model_id(&graph, "Post"), "comments")
                .map(|model| model.name.value().as_str()),
            Some("Comment")
        );
        assert_eq!(
            graph
                .resolve_relation(model_id(&graph, "Comment"), "author")
                .map(|model| model.name.value().as_str()),
            Some("User")
        );
    }

    #[test]
    fn multiple_abstract_parents() {
        let source = r"
from django.db import models

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
from django.db import models

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
        assert_eq!(restaurant.relations[0].field_name.value().as_str(), "owner");
        assert_eq!(bare_target_name(&restaurant.relations[0]), Some("User"));
    }

    #[test]
    fn qualified_base_class_inheritance() {
        let source = r"
from django.db import models

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
from django.db import models

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
from django.db import models

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
        assert_eq!(
            tagged.relations[0].field_name.value().as_str(),
            "content_type"
        );
        assert!(matches!(
            tagged.relations[0].relation_type,
            RelationType::ForeignKey { .. }
        ));

        // Second relation: GFK
        assert_eq!(
            tagged.relations[1].field_name.value().as_str(),
            "content_object"
        );
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
from django.db import models

class TaggedItem(models.Model):
    content_object = GenericForeignKey()
";
        let graph = extract_model_graph(source, "tagging.models");

        let rel = &model(&graph, "TaggedItem").relations[0];
        assert_eq!(rel.field_name.value().as_str(), "content_object");
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
from django.db import models

class ObjectLog(models.Model):
    parent = GenericForeignKey(ct_field='object_type', fk_field='object_id')
";
        let graph = extract_model_graph(source, "logs.models");

        let rel = &model(&graph, "ObjectLog").relations[0];
        assert_eq!(rel.field_name.value().as_str(), "parent");
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
from django.db import models
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
from django.db import models

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
        assert_eq!(
            tagged.relations[0].field_name.value().as_str(),
            "content_object"
        );
        assert!(matches!(
            tagged.relations[0].relation_type,
            RelationType::GenericForeignKey { .. }
        ));
    }

    #[test]
    fn multiple_generic_foreign_keys() {
        let source = r"
from django.db import models

class Action(models.Model):
    actor = GenericForeignKey('actor_content_type', 'actor_object_id')
    target = GenericForeignKey('target_content_type', 'target_object_id')
";
        let graph = extract_model_graph(source, "activity.models");

        let action = model(&graph, "Action");
        assert_eq!(action.relations.len(), 2);
        assert_eq!(action.relations[0].field_name.value().as_str(), "actor");
        assert!(matches!(
            action.relations[0].relation_type,
            RelationType::GenericForeignKey {
                ref ct_field,
                ..
            } if ct_field.as_str() == "actor_content_type"
        ));
        assert_eq!(action.relations[1].field_name.value().as_str(), "target");
        assert!(matches!(
            action.relations[1].relation_type,
            RelationType::GenericForeignKey {
                ref ct_field,
                ..
            } if ct_field.as_str() == "target_content_type"
        ));
    }
}
