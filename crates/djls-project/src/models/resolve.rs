use std::collections::BTreeMap;
use std::collections::BTreeSet;

use djls_source::Span;

use crate::db::Db;
use crate::models::extract::CandidateBaseRef;
use crate::models::extract::CandidateBaseReferenceError;
use crate::models::extract::ModelCandidate;
use crate::models::extract::ModelExtraction;
use crate::models::graph::AncestryOutcome;
use crate::models::graph::BaseOutcome;
use crate::models::graph::BaseUnresolvedReason;
use crate::models::graph::InheritanceError;
use crate::models::graph::InheritanceRecord;
use crate::models::graph::ModelGraph;
use crate::models::graph::ModelId;
use crate::models::graph::ModelKind;
use crate::project::Project;
use crate::python::PythonModuleName;
use crate::python::resolve_prefix;

#[derive(Clone)]
enum ResolvedBase {
    DjangoModelRoot {
        span: Span,
    },
    Model {
        model: ModelId,
        span: Span,
    },
    ReboundLocalBase {
        span: Span,
        model: ModelId,
        has_positive_model_evidence: bool,
    },
    Unresolved {
        span: Span,
        reason: BaseUnresolvedReason,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum C3Node {
    DjangoModelRoot,
    Model(ModelId),
}

#[derive(Clone)]
enum ComputedAncestry {
    Complete { mro: Vec<C3Node> },
    Partial { known_mro: Vec<C3Node> },
    Invalid { error: InheritanceError },
}

#[derive(Clone, Copy)]
struct CandidateOccurrence {
    span: Span,
    has_positive_model_evidence: bool,
}

struct ResolvedCandidate {
    candidate: ModelCandidate,
    bases: Vec<ResolvedBase>,
}

#[derive(Clone, Copy)]
enum AdmissionPolicy {
    Production,
    Local,
}

pub(super) fn resolve_model_inheritance(
    db: &dyn Db,
    project: Project,
    candidates: Vec<ModelCandidate>,
) -> ModelGraph {
    let mut prefix_cache = BTreeMap::new();
    assemble_model_graph(
        candidates,
        AdmissionPolicy::Production,
        |candidate, base, span| {
            resolve_candidate_base(candidate, base, span, &mut |path, span| {
                resolve_project_qualified_base(db, project, path, span, &mut prefix_cache)
            })
        },
    )
}

/// Resolve one file's extracted model candidates without consulting a Project.
///
/// This keeps corpus extraction deterministic and limited to classes whose
/// Django model ancestry can be proven within the file. Qualified bases remain
/// unresolved and cannot seed local model admission.
pub(crate) fn resolve_local_model_graph(extraction: &ModelExtraction) -> ModelGraph {
    assemble_model_graph(
        extraction.candidates.iter().cloned(),
        AdmissionPolicy::Local,
        |candidate, base, span| {
            resolve_candidate_base(candidate, base, span, &mut |path, span| {
                ResolvedBase::Unresolved {
                    span,
                    reason: BaseUnresolvedReason::ImportNotFound {
                        requested: path.clone(),
                    },
                }
            })
        },
    )
}

fn assemble_model_graph(
    candidates: impl IntoIterator<Item = ModelCandidate>,
    policy: AdmissionPolicy,
    mut resolve_base: impl FnMut(&ModelCandidate, &CandidateBaseRef, Span) -> ResolvedBase,
) -> ModelGraph {
    let mut occurrences: BTreeMap<ModelId, Vec<CandidateOccurrence>> = BTreeMap::new();
    let mut winners = BTreeMap::new();

    // Candidates retain source order within their selected module file. Keep
    // every occurrence and its proven model evidence before reducing to the
    // final module binding: a base expression may refer to an earlier class
    // that a later declaration replaces. Evidence can flow through an earlier
    // same-module base only after that base occurrence has itself been proven.
    for candidate in candidates {
        let id = candidate_id(&candidate);
        let has_positive_model_evidence =
            candidate_occurrence_has_positive_model_evidence(&candidate, &occurrences);
        occurrences
            .entry(id.clone())
            .or_default()
            .push(CandidateOccurrence {
                span: candidate.model.name.span(),
                has_positive_model_evidence,
            });
        winners.insert(id, candidate);
    }
    let selected_spans: BTreeMap<ModelId, Span> = winners
        .iter()
        .map(|(id, candidate)| (id.clone(), candidate.model.name.span()))
        .collect();

    let resolved: BTreeMap<ModelId, ResolvedCandidate> = winners
        .into_iter()
        .map(|(id, candidate)| {
            let bases = candidate
                .bases
                .iter()
                .map(|base| {
                    resolve_candidate_base_rebinding(
                        &candidate,
                        base.value(),
                        base.span(),
                        &occurrences,
                        &selected_spans,
                    )
                    .unwrap_or_else(|| resolve_base(&candidate, base.value(), base.span()))
                })
                .collect();
            (id, ResolvedCandidate { candidate, bases })
        })
        .collect();
    let admitted = admitted_candidate_ids(&resolved, policy);

    let bases_by_class: BTreeMap<ModelId, Vec<BaseOutcome>> = resolved
        .iter()
        .map(|(id, candidate)| {
            let bases = candidate
                .bases
                .iter()
                .cloned()
                .map(|base| terminal_outcome(base, candidate, &resolved, &admitted, policy))
                .collect();
            (id.clone(), bases)
        })
        .collect();

    let mut ancestry = BTreeMap::new();
    for id in resolved.keys() {
        let mut visiting = BTreeSet::new();
        compute_ancestry(id, &bases_by_class, &mut ancestry, &mut visiting);
    }

    let mut inheritance_by_model = BTreeMap::new();
    for id in &admitted {
        let bases = bases_by_class.get(id).cloned().unwrap_or_default();
        let ancestry = match ancestry
            .get(id)
            .cloned()
            .unwrap_or(ComputedAncestry::Complete {
                mro: vec![C3Node::Model(id.clone())],
            }) {
            ComputedAncestry::Complete { mro } => AncestryOutcome::Complete {
                mro: mro
                    .into_iter()
                    .filter_map(|node| match node {
                        C3Node::DjangoModelRoot => None,
                        C3Node::Model(class) => Some(class),
                    })
                    .collect(),
            },
            ComputedAncestry::Partial { .. } => AncestryOutcome::Partial,
            ComputedAncestry::Invalid { error } => AncestryOutcome::Invalid { error },
        };
        inheritance_by_model.insert(id.clone(), InheritanceRecord { bases, ancestry });
    }

    let mut graph = ModelGraph::new();
    for (id, candidate) in resolved {
        if let Some(inheritance) = inheritance_by_model.remove(&id) {
            graph.insert_resolved_model(candidate.candidate.model, inheritance);
        } else {
            graph.add_non_model_class(&id, &candidate.candidate.model);
        }
    }
    graph.build_effective_relation_bindings();
    graph
}

fn admitted_candidate_ids(
    candidates: &BTreeMap<ModelId, ResolvedCandidate>,
    policy: AdmissionPolicy,
) -> BTreeSet<ModelId> {
    let mut ids: BTreeSet<ModelId> = candidates
        .iter()
        .filter(|(_id, resolved)| match policy {
            AdmissionPolicy::Production => {
                has_django_root(resolved)
                    || has_positive_rebound_local_base(resolved)
                    || (!has_negative_django_root_evidence(resolved, candidates)
                        && (resolved.candidate.model.kind != ModelKind::Concrete
                            || resolved.candidate.model.has_local_relation_binding()))
            }
            AdmissionPolicy::Local => {
                has_django_root(resolved)
                    || has_positive_rebound_local_base(resolved)
                    || (!has_negative_django_root_evidence(resolved, candidates)
                        && resolved.candidate.model.kind != ModelKind::Concrete)
            }
        })
        .map(|(id, _resolved)| id.clone())
        .collect();

    loop {
        let descendants: Vec<ModelId> = candidates
            .iter()
            .filter(|(id, _candidate)| !ids.contains(*id))
            .filter(|(_id, candidate)| {
                candidate.bases.iter().any(|base| {
                    let ResolvedBase::Model { model, .. } = base else {
                        return false;
                    };
                    ids.contains(model)
                        && match policy {
                            AdmissionPolicy::Production => true,
                            AdmissionPolicy::Local => candidates.get(model).is_some_and(|parent| {
                                parent.candidate.model.file == candidate.candidate.model.file
                            }),
                        }
                })
            })
            .map(|(id, _candidate)| id.clone())
            .collect();
        if descendants.is_empty() {
            break;
        }
        ids.extend(descendants);
    }

    ids
}

fn has_django_root(candidate: &ResolvedCandidate) -> bool {
    candidate
        .bases
        .iter()
        .any(|base| matches!(base, ResolvedBase::DjangoModelRoot { .. }))
}

fn has_negative_django_root_evidence(
    candidate: &ResolvedCandidate,
    candidates: &BTreeMap<ModelId, ResolvedCandidate>,
) -> bool {
    candidate.bases.iter().any(|base| match base {
        ResolvedBase::Model { model, .. } => {
            model.name() == "Model" && !candidates.contains_key(model)
        }
        ResolvedBase::Unresolved { reason, .. } => {
            let path = match reason {
                BaseUnresolvedReason::MissingImportBinding { path }
                | BaseUnresolvedReason::ShadowedImportBinding { path } => path,
                BaseUnresolvedReason::UnsupportedExpression
                | BaseUnresolvedReason::InvalidImportedTarget { .. }
                | BaseUnresolvedReason::ImportNotFound { .. }
                | BaseUnresolvedReason::ImportedTargetIsModule { .. }
                | BaseUnresolvedReason::PartialImport { .. }
                | BaseUnresolvedReason::ModelNotFound { .. }
                | BaseUnresolvedReason::ReboundLocalBase { .. } => return false,
            };
            path.last().is_some_and(|name| name == "Model")
        }
        ResolvedBase::DjangoModelRoot { .. } | ResolvedBase::ReboundLocalBase { .. } => false,
    })
}

fn has_positive_rebound_local_base(candidate: &ResolvedCandidate) -> bool {
    candidate.bases.iter().any(|base| {
        matches!(
            base,
            ResolvedBase::ReboundLocalBase {
                has_positive_model_evidence: true,
                ..
            }
        )
    })
}

fn candidate_occurrence_has_positive_model_evidence(
    candidate: &ModelCandidate,
    occurrences: &BTreeMap<ModelId, Vec<CandidateOccurrence>>,
) -> bool {
    has_direct_model_evidence(candidate)
        || candidate.bases.iter().any(|base| {
            let CandidateBaseRef::SameModule(name) = base.value() else {
                return false;
            };
            let model = ModelId::new(candidate.model.module_name.clone(), name.clone());
            nearest_preceding_occurrence(occurrences, &model, base.span(), None)
                .is_some_and(|occurrence| occurrence.has_positive_model_evidence)
        })
}

fn has_direct_model_evidence(candidate: &ModelCandidate) -> bool {
    candidate.model.kind != ModelKind::Concrete
        || candidate
            .bases
            .iter()
            .any(|base| matches!(base.value(), CandidateBaseRef::DjangoModelRoot))
}

fn candidate_id(candidate: &ModelCandidate) -> ModelId {
    ModelId::new(
        candidate.model.module_name.clone(),
        candidate.model.name.value().clone(),
    )
}

fn nearest_preceding_occurrence<'a>(
    occurrences: &'a BTreeMap<ModelId, Vec<CandidateOccurrence>>,
    model: &ModelId,
    before: Span,
    excluded_span: Option<Span>,
) -> Option<&'a CandidateOccurrence> {
    occurrences
        .get(model)?
        .iter()
        .filter(|occurrence| {
            Some(occurrence.span) != excluded_span && occurrence.span.start() < before.start()
        })
        .max_by_key(|occurrence| occurrence.span.start())
}

