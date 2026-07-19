mod rules;
mod specs;

use std::collections::HashSet;

use djls_project::ContextualLibraryStep;
use djls_project::Project;
use djls_project::TemplateEnvironment;
use djls_project::TemplateLibraryKey;
use djls_project::TemplateSymbolKind;
use djls_project::template_environment;
use djls_project::template_library_tag_facts;
use djls_source::File;
use djls_source::Offset;
use djls_templates::NodeList;
pub(crate) use rules::evaluate_tag_rules;
pub use specs::EndTag;
pub use specs::IntermediateTag;
pub use specs::TagSpec;
pub use specs::TagSpecs;
pub use specs::builtin_tag_specs;

use crate::db::Db;
use crate::references::TemplateReferenceKind;
use crate::scoping::LoadState;
use crate::scoping::LoadedLibraries;
use crate::scoping::template_analysis_projection_for_file;
use crate::structure::CapturedClosingTag;
use crate::structure::active_template_tags;

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

    let facts = template_library_tag_facts(db, key);
    if !facts.tag_rules().is_empty() {
        specs.merge_tag_rules(facts.tag_rules());
    }
    if !facts.block_specs().is_empty() {
        specs.merge_block_specs(facts.block_specs());
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
fn effective_tag_spec(
    db: &dyn Db,
    file: File,
    name: &str,
    load_state: &LoadState<'_>,
) -> Option<TagSpec> {
    let Some(project) = db.project() else {
        return db.projectless_tag_specs().get(name).cloned();
    };
    effective_tag_spec_in_project(db, project, file, name, load_state)
}

fn effective_tag_spec_in_project(
    db: &dyn Db,
    project: Project,
    file: File,
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

pub(crate) fn effective_tag_spec_from_environment(
    db: &dyn Db,
    project: Project,
    environment: TemplateEnvironment<'_>,
    name: &str,
    loaded: &[&str],
) -> Option<TagSpec> {
    #[derive(Default)]
    struct Alternative<'a> {
        effective: Option<&'a TagSpec>,
        unknown: bool,
    }

    let mut agreed = None;
    let mut alternatives_agree = true;
    environment.fold_contextual_library_chains(
        loaded,
        Alternative::default,
        |alternative, step| {
            let ContextualLibraryStep::Library(library) = step else {
                alternative.unknown = true;
                return;
            };
            if let Some(spec) = library_tag_specs(db, project, library.key()).get(name) {
                alternative.effective = Some(spec);
                alternative.unknown = false;
            } else if library.symbol(TemplateSymbolKind::Tag, name).is_some() {
                alternative.effective = None;
                alternative.unknown = false;
            } else if library.symbol_inventory_is_open()
                && !hardcoded_tag_inventory_is_complete(library.module_name_str())
            {
                alternative.unknown = true;
            }
        },
        |alternative| {
            if alternative.unknown {
                alternatives_agree = false;
                return;
            }
            match agreed {
                None => agreed = Some(alternative.effective),
                Some(definition) if definition == alternative.effective => {}
                Some(_) => alternatives_agree = false,
            }
        },
    );

    alternatives_agree
        .then_some(agreed.flatten())
        .flatten()
        .cloned()
}

/// Specs effective before any file-local `{% load %}` statement.
#[salsa::tracked(returns(ref))]
pub fn tag_specs_for_file(db: &dyn Db, file: File) -> TagSpecs {
    let empty = LoadedLibraries::default();
    completion_tag_specs_for_load_state(db, file, &empty.available_at(0))
}

/// Return the converged spec for the active tag occurrence at `position`.
///
/// This is occurrence meaning, not a name lookup: a spelling captured as an
/// intermediate or closer by an open block does not retain a colliding standalone spec.
#[must_use]
pub fn tag_spec_at(
    db: &dyn Db,
    file: File,
    nodelist: NodeList<'_>,
    position: u32,
    name: &str,
) -> Option<TagSpec> {
    let projection = template_analysis_projection_for_file(db, file, nodelist);
    let offset = Offset::new(position);
    if let Some(closer) = projection
        .captured_closers(db)
        .iter()
        .map(CapturedClosingTag::as_active)
        .find(|closer| closer.tag == name && closer.full_span.contains(offset))
    {
        return projection
            .scoped_tag_facts(db)
            .for_tag(closer)
            .and_then(|facts| facts.spec.clone());
    }

    let tree = projection.tree(db);
    if let Some(tag) = active_template_tags(tree.regions(db), tree.root(db))
        .into_iter()
        .find(|tag| tag.tag == name && tag.full_span.contains(offset))
    {
        return projection
            .scoped_tag_facts(db)
            .for_tag(tag)
            .and_then(|facts| facts.spec.clone());
    }

    // Error recovery can leave a syntactically recognizable completion context
    // without a structural occurrence. Resolve its name against the converged
    // load prefix, but never use this fallback for a captured structural tag.
    effective_tag_spec(
        db,
        file,
        name,
        &projection.loaded_libraries(db).available_at(position),
    )
}

#[salsa::tracked(returns(ref))]
pub fn tag_specs_at(db: &dyn Db, file: File, nodelist: NodeList<'_>, position: u32) -> TagSpecs {
    let projection = template_analysis_projection_for_file(db, file, nodelist);
    completion_tag_specs_for_load_state(
        db,
        file,
        &projection.loaded_libraries(db).available_at(position),
    )
}

fn completion_tag_specs_for_load_state(
    db: &dyn Db,
    file: File,
    load_state: &LoadState<'_>,
) -> TagSpecs {
    let names = if let Some(project) = db.project() {
        completion_tag_candidate_names(db, project, template_environment(db, project, file))
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

fn completion_tag_candidate_names(
    db: &dyn Db,
    project: Project,
    environment: TemplateEnvironment<'_>,
) -> HashSet<String> {
    let mut names: HashSet<_> = environment
        .inventory_symbol_names(TemplateSymbolKind::Tag)
        .map(str::to_owned)
        .collect();
    for library in environment.resolved_libraries() {
        names.extend(
            library_tag_specs(db, project, library.key())
                .iter()
                .map(|(name, _spec)| name.clone()),
        );
    }
    names
}
