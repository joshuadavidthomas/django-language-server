mod rules;
mod specs;

use djls_project::BlockSpecs;
use djls_project::EffectiveDefinitionLibrary;
use djls_project::Project;
use djls_project::TagRuleMap;
use djls_project::TemplateSymbolKind;
use djls_project::extract_block_specs;
use djls_project::extract_tag_rules;
use djls_project::template_environment;
use djls_project::template_libraries;
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

/// Compute `TagSpecs` from tag-rule and block-spec extraction results.
///
/// This tracked function reads only the extraction domains needed to build tag
/// specs. Filter-only extraction changes should not invalidate this query.
///
/// Does NOT read from `Arc<Mutex<Settings>>`.
#[salsa::tracked(returns(ref))]
pub fn compute_tag_specs(db: &dyn Db, project: Project) -> TagSpecs {
    let tagspecs = project.tagspecs(db);

    let mut specs = builtin_tag_specs();

    for library in template_libraries(db, project).resolved_libraries() {
        let block_specs = extract_block_specs(db, library.file(), library.module_name().clone());
        if !block_specs.is_empty() {
            specs.merge_block_specs(block_specs);
        }

        let tag_rules = extract_tag_rules(db, library.file(), library.module_name().clone());
        if !tag_rules.is_empty() {
            specs.merge_tag_rules(tag_rules);
        }
    }

    if !tagspecs.libraries.is_empty() {
        let fallback = TagSpecs::from_tagspec_def(tagspecs);
        specs.merge_fallback(fallback);
    }

    specs
}

fn spec_from_library(
    db: &dyn Db,
    library: &djls_project::TemplateLibrary,
    name: &str,
) -> Option<TagSpec> {
    let mut specs = db.tag_specs().clone();
    specs.retain(|_, spec| spec.module() == library.module_name_str());

    let rules = extract_tag_rules(db, library.file(), library.module_name().clone());
    if let Some((key, rule)) = rules.iter().find(|(key, _)| key.name == name) {
        let mut selected = TagRuleMap::default();
        selected.insert(key.clone(), rule.clone());
        specs.merge_tag_rules(&selected);
    }
    let blocks = extract_block_specs(db, library.file(), library.module_name().clone());
    if let Some((key, block)) = blocks.as_map().iter().find(|(key, _)| key.name == name) {
        let mut selected = BlockSpecs::default();
        selected.insert(key.clone(), block.clone());
        specs.merge_block_specs(&selected);
    }
    specs.get(name).cloned()
}

/// Return the effective tag spec at one occurrence, but only when every feasible backend agrees.
pub(crate) fn effective_tag_spec(
    db: &dyn Db,
    file: djls_source::File,
    name: &str,
    load_state: &LoadState<'_>,
) -> Option<TagSpec> {
    let Some(project) = db.project() else {
        return db.tag_specs().get(name).cloned();
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
    // A manually configured spec is explicit fallback evidence for extraction gaps. Builtin specs
    // alone are not: backend uncertainty must not silently promote them to effective definitions.
    let configured_fallback = project
        .tagspecs(db)
        .libraries
        .iter()
        .any(|library| library.tags.iter().any(|tag| tag.name == name))
        .then(|| db.tag_specs().get(name).cloned())
        .flatten();
    let alternatives =
        environment.effective_definition_libraries(db, name, TemplateSymbolKind::Tag, loaded);
    if alternatives.is_empty() {
        return db.tag_specs().get(name).cloned();
    }

    let mut has_gap = false;
    let mut definitions = Vec::new();
    for alternative in alternatives {
        match alternative {
            EffectiveDefinitionLibrary::Known(library) => {
                definitions.push(library.and_then(|library| spec_from_library(db, library, name)));
            }
            EffectiveDefinitionLibrary::Unknown => has_gap = true,
        }
    }

    let Some(first) = definitions.iter().find_map(Option::as_ref) else {
        return configured_fallback;
    };
    if definitions
        .iter()
        .filter_map(Option::as_ref)
        .any(|definition| definition != first)
    {
        return None;
    }
    if has_gap || definitions.iter().any(Option::is_none) {
        return (configured_fallback.as_ref() == Some(first)).then(|| first.clone());
    }
    Some(first.clone())
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
    let mut specs = TagSpecs::default();
    for name in db.tag_specs().keys() {
        if let Some(spec) =
            effective_tag_spec_in_project_for_scope(db, project, scope_file, name, load_state)
        {
            specs.insert(name.clone(), spec);
        }
    }
    specs
}

pub(crate) fn effective_tag_specs_for_load_state(
    db: &dyn Db,
    file: djls_source::File,
    load_state: &LoadState<'_>,
) -> TagSpecs {
    let mut specs = TagSpecs::default();
    for name in db.tag_specs().keys() {
        if let Some(spec) = effective_tag_spec(db, file, name, load_state) {
            specs.insert(name.clone(), spec);
        }
    }
    specs
}