fn resolve_candidate_base_rebinding(
    candidate: &ModelCandidate,
    base: &CandidateBaseRef,
    span: Span,
    occurrences: &BTreeMap<ModelId, Vec<CandidateOccurrence>>,
    selected_spans: &BTreeMap<ModelId, Span>,
) -> Option<ResolvedBase> {
    let CandidateBaseRef::SameModule(name) = base else {
        return None;
    };
    let model = ModelId::new(candidate.model.module_name.clone(), name.clone());
    let nearest =
        nearest_preceding_occurrence(occurrences, &model, span, Some(candidate.model.name.span()))?;
    if selected_spans.get(&model) == Some(&nearest.span) {
        return None;
    }

    Some(ResolvedBase::ReboundLocalBase {
        span,
        model,
        has_positive_model_evidence: nearest.has_positive_model_evidence,
    })
}

fn resolve_candidate_base(
    candidate: &ModelCandidate,
    base: &CandidateBaseRef,
    span: Span,
    resolve_qualified: &mut impl FnMut(&PythonModuleName, Span) -> ResolvedBase,
) -> ResolvedBase {
    match base {
        CandidateBaseRef::DjangoModelRoot => ResolvedBase::DjangoModelRoot { span },
        CandidateBaseRef::SameModule(name) => ResolvedBase::Model {
            model: ModelId::new(candidate.model.module_name.clone(), name.clone()),
            span,
        },
        CandidateBaseRef::UnsupportedExpression => ResolvedBase::Unresolved {
            span,
            reason: BaseUnresolvedReason::UnsupportedExpression,
        },
        CandidateBaseRef::Qualified(path) => resolve_qualified(path, span),
        CandidateBaseRef::Unresolved { path, reason } => ResolvedBase::Unresolved {
            span,
            reason: match reason {
                CandidateBaseReferenceError::MissingBinding => {
                    BaseUnresolvedReason::MissingImportBinding { path: path.clone() }
                }
                CandidateBaseReferenceError::ShadowedBinding => {
                    BaseUnresolvedReason::ShadowedImportBinding { path: path.clone() }
                }
                CandidateBaseReferenceError::InvalidTarget { target } => {
                    BaseUnresolvedReason::InvalidImportedTarget {
                        target: target.clone(),
                    }
                }
            },
        },
    }
}

