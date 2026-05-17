//! Static Django template symbol facts.
//!
//! This module resolves assembled template libraries to Python files and uses
//! the existing Ruff-based registration extraction to produce static tag/filter
//! symbol facts. It does not wire the facts into validators yet.
//!
//! The first slice records the registration module as the symbol definition
//! module and leaves docstrings empty. It also detects only syntactic
//! registrations against the module-level `register` object; runtime-only
//! conditional registration remains a partial-static-model limitation.

#![allow(
    dead_code,
    reason = "Milestone A9 adds template symbol facts before project facts are assembled."
)]

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::fs;

use camino::Utf8Path;

use crate::project::facts::Fact;
use crate::project::facts::Field;
use crate::project::facts::ModuleSearchPathEntry;
use crate::project::facts::Reason;
use crate::project::facts::ResolvedModule;
use crate::project::facts::TemplateLibraryFact;
use crate::project::facts::TemplateLibrarySource;
use crate::project::facts::TemplateSymbolFact;
use crate::project::module_resolver::resolve_module;
use crate::project::names::LibraryName;
use crate::project::names::PyModuleName;
use crate::project::names::TemplateSymbolName;
use crate::project::symbols::TemplateLibrarySnapshot;
use crate::project::symbols::TemplateSymbolKind;
use crate::project::symbols::TemplateSymbolSnapshot;
use crate::python::SymbolKind;

#[must_use]
pub(crate) fn assemble_template_symbols(
    template_libraries: &Fact<Vec<TemplateLibraryFact>>,
    module_search_paths: &Fact<Vec<ModuleSearchPathEntry>>,
    project_root: &Utf8Path,
) -> Fact<Vec<TemplateSymbolFact>> {
    let (libraries, mut reasons) = match fact_value_with_reasons(template_libraries) {
        Ok((libraries, reasons)) => (libraries, reasons),
        Err(reasons) => return Fact::unknown(reasons),
    };

    if libraries.is_empty() {
        return known_or_partial(Vec::new(), reasons);
    }

    let search_paths = match fact_value_with_reasons(module_search_paths) {
        Ok((search_paths, search_reasons)) => {
            extend_unique_reasons(&mut reasons, search_reasons);
            search_paths
        }
        Err(search_reasons) => {
            extend_unique_reasons(&mut reasons, search_reasons);
            return Fact::unknown(reasons);
        }
    };

    let mut symbols = Vec::new();
    for library in libraries {
        extract_library_symbols(
            library,
            search_paths,
            project_root,
            &mut symbols,
            &mut reasons,
        );
    }

    symbols.sort_by(|a, b| {
        a.library
            .cmp(&b.library)
            .then_with(|| a.module.cmp(&b.module))
            .then_with(|| a.kind.cmp(&b.kind))
            .then_with(|| a.name.cmp(&b.name))
    });
    symbols.dedup();

    known_or_partial(symbols, reasons)
}

#[must_use]
pub(crate) fn assemble_template_library_snapshot(
    template_libraries: &Fact<Vec<TemplateLibraryFact>>,
    template_symbols: &Fact<Vec<TemplateSymbolFact>>,
) -> Fact<TemplateLibrarySnapshot> {
    let (libraries, mut reasons) = match fact_value_with_reasons(template_libraries) {
        Ok((libraries, reasons)) => (libraries, reasons),
        Err(reasons) => return Fact::unknown(reasons),
    };

    let active_loadable = active_loadable_modules(libraries);
    let builtin_modules = builtin_modules(libraries);
    let mut snapshot = TemplateLibrarySnapshot {
        symbols: Vec::new(),
        libraries: active_loadable
            .iter()
            .map(|(load_name, module)| {
                (load_name.as_str().to_string(), module.as_str().to_string())
            })
            .collect(),
        builtins: builtin_module_names(libraries),
    };

    match template_symbols {
        Fact::Known { value } => {
            snapshot.symbols = symbol_snapshots(value, &active_loadable, &builtin_modules);
        }
        Fact::Partial {
            value,
            reasons: symbol_reasons,
        } => {
            snapshot.symbols = symbol_snapshots(value, &active_loadable, &builtin_modules);
            extend_unique_reasons(&mut reasons, symbol_reasons.iter().cloned());
        }
        Fact::Unknown {
            reasons: symbol_reasons,
        }
        | Fact::Ambiguous {
            reasons: symbol_reasons,
            ..
        } => {
            extend_unique_reasons(&mut reasons, symbol_reasons.iter().cloned());
        }
    }

    known_or_partial(snapshot, reasons)
}

