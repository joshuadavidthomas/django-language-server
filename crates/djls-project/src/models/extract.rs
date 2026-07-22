use std::collections::BTreeMap;
use std::collections::BTreeSet;

use djls_source::File;
use djls_source::Spanned;
use ruff_python_ast::Expr;
use ruff_python_ast::Stmt;
use ruff_python_ast::StmtClassDef;

use crate::ast::ExprExt;
use crate::ast::RangedExt;
use crate::models::graph::ClassName;
use crate::models::graph::FieldName;
use crate::models::graph::ModelDef;
use crate::models::graph::ModelKind;
use crate::models::graph::ModelName;
use crate::models::graph::Relation;
use crate::models::graph::RelationTarget;
use crate::models::graph::RelationType;
use crate::models::imports::ModelImportPathResolutionError;
use crate::models::imports::ModelImportState;
use crate::python::PythonModuleName;
use crate::python::import::DirectImportClause;
use crate::python::import::FromImportSyntax;
use crate::python::import::ModuleKind;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ExtractedClasses(Vec<ExtractedClass>);

impl ExtractedClasses {
    pub(super) fn as_slice(&self) -> &[ExtractedClass] {
        &self.0
    }
}

/// A Python class declaration and the source facts extracted from its body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ExtractedClass {
    pub(super) file: File,
    pub(super) name: Spanned<ClassName>,
    pub(super) module_name: PythonModuleName,
    relations: Vec<Relation>,
    pub(super) declared_model_kind: ModelKind,
    pub(super) bases: Vec<Spanned<ExtractedBaseRef>>,
    pub(super) local_bindings: BTreeMap<FieldName, LocalBinding>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum LocalBinding {
    Relation(usize),
    Other,
}

/// A base expression as understood from imports at its source occurrence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum ExtractedBaseRef {
    DjangoModelRoot,
    Qualified(PythonModuleName),
    SameModule(ClassName),
    UnsupportedExpression,
    MissingBinding { path: Vec<String> },
    ShadowedBinding { path: Vec<String> },
    InvalidTarget { target: String },
}

impl ExtractedClass {
    fn push_local_relation(&mut self, relation: Relation) {
        let name = relation.field_name.value().clone();
        let relation_index = self.relations.len();
        self.relations.push(relation);
        self.local_bindings
            .insert(name, LocalBinding::Relation(relation_index));
    }

    fn bind_local_other(&mut self, name: FieldName) {
        self.local_bindings.insert(name, LocalBinding::Other);
    }

    pub(super) fn has_local_relation_binding(&self) -> bool {
        self.local_bindings
            .values()
            .any(|binding| matches!(binding, LocalBinding::Relation(_)))
    }

    pub(super) fn into_admitted_model(self) -> (ModelDef, BTreeMap<FieldName, LocalBinding>) {
        let definition = ModelDef {
            file: self.file,
            name: Spanned::new(ModelName::new(self.name.value().as_str()), self.name.span()),
            module_name: self.module_name,
            relations: self.relations,
            kind: self.declared_model_kind,
        };
        (definition, self.local_bindings)
    }
}

#[derive(Clone, Copy)]
struct ModelExtractionContext<'a> {
    module_name: &'a PythonModuleName,
    file: File,
    module_kind: ModuleKind,
}

enum ModelExtractionTarget<'out> {
    Module {
        classes: &'out mut Vec<ExtractedClass>,
    },
    Class {
        extracted_class: &'out mut ExtractedClass,
    },
}

fn invalidate_names(state: &mut ModelImportState, names: &BTreeSet<String>) {
    for name in names {
        state.invalidate_root(name);
    }
}