fn resolve_project_qualified_base(
    db: &dyn Db,
    project: Project,
    path: &PythonModuleName,
    span: Span,
    prefix_cache: &mut BTreeMap<PythonModuleName, crate::python::ResolvedPrefix>,
) -> ResolvedBase {
    let resolved = prefix_cache
        .entry(path.clone())
        .or_insert_with(|| resolve_prefix(db, project, path.as_str()));
    let Some(module) = &resolved.module else {
        return ResolvedBase::Unresolved {
            span,
            reason: BaseUnresolvedReason::ImportNotFound {
                requested: path.clone(),
            },
        };
    };
    match resolved.unresolved_tail.as_slice() {
        [] => ResolvedBase::Unresolved {
            span,
            reason: BaseUnresolvedReason::ImportedTargetIsModule {
                module: module.name().clone(),
            },
        },
        [name] => ResolvedBase::Model {
            model: ModelId::new(module.name().clone(), name.as_str().into()),
            span,
        },
        unresolved_tail => ResolvedBase::Unresolved {
            span,
            reason: BaseUnresolvedReason::PartialImport {
                resolved_prefix: module.name().clone(),
                unresolved_tail: unresolved_tail.to_vec(),
            },
        },
    }
}

fn terminal_outcome(
    base: ResolvedBase,
    candidate: &ResolvedCandidate,
    selected: &BTreeMap<ModelId, ResolvedCandidate>,
    admitted: &BTreeSet<ModelId>,
    policy: AdmissionPolicy,
) -> BaseOutcome {
    match base {
        ResolvedBase::DjangoModelRoot { span } => BaseOutcome::DjangoModelRoot { span },
        ResolvedBase::Model { model, span } => {
            let known = selected.get(&model).is_some_and(|target| match policy {
                AdmissionPolicy::Production => true,
                AdmissionPolicy::Local => {
                    target.candidate.model.file == candidate.candidate.model.file
                }
            });
            if !known {
                return BaseOutcome::Unresolved {
                    span,
                    reason: BaseUnresolvedReason::ModelNotFound { model },
                };
            }
            if admitted.contains(&model) {
                BaseOutcome::Model { model, span }
            } else {
                BaseOutcome::NonModelClass { class: model, span }
            }
        }
        ResolvedBase::ReboundLocalBase { span, model, .. } => BaseOutcome::Unresolved {
            span,
            reason: BaseUnresolvedReason::ReboundLocalBase { model },
        },
        ResolvedBase::Unresolved { span, reason } => BaseOutcome::Unresolved { span, reason },
    }
}

