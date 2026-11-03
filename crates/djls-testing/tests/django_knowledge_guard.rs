//! Guards AGENTS.md's Extraction First rule: template analysis, IDE features, and the server
//! consume Django facts from `djls-project` and must not key behavior on Django names.
//!
//! The name set comes from extracting the bundled Django releases, so it follows Django rather
//! than a hand list. Production code is scanned through each crate's module tree, skipping
//! `#[cfg(test)]` and `#[test]` items and attributes such as doc comments.
#![expect(
    clippy::expect_used,
    clippy::unwrap_used,
    reason = "unreadable bundled sources or workspace files should fail the guard loudly"
)]

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::fmt::Write as _;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_conf::DjangoVersion;
use djls_conf::TagSpecDef;
use djls_project::Project;
use djls_project::PythonEnvironment;
use djls_project::PythonModuleName;
use djls_project::PythonSourceModule;
use djls_project::SearchPaths;
use djls_project::TemplateLibraryId;
use djls_project::template_library_definition_facts;
use djls_project::template_library_tag_facts;
use djls_source::Db as _;
use djls_source::RootWalk;
use djls_source::WalkOptions;
use djls_testing::TestDatabase;
use proc_macro2::TokenStream;
use proc_macro2::TokenTree;
use serde::Deserialize;
use syn::visit::Visit;

const SCANNED_CRATES: &[&str] = &[
    "djls-semantic",
    "djls-ide",
    "djls-lsp",
    "djls-server",
    "djls-wasm",
];
const ALLOWLIST: &str = "tests/django_knowledge_allowlist.toml";

#[test]
fn production_code_holds_no_name_keyed_django_knowledge() {
    let names = bundled_django_names();
    assert!(
        ["if", "endif", "elif", "cycle", "date", "partialdef"]
            .iter()
            .all(|name| names.contains(*name)),
        "bundled Django extraction should yield tag, filter, intermediate, and closer names"
    );

    let workspace = Utf8Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut found = BTreeMap::<(String, String), Vec<usize>>::new();
    for krate in SCANNED_CRATES {
        let src = workspace.join("crates").join(krate).join("src");
        for file in production_files(&src) {
            let source = std::fs::read_to_string(&file).expect("source should be readable");
            let relative = file
                .strip_prefix(&workspace)
                .expect("scanned files live in the workspace");
            for (value, line) in string_literals(&source) {
                if names.contains(&value) || value.starts_with("django.") {
                    found
                        .entry((relative.to_string(), value))
                        .or_default()
                        .push(line);
                }
            }
        }
    }

    let allowlist = Allowlist::load(&Utf8Path::new(env!("CARGO_MANIFEST_DIR")).join(ALLOWLIST));
    let mut report = String::new();
    for ((path, value), lines) in &found {
        if !allowlist.permits(path, value) {
            for line in lines {
                writeln!(report, "  {path}:{line}: {value:?}").unwrap();
            }
        }
    }
    assert!(
        report.is_empty(),
        "Django names found in production template-analysis code:\n{report}\n\
         Extraction First (AGENTS.md): Django knowledge comes from extraction in djls-project, \
         or from a Fallback Rule in crates/djls-project/src/templates/tags/fallback.rs when \
         extraction cannot prove it. Key behavior on Tag Roles and extracted facts instead of \
         names. If the literal is not Django knowledge, add an entry with a one-line reason to \
         crates/djls-testing/{ALLOWLIST}."
    );

    let stale = allowlist
        .entries
        .iter()
        .filter(|entry| !found.contains_key(&(entry.path.clone(), entry.literal.clone())))
        .map(|entry| format!("  {} {:?}", entry.path, entry.literal))
        .collect::<Vec<_>>();
    assert!(
        stale.is_empty(),
        "Allowlist entries no longer match production code; remove them:\n{}",
        stale.join("\n")
    );
}