fn extract_library_symbols(
    library: &TemplateLibraryFact,
    search_paths: &[ModuleSearchPathEntry],
    project_root: &Utf8Path,
    symbols: &mut Vec<TemplateSymbolFact>,
    reasons: &mut Vec<Reason>,
) {
    let resolution = resolve_module(library.module.clone(), search_paths, project_root);
    match resolution.resolved {
        Fact::Known { value } => {
            read_and_extract_library_symbols(library, &value, symbols, reasons);
        }
        Fact::Partial {
            value,
            reasons: resolution_reasons,
        } => {
            extend_unique_reasons(reasons, resolution_reasons);
            read_and_extract_library_symbols(library, &value, symbols, reasons);
        }
        Fact::Unknown {
            reasons: resolution_reasons,
        }
        | Fact::Ambiguous {
            reasons: resolution_reasons,
            ..
        } => {
            extend_unique_reasons(reasons, resolution_reasons);
        }
    }
}

fn read_and_extract_library_symbols(
    library: &TemplateLibraryFact,
    resolved: &ResolvedModule,
    symbols: &mut Vec<TemplateSymbolFact>,
    reasons: &mut Vec<Reason>,
) {
    let source = match fs::read_to_string(resolved.file.as_std_path()) {
        Ok(source) => source,
        Err(error) => {
            reasons.push(Reason::file(
                Field::TemplateSymbols,
                &resolved.file,
                format!("failed to read template library module: {error}"),
            ));
            return;
        }
    };

    let registrations = match crate::python::extract_template_registrations(&source) {
        Ok(registrations) => registrations,
        Err(error) => {
            reasons.push(Reason::file(
                Field::TemplateSymbols,
                &resolved.file,
                format!("failed to parse template library module: {error}"),
            ));
            return;
        }
    };

    for registration in registrations {
        let name = match TemplateSymbolName::parse(&registration.name) {
            Ok(name) => name,
            Err(error) => {
                reasons.push(Reason::file(
                    Field::TemplateSymbols,
                    &resolved.file,
                    format!(
                        "template symbol `{}` has an invalid name: {error}",
                        registration.name
                    ),
                ));
                continue;
            }
        };

        symbols.push(TemplateSymbolFact {
            library: library.load_name.clone(),
            module: library.module.clone(),
            kind: template_symbol_kind(registration.kind),
            name,
        });
    }
}

fn symbol_snapshots(
    symbols: &[TemplateSymbolFact],
    active_loadable: &BTreeMap<LibraryName, PyModuleName>,
    builtin_modules: &BTreeSet<PyModuleName>,
) -> Vec<TemplateSymbolSnapshot> {
    let mut snapshots = symbols
        .iter()
        .filter_map(|symbol| {
            let load_name = if builtin_modules.contains(&symbol.module) {
                None
            } else if active_loadable.get(&symbol.library) == Some(&symbol.module) {
                Some(symbol.library.as_str().to_string())
            } else {
                return None;
            };

            Some(TemplateSymbolSnapshot {
                kind: Some(symbol.kind),
                name: symbol.name.as_str().to_string(),
                load_name,
                library_module: symbol.module.as_str().to_string(),
                module: symbol.module.as_str().to_string(),
                doc: None,
            })
        })
        .collect::<Vec<_>>();

    snapshots.sort_by(|a, b| {
        a.load_name
            .cmp(&b.load_name)
            .then_with(|| a.library_module.cmp(&b.library_module))
            .then_with(|| symbol_snapshot_kind_rank(a.kind).cmp(&symbol_snapshot_kind_rank(b.kind)))
            .then_with(|| a.name.cmp(&b.name))
    });
    snapshots
}

fn template_symbol_kind(kind: SymbolKind) -> TemplateSymbolKind {
    match kind {
        SymbolKind::Tag => TemplateSymbolKind::Tag,
        SymbolKind::Filter => TemplateSymbolKind::Filter,
    }
}

