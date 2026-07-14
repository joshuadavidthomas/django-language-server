mod rules;
mod specs;

use djls_project::ContextualLibraryStep;
use djls_project::Project;
use djls_project::TemplateLibraryKey;
use djls_project::TemplateSymbolKind;
use djls_project::extract_block_specs;
use djls_project::extract_tag_rules;
use djls_project::template_environment;
pub(crate) use rules::evaluate_tag_rules;
pub use specs::EndTag;
pub use specs::IntermediateTag;
pub use specs::TagSpec;
pub use specs::TagSpecs;
pub use specs::builtin_tag_specs;

use crate::db::Db;
use crate::references::TemplateReferenceKind;
use crate::scoping::LoadState;

/// Durable Django template meaning for a tag.
///
/// This describes what the tag does in the template domain. Feature-specific
/// projections, such as document symbols, map these roles into their own shapes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TagRole {
    TemplateReference(TemplateReferenceKind),
    TemplateLibraryLoader,
    TemplateBlock,
    TemplatePartial,
    ControlTag,
    TemplateTag,
    StaticAssetReference,
    RouteReference,
}

/// Independently backdatable semantic Tag facts for one Template Library.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LibraryTagSpecs(TagSpecs);

impl LibraryTagSpecs {
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&TagSpec> {
        self.0.get(name)
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = (&String, &TagSpec)> {
        self.0.iter()
    }
}

/// Fuse builtin/manual fallback meaning with one library's extracted Tag facts.
#[salsa::tracked(returns(ref))]
#[allow(clippy::needless_pass_by_value)]
pub fn library_tag_specs(
    db: &dyn Db,
    project: Project,
    key: TemplateLibraryKey,
) -> LibraryTagSpecs {
    let mut specs = builtin_tag_specs();
    specs.retain(|_, spec| spec.module() == key.module(db).as_str());

    let rules = extract_tag_rules(db, key);
    if !rules.is_empty() {
        specs.merge_tag_rules(rules);
    }
    let blocks = extract_block_specs(db, key);
    if !blocks.is_empty() {
        specs.merge_block_specs(blocks);
    }

    specs.merge_fallback(configured_library_tag_specs(db, project, key).clone());
    LibraryTagSpecs(specs)
}

/// Equality-bearing configured fallback for one Template Library.
#[salsa::tracked(returns(ref))]
fn configured_library_tag_specs(
    db: &dyn Db,
    project: Project,
    key: TemplateLibraryKey,
) -> TagSpecs {
    project
        .tagspecs(db)
        .libraries
        .iter()
        .filter(|library| library.module == key.module(db).as_str())
        .map(TagSpecs::from_tagspec_library)
        .fold(TagSpecs::default(), |mut specs, configured| {
            specs.merge(configured);
            specs
        })
}

/// Return the effective tag spec at one occurrence, but only when every feasible backend agrees.
pub(crate) fn effective_tag_spec(
    db: &dyn Db,
    file: djls_source::File,
    name: &str,
    load_state: &LoadState<'_>,
) -> Option<TagSpec> {
    let Some(project) = db.project() else {
        return db.projectless_tag_specs().get(name).cloned();
    };
    effective_tag_spec_in_project(db, project, file, name, load_state)
}

pub(crate) fn effective_tag_spec_in_project_for_scope(
    db: &dyn Db,
    project: Project,
    scope_file: djls_source::File,
    name: &str,
    load_state: &LoadState<'_>,
) -> Option<TagSpec> {
    let loaded = load_state.libraries_loading_symbol(name);
    effective_tag_spec_from_environment(
        db,
        project,
        template_environment(db, project, scope_file),
        name,
        &loaded,
    )
}

pub(crate) fn effective_tag_spec_in_project(
    db: &dyn Db,
    project: Project,
    file: djls_source::File,
    name: &str,
    load_state: &LoadState<'_>,
) -> Option<TagSpec> {
    let loaded = load_state.libraries_loading_symbol(name);
    effective_tag_spec_from_environment(
        db,
        project,
        template_environment(db, project, file),
        name,
        &loaded,
    )
}

fn effective_tag_spec_from_environment(
    db: &dyn Db,
    project: Project,
    environment: djls_project::TemplateEnvironment<'_>,
    name: &str,
    loaded: &[&str],
) -> Option<TagSpec> {
    let definitions: Option<Vec<Option<TagSpec>>> = environment
        .contextual_library_chains(loaded)
        .into_iter()
        .map(|chain| {
            let mut effective = None;
            let mut unknown = false;
            for step in chain.steps() {
                let ContextualLibraryStep::Library(library) = step else {
                    unknown = true;
                    continue;
                };
                if let Some(spec) = library_tag_specs(db, project, library.key(db)).get(name) {
                    effective = Some(spec.clone());
                    unknown = false;
                } else if library.symbol(TemplateSymbolKind::Tag, name).is_some() {
                    effective = None;
                    unknown = false;
                } else if library.symbol_inventory_is_open()
                    && !hardcoded_tag_inventory_is_complete(library.module_name_str())
                {
                    unknown = true;
                }
            }
            (!unknown).then_some(effective)
        })
        .collect();
    let definitions = definitions?;
    let first = definitions.first()?.as_ref()?;
    definitions
        .iter()
        .all(|definition| definition.as_ref() == Some(first))
        .then(|| first.clone())
}

/// Specs effective before any file-local `{% load %}` statement.
#[salsa::tracked(returns(ref))]
pub fn tag_specs_for_file(db: &dyn Db, file: djls_source::File) -> TagSpecs {
    let empty = crate::scoping::LoadedLibraries::default();
    effective_tag_specs_for_load_state(db, file, &empty.available_at(0))
}

#[salsa::tracked(returns(ref))]
pub fn tag_specs_at(
    db: &dyn Db,
    file: djls_source::File,
    nodelist: djls_templates::NodeList<'_>,
    position: u32,
) -> TagSpecs {
    let loaded = crate::scoping::compute_loaded_libraries_for_file(db, file, nodelist);
    effective_tag_specs_for_load_state(db, file, &loaded.available_at(position))
}

pub(crate) fn effective_tag_specs_for_load_state_in_project_scope(
    db: &dyn Db,
    project: Project,
    scope_file: djls_source::File,
    load_state: &LoadState<'_>,
) -> TagSpecs {
    let environment = template_environment(db, project, scope_file);
    let names = semantic_tag_candidate_names(db, project, environment);

    let mut specs = TagSpecs::default();
    for name in names {
        if let Some(spec) =
            effective_tag_spec_in_project_for_scope(db, project, scope_file, &name, load_state)
        {
            specs.insert(name, spec);
        }
    }
    specs
}

pub(crate) fn effective_tag_specs_for_load_state(
    db: &dyn Db,
    file: djls_source::File,
    load_state: &LoadState<'_>,
) -> TagSpecs {
    let names = if let Some(project) = db.project() {
        semantic_tag_candidate_names(db, project, template_environment(db, project, file))
    } else {
        db.projectless_tag_specs().keys().cloned().collect()
    };

    let mut specs = TagSpecs::default();
    for name in names {
        if let Some(spec) = effective_tag_spec(db, file, &name, load_state) {
            specs.insert(name, spec);
        }
    }
    specs
}

fn hardcoded_tag_inventory_is_complete(module: &str) -> bool {
    matches!(
        module,
        "django.template.defaulttags"
            | "django.template.defaultfilters"
            | "django.template.loader_tags"
    )
}

fn semantic_tag_candidate_names(
    db: &dyn Db,
    project: Project,
    environment: djls_project::TemplateEnvironment<'_>,
) -> std::collections::HashSet<String> {
    let mut names: std::collections::HashSet<_> = environment
        .inventory_symbol_names(TemplateSymbolKind::Tag)
        .map(str::to_owned)
        .collect();
    for library in environment.resolved_libraries() {
        names.extend(
            library_tag_specs(db, project, library.key(db))
                .iter()
                .map(|(name, _spec)| name.clone()),
        );
    }
    names
}