fn scan_class(
    class: &StmtClassDef,
    aliases: &ModelImportState,
    target: &mut ModelExtractionTarget<'_>,
    context: ModelExtractionContext<'_>,
) {
    let ModelExtractionTarget::Module { classes } = target else {
        return;
    };
    let bases = class
        .arguments
        .iter()
        .flat_map(|arguments| &arguments.args)
        .map(|arg| Spanned::new(ExtractedBaseRef::from_expr(arg, aliases), arg.span()))
        .collect();
    let mut extracted_class = ExtractedClass {
        file: context.file,
        name: Spanned::new(ClassName::new(class.name.to_string()), class.name.span()),
        module_name: context.module_name.clone(),
        relations: Vec::new(),
        declared_model_kind: ModelKind::Concrete,
        bases,
        local_bindings: BTreeMap::new(),
    };
    let mut class_state = aliases.clone();
    let mut class_target = ModelExtractionTarget::Class {
        extracted_class: &mut extracted_class,
    };
    scan_statements(
        &class.body,
        &mut class_state,
        &mut class_target,
        context,
        true,
    );
    classes.push(extracted_class);
}

/// Scan model-relevant occurrences in source order.
///
/// Module scans collect Python classes. Class scans collect Django relation and
/// `Meta` evidence. Compound bodies share this transition engine but receive
/// independent alias clones, so contained facts see their branch's exact source
/// order while no branch-local alias leaks into the following statement.
fn scan_statements(
    stmts: &[Stmt],
    state: &mut ModelImportState,
    target: &mut ModelExtractionTarget<'_>,
    context: ModelExtractionContext<'_>,
    record_local_bindings: bool,
) {
    for stmt in stmts {
        if record_local_bindings && let ModelExtractionTarget::Class { extracted_class } = target {
            let mut names = BTreeSet::new();
            collect_touched_roots(stmt, &mut names);
            for name in names {
                extracted_class.bind_local_other(FieldName::new(name));
            }
        }

        if let Stmt::Import(import) = stmt {
            state.apply_direct_import(&DirectImportClause::lower(import));
            continue;
        }
        if let Stmt::ImportFrom(import) = stmt {
            state.apply_from_import(
                &FromImportSyntax::lower(import),
                context.module_name,
                context.module_kind,
            );
            continue;
        }
        if let Stmt::ClassDef(class) = stmt {
            match target {
                ModelExtractionTarget::Module { .. } => {
                    scan_class(class, state, target, context);
                    state.bind_local_class(class.name.as_str());
                }
                ModelExtractionTarget::Class { extracted_class } => {
                    process_class_body(
                        stmt,
                        context.file,
                        extracted_class,
                        state,
                        record_local_bindings,
                    );
                    state.invalidate_root(class.name.as_str());
                }
            }
            continue;
        }

        if let ModelExtractionTarget::Class { extracted_class } = target {
            process_class_body(
                stmt,
                context.file,
                extracted_class,
                state,
                record_local_bindings,
            );
        }
        scan_compound(stmt, state, target, context);

        let mut roots = BTreeSet::new();
        collect_touched_roots(stmt, &mut roots);
        invalidate_names(state, &roots);
    }
}

fn scan_compound(
    stmt: &Stmt,
    entry: &ModelImportState,
    target: &mut ModelExtractionTarget<'_>,
    context: ModelExtractionContext<'_>,
) {
    let mut scan = |body: &[Stmt], invalidated: &BTreeSet<String>| {
        let mut state = entry.clone();
        invalidate_names(&mut state, invalidated);
        scan_statements(body, &mut state, target, context, false);
    };
    let none = BTreeSet::new();

    if let Stmt::For(statement) = stmt {
        let mut targets = BTreeSet::new();
        push_name_targets(&statement.target, &mut targets);
        scan(&statement.body, &targets);
        scan(&statement.orelse, &none);
        return;
    }
    if let Stmt::While(statement) = stmt {
        scan(&statement.body, &none);
        scan(&statement.orelse, &none);
        return;
    }
    if let Stmt::If(statement) = stmt {
        scan(&statement.body, &none);
        for clause in &statement.elif_else_clauses {
            scan(&clause.body, &none);
        }
        return;
    }
    if let Stmt::With(statement) = stmt {
        let mut optional_variables = BTreeSet::new();
        for item in &statement.items {
            if let Some(variables) = &item.optional_vars {
                push_name_targets(variables, &mut optional_variables);
            }
        }
        scan(&statement.body, &optional_variables);
        return;
    }
    if let Stmt::Try(statement) = stmt {
        scan(&statement.body, &none);
        for handler in &statement.handlers {
            let ruff_python_ast::ExceptHandler::ExceptHandler(handler) = handler;
            let mut exception_name = BTreeSet::new();
            if let Some(name) = &handler.name {
                exception_name.insert(name.to_string());
            }
            scan(&handler.body, &exception_name);
        }
        scan(&statement.orelse, &none);
        scan(&statement.finalbody, &none);
        return;
    }
    if let Stmt::Match(statement) = stmt {
        for case in &statement.cases {
            let mut pattern_names = BTreeSet::new();
            collect_pattern_names(&case.pattern, &mut pattern_names);
            scan(&case.body, &pattern_names);
        }
    }
}