fn symbol_snapshot_kind_rank(kind: Option<TemplateSymbolKind>) -> u8 {
    match kind {
        Some(TemplateSymbolKind::Tag) => 0,
        Some(TemplateSymbolKind::Filter) => 1,
        None => 2,
    }
}

fn active_loadable_modules(
    libraries: &[TemplateLibraryFact],
) -> BTreeMap<LibraryName, PyModuleName> {
    let mut active = BTreeMap::new();
    for library in libraries {
        if !is_builtin_library(&library.source) {
            active.insert(library.load_name.clone(), library.module.clone());
        }
    }
    active
}

fn builtin_modules(libraries: &[TemplateLibraryFact]) -> BTreeSet<PyModuleName> {
    libraries
        .iter()
        .filter(|library| is_builtin_library(&library.source))
        .map(|library| library.module.clone())
        .collect()
}

fn builtin_module_names(libraries: &[TemplateLibraryFact]) -> Vec<String> {
    let mut modules = Vec::new();
    for library in libraries {
        if is_builtin_library(&library.source)
            && !modules
                .iter()
                .any(|module| module == library.module.as_str())
        {
            modules.push(library.module.as_str().to_string());
        }
    }
    modules
}

fn is_builtin_library(source: &TemplateLibrarySource) -> bool {
    match source {
        TemplateLibrarySource::DjangoDefaultBuiltin | TemplateLibrarySource::SettingsBuiltins => {
            true
        }
        TemplateLibrarySource::AppTemplateTags { .. }
        | TemplateLibrarySource::SettingsLibraries
        | TemplateLibrarySource::DjangoDefaultLibrary
        | TemplateLibrarySource::Discovered
        | TemplateLibrarySource::UserOverride => false,
    }
}

fn fact_value_with_reasons<T>(fact: &Fact<T>) -> Result<(&T, Vec<Reason>), Vec<Reason>> {
    match fact {
        Fact::Known { value } => Ok((value, Vec::new())),
        Fact::Partial { value, reasons } => Ok((value, reasons.clone())),
        Fact::Unknown { reasons } | Fact::Ambiguous { reasons, .. } => Err(reasons.clone()),
    }
}

fn known_or_partial<T>(value: T, reasons: Vec<Reason>) -> Fact<T> {
    if reasons.is_empty() {
        Fact::known(value)
    } else {
        Fact::partial(value, reasons)
    }
}

