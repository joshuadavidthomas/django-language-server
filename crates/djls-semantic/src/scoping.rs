pub(crate) mod loads;
pub(crate) mod symbols;

use djls_project::EffectiveDefinitionLibrary;
use djls_project::TemplateSymbolAvailability;
use djls_project::TemplateSymbolCandidate;
use djls_project::TemplateSymbolKind;
use djls_source::File;
use djls_templates::NodeList;

use crate::db::Db;
pub(crate) use crate::scoping::loads::LoadKind;
pub(crate) use crate::scoping::loads::LoadState;
pub(crate) use crate::scoping::loads::LoadStatement;
pub(crate) use crate::scoping::loads::LoadedLibraries;
pub(crate) use crate::scoping::symbols::SymbolIndex;
use crate::structure::active_template_tags;

#[salsa::tracked(returns(ref))]
pub(crate) fn compute_loaded_libraries_for_file(
    db: &dyn Db,
    file: File,
    nodelist: NodeList<'_>,
) -> LoadedLibraries {
    compute_loaded_libraries_for_file_in_scope(db, file, nodelist, file).clone()
}

#[salsa::tracked(returns(ref))]
pub(crate) fn compute_loaded_libraries_for_file_in_scope(
    db: &dyn Db,
    file: File,
    nodelist: NodeList<'_>,
    scope_file: File,
) -> LoadedLibraries {
    let source = file
        .try_source(db)
        .expect("a parsed template file has readable source");
    let fixed_point_limit = source.as_str().matches("{%").count() + 1;

    let mut loaded = LoadedLibraries::default();
    for _ in 0..fixed_point_limit {
        let index = crate::structure::grammar::scoped_tag_index_for_known_loads(
            db, file, scope_file, &loaded,
        );
        let tree = crate::structure::TemplateTreeBuilder::new(db, &index)
            .without_diagnostics()
            .model(db, nodelist);
        let mut statements = Vec::new();
        for tag in active_template_tags(tree.regions(db), tree.root(db)) {
            let preceding = LoadedLibraries::new(statements.clone());
            let load_state = preceding.available_at(tag.span.start());
            let effective = match db.project() {
                Some(project) => crate::tags::effective_tag_spec_in_project_for_scope(
                    db,
                    project,
                    scope_file,
                    tag.tag,
                    &load_state,
                ),
                None => crate::tags::effective_tag_spec(db, file, tag.tag, &load_state),
            };
            let role = effective.as_ref().and_then(crate::TagSpec::role);
            if role == Some(crate::tags::TagRole::TemplateLibraryLoader)
                && let Some(statement) = LoadStatement::from_tag(tag.tag, tag.bits, tag.span)
            {
                statements.push(statement);
            }
        }
        let next = LoadedLibraries::new(statements);
        if next == loaded {
            return next;
        }
        loaded = next;
    }
    panic!("template load discovery did not converge within the number of template tags")
}

/// Return the single effective definition of each symbol at a source position.
///
/// Django applies builtins and then loaded libraries in source order, with later
/// definitions shadowing earlier ones. Candidates are omitted when feasible
/// backends disagree about the effective definition.
#[must_use]
pub fn effective_symbol_candidates_at(
    db: &dyn Db,
    file: File,
    nodelist: NodeList<'_>,
    position: u32,
    kind: TemplateSymbolKind,
) -> Vec<TemplateSymbolCandidate> {
    let environment = crate::db::template_environment_for_file(db, file);
    let loaded = compute_loaded_libraries_for_file(db, file, nodelist);
    let load_state = loaded.available_at(position);
    let names = environment.candidate_symbol_names(db, kind);

    names
        .into_iter()
        .filter_map(|name| {
            let loaded_names = load_state.libraries_loading_symbol(&name);
            let definitions =
                environment.effective_definition_libraries(db, &name, kind, &loaded_names);
            let candidates = definitions
                .into_iter()
                .map(|definition| {
                    let EffectiveDefinitionLibrary::Known(Some(library)) = definition else {
                        return None;
                    };
                    let symbol = library
                        .symbols()
                        .iter()
                        .filter(|symbol| symbol.kind == kind && symbol.name() == name)
                        .find(|symbol| symbol.doc().is_some_and(|doc| !doc.trim().is_empty()))
                        .or_else(|| {
                            library
                                .symbols()
                                .iter()
                                .find(|symbol| symbol.kind == kind && symbol.name() == name)
                        })?
                        .clone();
                    let availability = library.load_name().map_or_else(
                        || TemplateSymbolAvailability::Builtin {
                            module: library.module_name().clone(),
                        },
                        |load_name| TemplateSymbolAvailability::RequiresLoad {
                            load_name: load_name.clone(),
                        },
                    );
                    Some(TemplateSymbolCandidate {
                        symbol,
                        availability,
                    })
                })
                .collect::<Option<Vec<_>>>()?;
            let first = candidates.first()?;
            if !candidates
                .iter()
                .all(|candidate| candidate.symbol.has_same_definition(&first.symbol))
            {
                return None;
            }

            // Documentation is presentation metadata and can vary between inventories that
            // identify the same definition. Prefer a non-empty value, then use its contents as a
            // stable tie-breaker instead of making backend order observable.
            let symbol = candidates
                .iter()
                .map(|candidate| &candidate.symbol)
                .max_by_key(|symbol| {
                    symbol
                        .doc()
                        .filter(|doc| !doc.trim().is_empty())
                        .map(str::trim)
                })?
                .clone();
            let mut availability = first.availability.clone();
            for candidate in &candidates[1..] {
                if availability == candidate.availability {
                    continue;
                }
                match (&availability, &candidate.availability) {
                    (
                        TemplateSymbolAvailability::Builtin { .. },
                        TemplateSymbolAvailability::RequiresLoad { .. },
                    ) => availability = candidate.availability.clone(),
                    (
                        TemplateSymbolAvailability::RequiresLoad { .. },
                        TemplateSymbolAvailability::Builtin { .. },
                    ) => {}
                    (
                        TemplateSymbolAvailability::RequiresLoad { .. },
                        TemplateSymbolAvailability::RequiresLoad { .. },
                    )
                    | (
                        TemplateSymbolAvailability::Builtin { .. },
                        TemplateSymbolAvailability::Builtin { .. },
                    ) => return None,
                }
            }

            Some(TemplateSymbolCandidate {
                symbol,
                availability,
            })
        })
        .collect()
}