pub(super) fn extract_models_impl(
    stmts: &[Stmt],
    module_name: &PythonModuleName,
    file: File,
    module_kind: ModuleKind,
) -> ExtractedClasses {
    let mut classes = Vec::new();
    let mut state = ModelImportState::default();
    let context = ModelExtractionContext {
        module_name,
        file,
        module_kind,
    };
    let mut target = ModelExtractionTarget::Module {
        classes: &mut classes,
    };
    scan_statements(stmts, &mut state, &mut target, context, false);

    ExtractedClasses(classes)
}

impl ExtractedBaseRef {
    fn from_expr(expr: &Expr, aliases: &ModelImportState) -> Self {
        let Some(path) = expr.path_segments() else {
            return Self::UnsupportedExpression;
        };
        let Some((root, tail)) = path.split_first() else {
            return Self::UnsupportedExpression;
        };
        match aliases.resolve_qualified_path(root, tail) {
            Ok(path)
                if matches!(
                    path.as_str(),
                    "django.db.models.Model" | "django.contrib.gis.db.models.Model"
                ) =>
            {
                Self::DjangoModelRoot
            }
            Ok(path) => Self::Qualified(path),
            Err(ModelImportPathResolutionError::MissingBinding) if path.len() == 1 => {
                Self::SameModule(ClassName::new(path[0].clone()))
            }
            Err(ModelImportPathResolutionError::MissingBinding) => Self::MissingBinding { path },
            Err(ModelImportPathResolutionError::ShadowedBinding) => Self::ShadowedBinding { path },
            Err(ModelImportPathResolutionError::InvalidTarget { target, .. }) => {
                Self::InvalidTarget { target }
            }
        }
    }
}

fn process_class_body(
    stmt: &Stmt,
    file: File,
    extracted_class: &mut ExtractedClass,
    aliases: &ModelImportState,
    record_local_binding: bool,
) {
    if record_local_binding
        && let Stmt::ClassDef(meta) = stmt
        && meta.name.as_str() == "Meta"
    {
        // A class statement rebinds `Meta` when it finishes. Start each
        // top-level declaration from Django's concrete default so an earlier
        // `Meta.abstract` value cannot survive a later replacement class.
        extracted_class.declared_model_kind = ModelKind::Concrete;
        for meta_stmt in &meta.body {
            if let Some(is_abstract) = static_abstract_assignment(meta_stmt) {
                extracted_class.declared_model_kind = if is_abstract {
                    ModelKind::Abstract
                } else {
                    ModelKind::Concrete
                };
            }
        }
    }

    let relation =
        extract_relation(stmt, file, aliases).or_else(|| extract_generic_foreign_key(stmt, file));
    let Some(relation) = relation else {
        return;
    };
    extracted_class.push_local_relation(relation);
}

fn static_abstract_assignment(stmt: &Stmt) -> Option<bool> {
    let Stmt::Assign(assign) = stmt else {
        return None;
    };
    let target = assign.targets.first()?;
    if target.name_target() != Some("abstract") {
        return None;
    }
    let Expr::BooleanLiteral(value) = assign.value.as_ref() else {
        return None;
    };
    Some(value.value)
}

fn relation_target_expr(call: &ruff_python_ast::ExprCall) -> Option<&Expr> {
    call.arguments.args.first().or_else(|| {
        call.arguments
            .keywords
            .iter()
            .find(|keyword| {
                keyword
                    .arg
                    .as_ref()
                    .is_some_and(|name| name.as_str() == "to")
            })
            .map(|keyword| &keyword.value)
    })
}