fn extend_unique_reasons(reasons: &mut Vec<Reason>, new_reasons: impl IntoIterator<Item = Reason>) {
    for reason in new_reasons {
        if !reasons.contains(&reason) {
            reasons.push(reason);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use camino::Utf8PathBuf;
    use tempfile::tempdir;

    use super::*;
    use crate::project::facts::ModuleSearchPathKind;
    use crate::project::facts::ReasonSource;
    use crate::project::names::LibraryName;
    use crate::project::names::PyModuleName;

    fn module(name: &str) -> PyModuleName {
        PyModuleName::parse(name).unwrap()
    }

    fn library(name: &str) -> LibraryName {
        LibraryName::parse(name).unwrap()
    }

    fn search_path(root: &Utf8Path) -> ModuleSearchPathEntry {
        ModuleSearchPathEntry {
            kind: ModuleSearchPathKind::Workspace,
            path: root.to_path_buf(),
        }
    }

    fn write_file(path: &Utf8Path, contents: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, contents).unwrap();
    }

    fn loadable_library(load_name: &str, module_name: &str) -> TemplateLibraryFact {
        TemplateLibraryFact {
            load_name: library(load_name),
            module: module(module_name),
            source: TemplateLibrarySource::AppTemplateTags {
                app: module("blog"),
            },
        }
    }

    fn builtin_library(module_name: &str) -> TemplateLibraryFact {
        TemplateLibraryFact {
            load_name: library(module_name.split('.').next_back().unwrap()),
            module: module(module_name),
            source: TemplateLibrarySource::DjangoDefaultBuiltin,
        }
    }

    fn known_symbols(fact: &Fact<Vec<TemplateSymbolFact>>) -> &[TemplateSymbolFact] {
        match fact {
            Fact::Known { value } | Fact::Partial { value, .. } => value,
            Fact::Unknown { reasons } => panic!("expected symbols, got unknown: {reasons:?}"),
            Fact::Ambiguous {
                candidates,
                reasons,
            } => panic!("expected symbols, got ambiguous: {candidates:?} {reasons:?}"),
        }
    }

    fn partial_reasons<T>(fact: &Fact<T>) -> &[Reason] {
        match fact {
            Fact::Known { .. } => panic!("expected partial or unknown fact"),
            Fact::Partial { reasons, .. }
            | Fact::Unknown { reasons }
            | Fact::Ambiguous { reasons, .. } => reasons,
        }
    }

    #[test]
    fn extracts_symbols_from_loadable_and_builtin_modules() {
        let tmp = tempdir().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        write_file(
            &root.join("django/template/defaulttags.py"),
            r#"
from django import template
register = template.Library()

@register.tag("if")
def do_if(parser, token):
    pass

@register.filter
def lower(value):
    pass
"#,
        );
        write_file(
            &root.join("blog/templatetags/blog_tags.py"),
            r#"
from django import template
register = template.Library()

@register.simple_tag(name="greet")
def greet_tag():
    pass

register.filter("shout", shout)
"#,
        );

        let libraries = vec![
            builtin_library("django.template.defaulttags"),
            loadable_library("blog_tags", "blog.templatetags.blog_tags"),
        ];
        let facts = assemble_template_symbols(
            &Fact::known(libraries),
            &Fact::known(vec![search_path(&root)]),
            &root,
        );

        let symbols = known_symbols(&facts);
        assert!(symbols.iter().any(|symbol| {
            symbol.library == library("defaulttags")
                && symbol.kind == TemplateSymbolKind::Tag
                && symbol.name.as_str() == "if"
        }));
        assert!(symbols.iter().any(|symbol| {
            symbol.library == library("defaulttags")
                && symbol.kind == TemplateSymbolKind::Filter
                && symbol.name.as_str() == "lower"
        }));
        assert!(symbols.iter().any(|symbol| {
            symbol.library == library("blog_tags")
                && symbol.kind == TemplateSymbolKind::Tag
                && symbol.name.as_str() == "greet"
        }));
        assert!(symbols.iter().any(|symbol| {
            symbol.library == library("blog_tags")
                && symbol.kind == TemplateSymbolKind::Filter
                && symbol.name.as_str() == "shout"
        }));
    }

    #[test]
    fn unresolved_library_modules_are_partial() {
        let tmp = tempdir().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();

        let facts = assemble_template_symbols(
            &Fact::known(vec![loadable_library(
                "missing",
                "blog.templatetags.missing",
            )]),
            &Fact::known(vec![search_path(&root)]),
            &root,
        );

        assert!(known_symbols(&facts).is_empty());
        assert!(partial_reasons(&facts)
            .iter()
            .any(|reason| reason.field == Field::ResolverModule));
    }

    #[test]
    fn invalid_symbol_names_are_partial() {
        let tmp = tempdir().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        write_file(
            &root.join("blog/templatetags/blog_tags.py"),
            r#"
from django import template
register = template.Library()

@register.tag("bad name")
def bad(parser, token):
    pass
"#,
        );

        let facts = assemble_template_symbols(
            &Fact::known(vec![loadable_library(
                "blog_tags",
                "blog.templatetags.blog_tags",
            )]),
            &Fact::known(vec![search_path(&root)]),
            &root,
        );

        assert!(known_symbols(&facts).is_empty());
        assert!(partial_reasons(&facts)
            .iter()
            .any(|reason| reason.field == Field::TemplateSymbols));
    }

    #[test]
    fn unknown_module_search_paths_make_symbols_unknown_when_libraries_exist() {
        let reason = Reason::new(
            Field::ResolverModuleSearchPaths,
            ReasonSource::Unknown,
            "module search paths were not discovered",
        );

        let facts = assemble_template_symbols(
            &Fact::known(vec![loadable_library(
                "blog_tags",
                "blog.templatetags.blog_tags",
            )]),
            &Fact::unknown(vec![reason.clone()]),
            Utf8Path::new("/workspace"),
        );

        assert!(matches!(facts, Fact::Unknown { .. }));
        assert_eq!(facts.reasons(), &[reason]);
    }

    #[test]
    fn assembles_snapshot_from_library_and_symbol_facts() {
        let libraries = vec![
            builtin_library("django.template.defaulttags"),
            loadable_library("blog_tags", "blog.templatetags.blog_tags"),
        ];
        let symbols = vec![
            TemplateSymbolFact {
                library: library("defaulttags"),
                module: module("django.template.defaulttags"),
                kind: TemplateSymbolKind::Tag,
                name: TemplateSymbolName::parse("if").unwrap(),
            },
            TemplateSymbolFact {
                library: library("blog_tags"),
                module: module("blog.templatetags.blog_tags"),
                kind: TemplateSymbolKind::Filter,
                name: TemplateSymbolName::parse("shout").unwrap(),
            },
        ];

        let snapshot =
            assemble_template_library_snapshot(&Fact::known(libraries), &Fact::known(symbols));

        let Fact::Known { value } = snapshot else {
            panic!("expected known snapshot");
        };
        assert_eq!(
            value.libraries,
            BTreeMap::from([(
                "blog_tags".to_string(),
                "blog.templatetags.blog_tags".to_string()
            )])
        );
        assert_eq!(value.builtins, ["django.template.defaulttags"]);
        assert!(value.symbols.iter().any(|symbol| {
            symbol.name == "if"
                && symbol.load_name.is_none()
                && symbol.library_module == "django.template.defaulttags"
                && symbol.module == "django.template.defaulttags"
        }));
        assert!(value.symbols.iter().any(|symbol| {
            symbol.name == "shout"
                && symbol.load_name.as_deref() == Some("blog_tags")
                && symbol.library_module == "blog.templatetags.blog_tags"
                && symbol.module == "blog.templatetags.blog_tags"
        }));
    }

    #[test]
    fn snapshot_skips_shadowed_loadable_symbols() {
        let libraries = vec![
            loadable_library("foo", "app_a.templatetags.foo"),
            loadable_library("foo", "app_b.templatetags.foo"),
        ];
        let symbols = vec![
            TemplateSymbolFact {
                library: library("foo"),
                module: module("app_a.templatetags.foo"),
                kind: TemplateSymbolKind::Tag,
                name: TemplateSymbolName::parse("a_only").unwrap(),
            },
            TemplateSymbolFact {
                library: library("foo"),
                module: module("app_b.templatetags.foo"),
                kind: TemplateSymbolKind::Tag,
                name: TemplateSymbolName::parse("b_only").unwrap(),
            },
        ];

        let snapshot =
            assemble_template_library_snapshot(&Fact::known(libraries), &Fact::known(symbols));

        let Fact::Known { value } = snapshot else {
            panic!("expected known snapshot");
        };
        assert_eq!(
            value.libraries,
            BTreeMap::from([("foo".to_string(), "app_b.templatetags.foo".to_string())])
        );
        assert!(!value.symbols.iter().any(|symbol| symbol.name == "a_only"));
        assert!(value.symbols.iter().any(|symbol| {
            symbol.name == "b_only"
                && symbol.load_name.as_deref() == Some("foo")
                && symbol.library_module == "app_b.templatetags.foo"
        }));
    }

    #[test]
    fn snapshot_preserves_symbol_reasons_as_partial() {
        let reason = Reason::file(
            Field::TemplateSymbols,
            "blog/templatetags/blog_tags.py",
            "failed to parse template library module",
        );

        let snapshot = assemble_template_library_snapshot(
            &Fact::known(vec![loadable_library(
                "blog_tags",
                "blog.templatetags.blog_tags",
            )]),
            &Fact::partial(Vec::new(), vec![reason.clone()]),
        );

        let Fact::Partial { value, reasons } = snapshot else {
            panic!("expected partial snapshot");
        };
        assert_eq!(
            value.libraries,
            BTreeMap::from([(
                "blog_tags".to_string(),
                "blog.templatetags.blog_tags".to_string()
            )])
        );
        assert_eq!(reasons, [reason]);
    }
}