/// Tag, Filter, Intermediate Tag, and Closing Tag names from every bundled Django release.
fn bundled_django_names() -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for version in [
        DjangoVersion::Django52,
        DjangoVersion::Django60,
        DjangoVersion::Django61,
    ] {
        let mut db = TestDatabase::new();
        let root = Utf8PathBuf::from("/project");
        let mut search_paths = SearchPaths::from_paths(Vec::new());
        search_paths.add_bundled_django(db.file_system(), &root, Some(version));
        let site_packages = search_paths
            .iter()
            .next()
            .expect("bundled Django should be mounted")
            .path()
            .to_path_buf();
        search_paths.register_roots(&db);
        let project = Project::new(
            &db,
            root,
            search_paths,
            PythonEnvironment::Auto,
            None,
            Some(version),
            Vec::new(),
            Vec::new(),
            TagSpecDef::default(),
        );
        db.set_project(project);

        for module in library_modules(&db, &site_packages) {
            let source = PythonSourceModule::resolve(&db, project, module.clone())
                .expect("bundled library module should resolve");
            let key = TemplateLibraryId::new(&db, Some(source.file()), module);
            names.extend(
                template_library_definition_facts(&db, key)
                    .symbols()
                    .map(|symbol| symbol.name().to_string()),
            );
            for spec in template_library_tag_facts(&db, key)
                .block_specs()
                .as_map()
                .values()
            {
                names.extend(spec.end_tag.iter().cloned());
                names.extend(spec.intermediates.iter().cloned());
            }
        }
    }
    names
}

/// Django's builtin libraries and every `templatetags` package module in the bundle.
fn library_modules(db: &TestDatabase, site_packages: &Utf8Path) -> Vec<PythonModuleName> {
    let RootWalk::Directory { entries, .. } = db
        .file_system()
        .walk_root(&site_packages.join("django"), &WalkOptions::unrestricted())
    else {
        panic!("bundled Django should be a directory");
    };
    let mut modules = entries
        .iter()
        .filter_map(|entry| {
            let relative = entry.path.strip_prefix(site_packages).ok()?;
            let stem = relative.as_str().strip_suffix(".py")?;
            let builtin = matches!(
                stem,
                "django/template/defaulttags"
                    | "django/template/defaultfilters"
                    | "django/template/loader_tags"
            );
            let templatetags = relative.parent()?.file_name() == Some("templatetags")
                && relative.file_name() != Some("__init__.py");
            (builtin || templatetags).then(|| {
                PythonModuleName::parse(&stem.replace('/', "."))
                    .expect("bundled module path should be a module name")
            })
        })
        .collect::<Vec<_>>();
    modules.sort_by(|left, right| left.as_str().cmp(right.as_str()));
    modules
}

/// Files in a crate's module tree, following `mod` declarations that are not test-only.
fn production_files(src: &Utf8Path) -> Vec<Utf8PathBuf> {
    let mut files = Vec::new();
    let mut pending = vec![(src.join("lib.rs"), src.to_path_buf())];
    while let Some((file, dir)) = pending.pop() {
        let source = std::fs::read_to_string(&file).expect("module file should be readable");
        let syntax = syn::parse_file(&source).expect("module file should parse");
        let mut modules = ModuleCollector {
            dir: vec![dir],
            found: Vec::new(),
        };
        modules.visit_file(&syntax);
        pending.extend(modules.found);
        files.push(file);
    }
    files.sort();
    files
}

struct ModuleCollector {
    dir: Vec<Utf8PathBuf>,
    found: Vec<(Utf8PathBuf, Utf8PathBuf)>,
}

impl<'ast> Visit<'ast> for ModuleCollector {
    fn visit_item_mod(&mut self, item: &'ast syn::ItemMod) {
        if is_test_only(&item.attrs) {
            return;
        }
        let dir = self
            .dir
            .last()
            .expect("module stack is never empty")
            .clone();
        let child_dir = dir.join(item.ident.to_string());
        if item.content.is_some() {
            self.dir.push(child_dir);
            syn::visit::visit_item_mod(self, item);
            self.dir.pop();
        } else {
            let file = dir.join(format!("{}.rs", item.ident));
            self.found.push((file, child_dir));
        }
    }
}