fn compute_ancestry(
    id: &ModelId,
    bases_by_model: &BTreeMap<ModelId, Vec<BaseOutcome>>,
    memo: &mut BTreeMap<ModelId, ComputedAncestry>,
    visiting: &mut BTreeSet<ModelId>,
) -> ComputedAncestry {
    if let Some(outcome) = memo.get(id) {
        return outcome.clone();
    }
    if !visiting.insert(id.clone()) {
        return ComputedAncestry::Invalid {
            error: InheritanceError::Cycle,
        };
    }

    let Some(bases) = bases_by_model.get(id) else {
        let outcome = ComputedAncestry::Complete {
            mro: vec![C3Node::Model(id.clone())],
        };
        visiting.remove(id);
        memo.insert(id.clone(), outcome.clone());
        return outcome;
    };

    let mut direct_parents = Vec::new();
    let mut has_unresolved = false;
    let mut seen = BTreeSet::new();
    let mut duplicate = None;
    for base in bases {
        let node = match base {
            BaseOutcome::DjangoModelRoot { .. } => C3Node::DjangoModelRoot,
            BaseOutcome::Model { model, .. } => C3Node::Model(model.clone()),
            BaseOutcome::NonModelClass { class, .. } => C3Node::Model(class.clone()),
            BaseOutcome::Unresolved { .. } => {
                has_unresolved = true;
                continue;
            }
        };
        if !seen.insert(node.clone()) && duplicate.is_none() {
            duplicate = Some(match &node {
                C3Node::DjangoModelRoot => InheritanceError::DuplicateDjangoModelRoot,
                C3Node::Model(model) => InheritanceError::DuplicateModelBase {
                    model: model.clone(),
                },
            });
        }
        direct_parents.push(node);
    }

    let mut parent_mros = Vec::new();
    let mut parent_is_partial = false;
    let mut inherited_error = None;
    for parent in &direct_parents {
        match parent {
            C3Node::DjangoModelRoot => {
                parent_mros.push(vec![C3Node::DjangoModelRoot]);
            }
            C3Node::Model(parent) => {
                match compute_ancestry(parent, bases_by_model, memo, visiting) {
                    ComputedAncestry::Complete { mro } => parent_mros.push(mro),
                    ComputedAncestry::Partial { known_mro } => {
                        parent_mros.push(known_mro);
                        parent_is_partial = true;
                    }
                    ComputedAncestry::Invalid { error } => {
                        inherited_error.get_or_insert(error);
                    }
                }
            }
        }
    }

    let outcome = if let Some(error) = duplicate.or(inherited_error) {
        ComputedAncestry::Invalid { error }
    } else {
        match c3_merge(parent_mros, direct_parents) {
            None => ComputedAncestry::Invalid {
                error: InheritanceError::InconsistentC3,
            },
            Some(mut tail) => {
                tail.insert(0, C3Node::Model(id.clone()));
                if has_unresolved || parent_is_partial {
                    ComputedAncestry::Partial { known_mro: tail }
                } else {
                    ComputedAncestry::Complete { mro: tail }
                }
            }
        }
    };
    visiting.remove(id);
    memo.insert(id.clone(), outcome.clone());
    outcome
}

