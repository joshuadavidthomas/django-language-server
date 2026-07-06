use std::collections::BTreeMap;

use crate::db::Db;
use crate::models::extract::DeferredBaseRef;
use crate::models::extract::DeferredModel;
use crate::models::graph::ModelGraph;
use crate::models::graph::ModelId;
use crate::models::graph::ModelKind;
use crate::project::Project;
use crate::python::resolve_prefix;

pub(super) fn resolve_deferred_models(
    db: &dyn Db,
    project: Project,
    graph: &mut ModelGraph,
    deferred: Vec<DeferredModel>,
) {
    let mut remaining = deferred;
    let mut prefix_cache = BTreeMap::new();

    loop {
        let before = remaining.len();
        let mut unresolved = Vec::new();

        for deferred in remaining {
            let resolved_bases: Vec<ModelId> = deferred
                .bases
                .iter()
                .filter_map(|base| match base {
                    DeferredBaseRef::Qualified(path) => {
                        let resolved = prefix_cache
                            .entry(path.clone())
                            .or_insert_with(|| resolve_prefix(db, project, path.as_str()));
                        let module = resolved.module.as_ref()?;
                        let [name] = resolved.unresolved_tail.as_slice() else {
                            return None;
                        };

                        graph.model_id_in_module(module.name(), name)
                    }
                    DeferredBaseRef::SameModule(name) => {
                        graph.model_id_in_module(&deferred.model.module_name, name.as_str())
                    }
                })
                .collect();

            if resolved_bases.is_empty() {
                unresolved.push(deferred);
                continue;
            }

            let mut model = deferred.model.clone();
            let own_relations = std::mem::take(&mut model.relations);
            for base_id in &resolved_bases {
                let Some(parent) = graph.get_by_id(base_id) else {
                    continue;
                };
                if parent.kind == ModelKind::Abstract {
                    model.relations.extend(parent.relations.iter().cloned());
                }
            }
            model.relations.extend(own_relations);
            graph.add_model(model);
        }

        remaining = unresolved;
        if remaining.len() == before {
            break;
        }
    }
}