fn extract_relation(stmt: &Stmt, file: File, aliases: &ModelImportState) -> Option<Relation> {
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

    let field_class_name = if let Expr::Attribute(attr) = call.func.as_ref() {
        attr.attr.as_str()
    } else {
        call.func.name_target()?
    };

    let target_expr = relation_target_expr(call)?;
    let target = if let Expr::StringLiteral(string) = target_expr {
        let value = string.value.to_string();
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
                import_reference: None,
            }
        }
    } else {
        let path = target_expr.path_segments()?;
        let (root, tail) = path.split_first()?;
        let import_reference = aliases.resolve_reference(root, tail);
        if path.len() == 1 {
            RelationTarget::Bare {
                name: ModelName::new(path[0].clone()),
                import_reference: Some(import_reference),
            }
        } else {
            RelationTarget::Attribute {
                path,
                import_reference,
            }
        }
    };
    let related_name = extract_related_name(call);

    let relation_type = RelationType::from_field_class(
        field_class_name,
        Spanned::new(target, target_expr.span()),
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

fn push_name_targets(target: &Expr, out: &mut BTreeSet<String>) {
    if let Expr::Name(name) = target {
        out.insert(name.id.to_string());
        return;
    }
    if let Expr::Tuple(tuple) = target {
        for element in &tuple.elts {
            push_name_targets(element, out);
        }
        return;
    }
    if let Expr::List(list) = target {
        for element in &list.elts {
            push_name_targets(element, out);
        }
        return;
    }
    if let Expr::Starred(starred) = target {
        push_name_targets(&starred.value, out);
    }
}

fn collect_pattern_names(pattern: &ruff_python_ast::Pattern, out: &mut BTreeSet<String>) {
    use ruff_python_ast::Pattern;
    match pattern {
        Pattern::MatchValue(_) | Pattern::MatchSingleton(_) => {}
        Pattern::MatchSequence(sequence) => {
            for pattern in &sequence.patterns {
                collect_pattern_names(pattern, out);
            }
        }
        Pattern::MatchMapping(mapping) => {
            if let Some(rest) = &mapping.rest {
                out.insert(rest.to_string());
            }
            for pattern in &mapping.patterns {
                collect_pattern_names(pattern, out);
            }
        }
        Pattern::MatchClass(class) => {
            for pattern in &class.arguments.patterns {
                collect_pattern_names(pattern, out);
            }
            for keyword in &class.arguments.keywords {
                collect_pattern_names(&keyword.pattern, out);
            }
        }
        Pattern::MatchStar(star) => {
            if let Some(name) = &star.name {
                out.insert(name.to_string());
            }
        }
        Pattern::MatchAs(match_as) => {
            if let Some(name) = &match_as.name {
                out.insert(name.to_string());
            }
            if let Some(pattern) = &match_as.pattern {
                collect_pattern_names(pattern, out);
            }
        }
        Pattern::MatchOr(match_or) => {
            for pattern in &match_or.patterns {
                collect_pattern_names(pattern, out);
            }
        }
    }
}

/// Collect every root name bound, written, imported, or deleted by `stmt`,
/// recursing through compound statement bodies. Used to conservatively
/// invalidate occurrence-local aliases; over-approximation is safe.
fn collect_touched_roots(stmt: &Stmt, out: &mut BTreeSet<String>) {
    match stmt {
        Stmt::Import(import) => {
            for clause in DirectImportClause::lower(import) {
                out.insert(clause.bound().to_string());
            }
        }
        Stmt::ImportFrom(import) => {
            for clause in FromImportSyntax::lower(import).named_members() {
                out.insert(clause.bound().to_string());
            }
        }
        Stmt::Assign(assign) => {
            for target in &assign.targets {
                push_name_targets(target, out);
            }
        }
        Stmt::AnnAssign(assign) => push_name_targets(&assign.target, out),
        Stmt::AugAssign(assign) => push_name_targets(&assign.target, out),
        Stmt::Delete(delete) => {
            for target in &delete.targets {
                push_name_targets(target, out);
            }
        }
        Stmt::TypeAlias(alias) => push_name_targets(&alias.name, out),
        Stmt::FunctionDef(function) => {
            out.insert(function.name.to_string());
        }
        Stmt::ClassDef(class) => {
            out.insert(class.name.to_string());
        }
        Stmt::For(_)
        | Stmt::While(_)
        | Stmt::If(_)
        | Stmt::With(_)
        | Stmt::Try(_)
        | Stmt::Match(_) => collect_compound_touched_roots(stmt, out),
        Stmt::Expr(_)
        | Stmt::Return(_)
        | Stmt::Raise(_)
        | Stmt::Assert(_)
        | Stmt::Global(_)
        | Stmt::Nonlocal(_)
        | Stmt::Pass(_)
        | Stmt::Break(_)
        | Stmt::Continue(_)
        | Stmt::IpyEscapeCommand(_) => {}
    }
}

/// Collect touched roots from a compound statement's targets and nested bodies.
fn collect_compound_touched_roots(stmt: &Stmt, out: &mut BTreeSet<String>) {
    fn recurse(body: &[Stmt], out: &mut BTreeSet<String>) {
        for stmt in body {
            collect_touched_roots(stmt, out);
        }
    }

    if let Stmt::For(statement) = stmt {
        push_name_targets(&statement.target, out);
        recurse(&statement.body, out);
        recurse(&statement.orelse, out);
        return;
    }
    if let Stmt::While(statement) = stmt {
        recurse(&statement.body, out);
        recurse(&statement.orelse, out);
        return;
    }
    if let Stmt::If(statement) = stmt {
        recurse(&statement.body, out);
        for clause in &statement.elif_else_clauses {
            recurse(&clause.body, out);
        }
        return;
    }
    if let Stmt::With(statement) = stmt {
        for item in &statement.items {
            if let Some(vars) = &item.optional_vars {
                push_name_targets(vars, out);
            }
        }
        recurse(&statement.body, out);
        return;
    }
    if let Stmt::Try(statement) = stmt {
        recurse(&statement.body, out);
        for handler in &statement.handlers {
            let ruff_python_ast::ExceptHandler::ExceptHandler(handler) = handler;
            if let Some(name) = &handler.name {
                out.insert(name.to_string());
            }
            recurse(&handler.body, out);
        }
        recurse(&statement.orelse, out);
        recurse(&statement.finalbody, out);
        return;
    }
    if let Stmt::Match(statement) = stmt {
        for case in &statement.cases {
            collect_pattern_names(&case.pattern, out);
            recurse(&case.body, out);
        }
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
        .and_then(|kw| kw.value.string_literal().map(str::to_string))
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

    let is_gfk = if let Expr::Attribute(attr) = call.func.as_ref() {
        attr.attr.as_str() == "GenericForeignKey"
    } else {
        call.func.name_target() == Some("GenericForeignKey")
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
        .and_then(|kw| kw.value.string_literal().map(str::to_string))
    {
        return Some(value);
    }

    // Fall back to positional
    call.arguments
        .args
        .get(pos)
        .and_then(ExprExt::string_literal)
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use camino::Utf8Path;
    use djls_source::Span;
    use djls_testing::TestDatabase;

    use super::extract_models_impl;
    use super::*;
    use crate::models::graph::ModelDef;
    use crate::models::graph::ModelGraph;
    use crate::models::graph::ModelId;
    use crate::models::graph::RelatedName;
    use crate::models::resolve::resolve_local_model_graph;

    fn extract_model_facts(source: &str, module_name: &str) -> ExtractedClasses {
        let db = TestDatabase::new();
        db.add_file("/test.py", source)
            .expect("model extraction fixture should be added to the test database");
        let file = db
            .file(Utf8Path::new("/test.py"))
            .expect("model extraction fixture should exist in the test database");
        let module_name =
            PythonModuleName::parse(module_name).expect("test Python module name should be valid");
        let Ok(parsed) = ruff_python_parser::parse_module(source) else {
            return ExtractedClasses::default();
        };
        let module = parsed.into_syntax();
        extract_models_impl(&module.body, &module_name, file, ModuleKind::Module)
    }

    fn extract_model_graph(source: &str, module_name: &str) -> ModelGraph {
        resolve_local_model_graph(&extract_model_facts(source, module_name))
    }

    fn extracted_class<'a>(extraction: &'a ExtractedClasses, name: &str) -> &'a ExtractedClass {
        extraction
            .as_slice()
            .iter()
            .find(|class| class.name.value().as_str() == name)
            .expect("extracted class should exist")
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
            RelationTarget::Bare { name, .. } => Some(name.as_str()),
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
    fn plain_class_is_extracted_but_not_admitted() {
        let source = "class Foo:\n    pass\n";
        let extraction = extract_model_facts(source, "test");
        let class = extracted_class(&extraction, "Foo");

        assert_eq!(class.module_name.as_str(), "test");
        assert_eq!(class.name.span(), Span::new(6, 3));
        assert!(class.relations.is_empty());
        assert_eq!(class.declared_model_kind, ModelKind::Concrete);
        assert!(extract_model_graph(source, "test").is_empty());
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
    fn unrelated_alias_is_retained_as_a_qualified_base() {
        let source = r"
import foo

class NotAModel(foo.Model):
    pass
";
        let extraction = extract_model_facts(source, "app.models");
        let class = extracted_class(&extraction, "NotAModel");
        assert!(matches!(
            class.bases.as_slice(),
            [base] if matches!(
                base.value(),
                ExtractedBaseRef::Qualified(path) if path.as_str() == "foo.Model"
            )
        ));
    }

    #[test]
    fn imported_non_django_base_is_retained_as_a_qualified_base() {
        let source = r"
from pydantic import BaseModel

class NotDjango(BaseModel):
    pass
";
        let extraction = extract_model_facts(source, "app.models");
        let class = extracted_class(&extraction, "NotDjango");
        assert!(matches!(
            class.bases.as_slice(),
            [base] if matches!(
                base.value(),
                ExtractedBaseRef::Qualified(path) if path.as_str() == "pydantic.BaseModel"
            )
        ));
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
    fn keyword_bare_relation_target_preserves_value_span() {
        let source = r"
from django.db import models

class Order(models.Model):
    user = models.ForeignKey(to=User, on_delete=models.CASCADE)
";
        let graph = extract_model_graph(source, "shop.models");
        let relation = &model(&graph, "Order").relations[0];
        let target_start = source
            .find("to=User")
            .expect("keyword relation target should occur in the fixture")
            + "to=".len();

        assert_eq!(bare_target_name(relation), Some("User"));
        assert_eq!(
            relation.target_span(),
            Some(Span::saturating_from_parts_usize(
                target_start,
                "User".len()
            ))
        );
    }

    #[test]
    fn keyword_qualified_relation_target() {
        let source = r#"
from django.db import models

class Order(models.Model):
    user = models.ForeignKey(to="accounts.User", on_delete=models.CASCADE)
"#;
        let graph = extract_model_graph(source, "shop.models");

        assert!(matches!(
            model(&graph, "Order").relations[0].target_model(),
            Some(RelationTarget::Qualified { app_label, name })
                if app_label == "accounts" && name.as_str() == "User"
        ));
    }

    #[test]
    fn keyword_self_relation_target() {
        let source = r#"
from django.db import models

class Category(models.Model):
    parent = models.ForeignKey(to="self", on_delete=models.CASCADE)
"#;
        let graph = extract_model_graph(source, "catalog.models");

        assert!(matches!(
            model(&graph, "Category").relations[0].target_model(),
            Some(RelationTarget::SelfRef)
        ));
    }

    #[test]
    fn keyword_attribute_relation_target() {
        let source = r"
from django.db import models
import accounts.models as account_models

class Order(models.Model):
    user = models.ForeignKey(to=account_models.User, on_delete=models.CASCADE)
";
        let graph = extract_model_graph(source, "shop.models");

        assert!(matches!(
            model(&graph, "Order").relations[0].target_model(),
            Some(RelationTarget::Attribute { path, .. })
                if path == &["account_models", "User"]
        ));
    }

    #[test]
    fn positional_relation_target_wins_over_keyword() {
        let source = r"
from django.db import models

class Order(models.Model):
    preferred = models.ForeignKey(Author, to=Editor, on_delete=models.CASCADE)
    positional = models.ForeignKey(Editor, on_delete=models.CASCADE)
    missing = models.ForeignKey(on_delete=models.CASCADE)
";
        let graph = extract_model_graph(source, "shop.models");
        let order = model(&graph, "Order");

        assert_eq!(order.relations.len(), 2);
        assert_eq!(order.relations[0].field_name.value().as_str(), "preferred");
        assert_eq!(bare_target_name(&order.relations[0]), Some("Author"));
        assert_eq!(order.relations[1].field_name.value().as_str(), "positional");
        assert_eq!(bare_target_name(&order.relations[1]), Some("Editor"));
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
            RelationType::ForeignKey {
                related_name: Some(RelatedName::Named(ref name)),
                ..
            } if name == "orders"
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
        let extraction = extract_model_facts(source, "shop.models");
        let concrete = extracted_class(&extraction, "ConcreteOrder");

        assert_eq!(concrete.declared_model_kind, ModelKind::Concrete);
        assert_eq!(concrete.relations.len(), 1);
        assert_eq!(bare_target_name(&concrete.relations[0]), Some("Seller"));
        assert!(matches!(
            concrete.bases.as_slice(),
            [base] if matches!(base.value(), ExtractedBaseRef::SameModule(name) if name.as_str() == "BaseOrder")
        ));
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
        let extraction = extract_model_facts(source, "shop.models");
        let base = extracted_class(&extraction, "BaseOrder");
        let special = extracted_class(&extraction, "SpecialOrder");

        assert_eq!(base.relations.len(), 1);
        assert!(special.relations.is_empty());
        assert!(matches!(
            special.bases.as_slice(),
            [parent] if matches!(parent.value(), ExtractedBaseRef::SameModule(name) if name.as_str() == "BaseOrder")
        ));
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
        let extraction = extract_model_facts(source, "app.models");
        let document = extracted_class(&extraction, "Document");

        assert!(document.relations.is_empty());
        assert_eq!(document.bases.len(), 2);
        assert!(matches!(
            document.bases[0].value(),
            ExtractedBaseRef::SameModule(name) if name.as_str() == "TimestampMixin"
        ));
        assert!(matches!(
            document.bases[1].value(),
            ExtractedBaseRef::SameModule(name) if name.as_str() == "AuditMixin"
        ));
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
        let extraction = extract_model_facts(source, "app.models");
        let restaurant = extracted_class(&extraction, "Restaurant");

        assert_eq!(restaurant.relations.len(), 1);
        assert_eq!(restaurant.relations[0].field_name.value().as_str(), "owner");
        assert_eq!(bare_target_name(&restaurant.relations[0]), Some("User"));
    }

    #[test]
    fn unresolved_qualified_base_is_retained_without_becoming_same_module() {
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
        let extraction = extract_model_facts(source, "shop.models");
        let concrete = extracted_class(&extraction, "ConcreteOrder");

        assert!(matches!(
            concrete.bases.as_slice(),
            [base] if matches!(
                base.value(),
                ExtractedBaseRef::MissingBinding { path }
                    if path == &["some_module".to_string(), "BaseOrder".to_string()]
            )
        ));
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
        let extraction = extract_model_facts(source, "app.models");
        let middle = extracted_class(&extraction, "MiddleMixin");
        let concrete = extracted_class(&extraction, "Concrete");

        assert_eq!(middle.declared_model_kind, ModelKind::Abstract);
        assert!(middle.relations.is_empty());
        assert_eq!(concrete.declared_model_kind, ModelKind::Concrete);
        assert!(concrete.relations.is_empty());
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
        let extraction = extract_model_facts(source, "tagging.models");
        let mixin = extracted_class(&extraction, "GenericMixin");
        let tagged = extracted_class(&extraction, "TaggedItem");

        assert_eq!(mixin.relations.len(), 1);
        assert!(matches!(
            mixin.relations[0].relation_type,
            RelationType::GenericForeignKey { .. }
        ));
        assert!(tagged.relations.is_empty());
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