fn c3_merge(mut parent_mros: Vec<Vec<C3Node>>, direct_parents: Vec<C3Node>) -> Option<Vec<C3Node>> {
    parent_mros.push(direct_parents);
    let mut result = Vec::new();

    while parent_mros.iter().any(|sequence| !sequence.is_empty()) {
        let candidate = parent_mros.iter().find_map(|sequence| {
            let head = sequence.first()?;
            parent_mros
                .iter()
                .all(|other| !other.iter().skip(1).any(|item| item == head))
                .then(|| head.clone())
        })?;
        result.push(candidate.clone());
        for sequence in &mut parent_mros {
            if sequence.first() == Some(&candidate) {
                sequence.remove(0);
            }
        }
    }

    Some(result)
}

#[cfg(test)]
mod tests {
    use camino::Utf8Path;
    use djls_testing::TestDatabase;

    use super::C3Node;
    use super::c3_merge;
    use super::resolve_local_model_graph;
    use crate::models::extract::extract_models_impl;
    use crate::models::graph::AncestryOutcome;
    use crate::models::graph::ModelGraph;
    use crate::models::graph::ModelId;
    use crate::python::PythonModuleName;
    use crate::python::import::ModuleKind;

    fn model(value: &str) -> C3Node {
        C3Node::Model(
            value
                .parse::<ModelId>()
                .expect("test model id should parse"),
        )
    }