/// String literal values and their lines in production items of one file.
fn string_literals(source: &str) -> Vec<(String, usize)> {
    let syntax = syn::parse_file(source).expect("module file should parse");
    let mut collector = LiteralCollector::default();
    collector.visit_file(&syntax);
    collector.literals
}

#[derive(Default)]
struct LiteralCollector {
    literals: Vec<(String, usize)>,
}

impl LiteralCollector {
    fn collect_tokens(&mut self, tokens: TokenStream) {
        for token in tokens {
            if let TokenTree::Group(group) = &token {
                self.collect_tokens(group.stream());
            } else if let TokenTree::Literal(literal) = token
                && let syn::Lit::Str(value) = syn::Lit::new(literal)
            {
                self.literals
                    .push((value.value(), value.span().start().line));
            }
        }
    }
}

impl<'ast> Visit<'ast> for LiteralCollector {
    // Doc comments, lint reasons, and serde names are not behavior.
    fn visit_attribute(&mut self, _attribute: &'ast syn::Attribute) {}

    // Nested files are scanned on their own.
    fn visit_item_mod(&mut self, item: &'ast syn::ItemMod) {
        if !is_test_only(&item.attrs) {
            syn::visit::visit_item_mod(self, item);
        }
    }

    fn visit_item_fn(&mut self, item: &'ast syn::ItemFn) {
        if !is_test_only(&item.attrs) {
            syn::visit::visit_item_fn(self, item);
        }
    }

    fn visit_item_impl(&mut self, item: &'ast syn::ItemImpl) {
        if !is_test_only(&item.attrs) {
            syn::visit::visit_item_impl(self, item);
        }
    }

    fn visit_impl_item_fn(&mut self, item: &'ast syn::ImplItemFn) {
        if !is_test_only(&item.attrs) {
            syn::visit::visit_impl_item_fn(self, item);
        }
    }

    fn visit_item_const(&mut self, item: &'ast syn::ItemConst) {
        if !is_test_only(&item.attrs) {
            syn::visit::visit_item_const(self, item);
        }
    }

    fn visit_item_static(&mut self, item: &'ast syn::ItemStatic) {
        if !is_test_only(&item.attrs) {
            syn::visit::visit_item_static(self, item);
        }
    }

    fn visit_lit_str(&mut self, literal: &'ast syn::LitStr) {
        self.literals
            .push((literal.value(), literal.span().start().line));
    }

    // Macro arguments stay unparsed tokens, so read their literals directly.
    fn visit_macro(&mut self, mac: &'ast syn::Macro) {
        self.collect_tokens(mac.tokens.clone());
    }
}

fn is_test_only(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|attr| {
        attr.path().is_ident("test")
            || (attr.path().is_ident("cfg")
                && attr.meta.require_list().is_ok_and(|list| {
                    let condition = list.tokens.to_string();
                    condition
                        .split(|c: char| !c.is_alphanumeric() && c != '_')
                        .any(|word| word == "test")
                        && !condition.contains("not")
                }))
    })
}

#[derive(Deserialize)]
struct Allowlist {
    #[serde(rename = "allow", default)]
    entries: Vec<AllowEntry>,
}

#[derive(Deserialize)]
struct AllowEntry {
    path: String,
    literal: String,
    reason: String,
}

impl Allowlist {
    fn load(path: &Utf8Path) -> Self {
        let source = std::fs::read_to_string(path).expect("allowlist should be readable");
        let allowlist: Self = toml::from_str(&source).expect("allowlist should be valid TOML");
        for entry in &allowlist.entries {
            assert!(
                !entry.reason.trim().is_empty() && !entry.reason.contains('\n'),
                "allowlist entry {} {:?} needs a one-line reason",
                entry.path,
                entry.literal
            );
        }
        allowlist
    }

    fn permits(&self, path: &str, literal: &str) -> bool {
        self.entries
            .iter()
            .any(|entry| entry.path == path && entry.literal == literal)
    }
}