    fn local_graph(source: &str) -> ModelGraph {
        let db = TestDatabase::new();
        db.add_file("/test.py", source)
            .expect("local model fixture should be added to the test database");
        let file = db
            .file(Utf8Path::new("/test.py"))
            .expect("local model fixture should exist in the test database");
        let module_name =
            PythonModuleName::parse("app.models").expect("test module name should be valid");
        let module = ruff_python_parser::parse_module(source)
            .expect("local model fixture should parse")
            .into_syntax();
        let extraction = extract_models_impl(&module.body, &module_name, file, ModuleKind::Module);
        resolve_local_model_graph(&extraction)
    }

    #[test]
    fn c3_prefers_the_left_parent_at_a_collision() {
        let left = model("app.models.Left");
        let right = model("app.models.Right");
        let root = C3Node::DjangoModelRoot;
        assert_eq!(
            c3_merge(
                vec![
                    vec![left.clone(), root.clone()],
                    vec![right.clone(), root.clone()]
                ],
                vec![left.clone(), right.clone()],
            ),
            Some(vec![left, right, root])
        );
    }

    #[test]
    fn c3_rejects_inconsistent_parent_order() {
        let x = model("app.models.X");
        let y = model("app.models.Y");
        assert_eq!(
            c3_merge(
                vec![vec![x.clone(), y.clone()], vec![y.clone(), x.clone()]],
                vec![x, y],
            ),
            None
        );
    }

    #[test]
    fn local_resolution_admits_proven_descendants_and_explicit_abstract_models() {
        let graph = local_graph(
            r"
from django.db import models
from external.models import ImportedBase

class Target(models.Model):
    pass

class AbstractBase(models.Model):
    owner = models.ForeignKey(Target)

    class Meta:
        abstract = True

class Child(AbstractBase):
    pass

class Qualified(ImportedBase):
    pass

class RelationOnly(ImportedBase):
    direct = models.ForeignKey(Target)

class AbstractQualified(ImportedBase):
    class Meta:
        abstract = True

class Unresolved(missing.Base):
    pass

class Unsupported(make_base()):
    pass
",
        );

        let child: ModelId = "app.models.Child"
            .parse()
            .expect("child model id should parse");
        let abstract_base: ModelId = "app.models.AbstractBase"
            .parse()
            .expect("abstract base model id should parse");
        assert_eq!(
            graph
                .owned_relation_entries(&child)
                .map(|(relation, _resolution)| relation.field_name.value().as_str())
                .collect::<Vec<_>>(),
            ["owner"]
        );
        assert!(matches!(
            &graph
                .inheritance(&child)
                .expect("Child should retain inheritance")
                .ancestry,
            AncestryOutcome::Complete { mro }
                if mro == &[child.clone(), abstract_base]
        ));

        let abstract_qualified: ModelId = "app.models.AbstractQualified"
            .parse()
            .expect("abstract model id should parse");
        assert!(graph.contains_model(&abstract_qualified));
        assert!(matches!(
            &graph
                .inheritance(&abstract_qualified)
                .expect("abstract model should retain inheritance")
                .ancestry,
            AncestryOutcome::Partial
        ));

        for absent in ["Qualified", "RelationOnly", "Unresolved", "Unsupported"] {
            let id = ModelId::new(
                PythonModuleName::parse("app.models").expect("test module name should be valid"),
                absent.into(),
            );
            assert!(
                !graph.contains_model(&id),
                "{absent} should not be admitted"
            );
        }
    }

    #[test]
    fn unresolved_local_base_makes_proven_ancestry_partial() {
        let graph = local_graph(
            r"
from django.db import models

class Target(models.Model):
    pass

class AbstractBase(models.Model):
    owner = models.ForeignKey(Target)

    class Meta:
        abstract = True

class Child(AbstractBase, missing.Base):
    pass
",
        );
        let child: ModelId = "app.models.Child"
            .parse()
            .expect("child model id should parse");

        assert_eq!(
            graph
                .inheritance(&child)
                .expect("Child should retain inheritance")
                .ancestry,
            AncestryOutcome::Partial,
        );
        assert!(graph.owned_relation_entries(&child).next().is_none());
    }
}
