use std::collections::BTreeMap;
use std::collections::BTreeSet;

use djls_source::Span;
use djls_source::Spanned;

use crate::db::Db;
use crate::models::extract::ExtractedBaseRef;
use crate::models::extract::ExtractedClass;
use crate::models::extract::ExtractedClasses;
use crate::models::graph::AncestryOutcome;
use crate::models::graph::BaseOutcome;
use crate::models::graph::BaseUnresolvedReason;
use crate::models::graph::ClassId;
use crate::models::graph::InheritanceRecord;
use crate::models::graph::InvalidAncestryReason;
use crate::models::graph::ModelGraph;
use crate::models::graph::ModelKind;
use crate::project::Project;
use crate::python::PythonModuleName;
use crate::python::resolve_prefix;

/// An extracted base after project and occurrence resolution, before admission.
#[derive(Clone)]
enum ResolvedClassBase {
    DjangoModelRoot,
    Class(ClassId),
    ReboundLocalBase {
        class: ClassId,
        has_positive_model_evidence: bool,
    },
    Unresolved(ClassBaseUnresolvedReason),
}

/// A failure that can occur while resolving an extracted base, before model
/// admission assigns terminal model/class meaning.
#[derive(Clone)]
enum ClassBaseUnresolvedReason {
    UnsupportedExpression,
    MissingImportBinding {
        path: Vec<String>,
    },
    ShadowedImportBinding {
        path: Vec<String>,
    },
    InvalidImportedTarget {
        target: String,
    },
    ImportNotFound {
        requested: PythonModuleName,
    },
    ImportedTargetIsModule {
        module: PythonModuleName,
    },
    PartialImport {
        resolved_prefix: PythonModuleName,
        unresolved_tail: Vec<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum MroEntry {
    DjangoModelRoot,
    Class(ClassId),
}

#[derive(Clone)]
enum ComputedAncestry {
    Complete { mro: Vec<MroEntry> },
    Partial { known_mro: Vec<MroEntry> },
    Invalid { reason: InvalidAncestryReason },
}

#[derive(Clone, Copy)]
struct ClassOccurrence {
    span: Span,
    has_positive_model_evidence: bool,
}

struct ResolvedClass {
    extracted: ExtractedClass,
    bases: Vec<Spanned<ResolvedClassBase>>,
}

#[derive(Clone, Copy)]
enum AdmissionPolicy {
    Production,
    Local,
}

pub(super) fn resolve_model_inheritance(
    db: &dyn Db,
    project: Project,
    classes: Vec<ExtractedClass>,
) -> ModelGraph {
    let mut prefix_cache = BTreeMap::new();
    assemble_model_graph(classes, AdmissionPolicy::Production, |class, base| {
        resolve_class_base(class, base, &mut |path| {
            resolve_project_qualified_base(db, project, path, &mut prefix_cache)
        })
    })
}

/// Resolve one file's extracted classes without consulting a Project.
///
/// This keeps corpus extraction deterministic and limited to classes whose
/// Django model ancestry can be proven within the file. Qualified bases remain
/// unresolved and cannot seed local model admission.
pub(crate) fn resolve_local_model_graph(extraction: &ExtractedClasses) -> ModelGraph {
    assemble_model_graph(
        extraction.as_slice().iter().cloned(),
        AdmissionPolicy::Local,
        |class, base| {
            resolve_class_base(class, base, &mut |path| {
                ResolvedClassBase::Unresolved(ClassBaseUnresolvedReason::ImportNotFound {
                    requested: path.clone(),
                })
            })
        },
    )
}

fn assemble_model_graph(
    classes: impl IntoIterator<Item = ExtractedClass>,
    policy: AdmissionPolicy,
    mut resolve_base: impl FnMut(&ExtractedClass, &ExtractedBaseRef) -> ResolvedClassBase,
) -> ModelGraph {
    let mut occurrences: BTreeMap<ClassId, Vec<ClassOccurrence>> = BTreeMap::new();
    let mut winners = BTreeMap::new();

    // Keep each declaration until occurrence-local rebinding has been resolved;
    // only then does the final module/name winner replace earlier declarations.
    for class in classes {
        let id = class_id(&class);
        let has_positive_model_evidence =
            class_occurrence_has_positive_model_evidence(&class, &occurrences);
        occurrences
            .entry(id.clone())
            .or_default()
            .push(ClassOccurrence {
                span: class.name.span(),
                has_positive_model_evidence,
            });
        winners.insert(id, class);
    }
    let selected_spans: BTreeMap<ClassId, Span> = winners
        .iter()
        .map(|(id, class)| (id.clone(), class.name.span()))
        .collect();

    let resolved: BTreeMap<ClassId, ResolvedClass> = winners
        .into_iter()
        .map(|(id, extracted)| {
            let bases = extracted
                .bases
                .iter()
                .map(|base| {
                    let resolved = resolve_class_base_rebinding(
                        &extracted,
                        base.value(),
                        base.span(),
                        &occurrences,
                        &selected_spans,
                    )
                    .unwrap_or_else(|| resolve_base(&extracted, base.value()));
                    Spanned::new(resolved, base.span())
                })
                .collect();
            (id, ResolvedClass { extracted, bases })
        })
        .collect();
    let admitted = admitted_class_ids(&resolved, policy);

    // Terminal outcomes are the evidence retained by ModelGraph.
    let bases_by_class: BTreeMap<ClassId, Vec<BaseOutcome>> = resolved
        .iter()
        .map(|(id, class)| {
            let bases = class
                .bases
                .iter()
                .map(|base| terminal_outcome(base, class, &resolved, &admitted, policy))
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
                mro: vec![MroEntry::Class(id.clone())],
            }) {
            ComputedAncestry::Complete { mro } => AncestryOutcome::Complete {
                mro: mro
                    .into_iter()
                    .filter_map(|entry| match entry {
                        MroEntry::DjangoModelRoot => None,
                        MroEntry::Class(class) => Some(class),
                    })
                    .collect(),
            },
            ComputedAncestry::Partial { .. } => AncestryOutcome::Partial,
            ComputedAncestry::Invalid { reason } => AncestryOutcome::Invalid { reason },
        };
        inheritance_by_model.insert(id.clone(), InheritanceRecord { bases, ancestry });
    }

    let mut graph = ModelGraph::new();
    for (id, resolved_class) in resolved {
        if let Some(inheritance) = inheritance_by_model.remove(&id) {
            let (definition, local_bindings) = resolved_class.extracted.into_admitted_model();
            graph.insert_resolved_model(definition, inheritance, local_bindings);
        } else {
            graph.add_non_model_class(&id, resolved_class.extracted.local_bindings);
        }
    }
    graph.build_effective_relation_bindings();
    graph
}

fn admitted_class_ids(
    classes: &BTreeMap<ClassId, ResolvedClass>,
    policy: AdmissionPolicy,
) -> BTreeSet<ClassId> {
    let mut ids: BTreeSet<ClassId> = classes
        .iter()
        .filter(|(_id, resolved)| match policy {
            AdmissionPolicy::Production => {
                has_django_root(resolved)
                    || has_positive_rebound_local_base(resolved)
                    || (!has_negative_django_root_evidence(resolved, classes)
                        && (resolved.extracted.declared_model_kind != ModelKind::Concrete
                            || resolved.extracted.has_local_relation_binding()))
            }
            AdmissionPolicy::Local => {
                has_django_root(resolved)
                    || has_positive_rebound_local_base(resolved)
                    || (!has_negative_django_root_evidence(resolved, classes)
                        && resolved.extracted.declared_model_kind != ModelKind::Concrete)
            }
        })
        .map(|(id, _resolved)| id.clone())
        .collect();

    loop {
        let descendants: Vec<ClassId> = classes
            .iter()
            .filter(|(id, _class)| !ids.contains(*id))
            .filter(|(_id, class)| {
                class.bases.iter().any(|base| {
                    let ResolvedClassBase::Class(parent_id) = base.value() else {
                        return false;
                    };
                    ids.contains(parent_id)
                        && match policy {
                            AdmissionPolicy::Production => true,
                            AdmissionPolicy::Local => {
                                classes.get(parent_id).is_some_and(|parent| {
                                    parent.extracted.file == class.extracted.file
                                })
                            }
                        }
                })
            })
            .map(|(id, _class)| id.clone())
            .collect();
        if descendants.is_empty() {
            break;
        }
        ids.extend(descendants);
    }

    ids
}

fn has_django_root(class: &ResolvedClass) -> bool {
    class
        .bases
        .iter()
        .any(|base| matches!(base.value(), ResolvedClassBase::DjangoModelRoot))
}

fn has_negative_django_root_evidence(
    class: &ResolvedClass,
    classes: &BTreeMap<ClassId, ResolvedClass>,
) -> bool {
    class.bases.iter().any(|base| match base.value() {
        ResolvedClassBase::Class(base_class) => {
            base_class.name() == "Model" && !classes.contains_key(base_class)
        }
        ResolvedClassBase::Unresolved(reason) => {
            let path = match reason {
                ClassBaseUnresolvedReason::MissingImportBinding { path }
                | ClassBaseUnresolvedReason::ShadowedImportBinding { path } => path,
                ClassBaseUnresolvedReason::UnsupportedExpression
                | ClassBaseUnresolvedReason::InvalidImportedTarget { .. }
                | ClassBaseUnresolvedReason::ImportNotFound { .. }
                | ClassBaseUnresolvedReason::ImportedTargetIsModule { .. }
                | ClassBaseUnresolvedReason::PartialImport { .. } => return false,
            };
            path.last().is_some_and(|name| name == "Model")
        }
        ResolvedClassBase::DjangoModelRoot | ResolvedClassBase::ReboundLocalBase { .. } => false,
    })
}

fn has_positive_rebound_local_base(class: &ResolvedClass) -> bool {
    class.bases.iter().any(|base| {
        matches!(
            base.value(),
            ResolvedClassBase::ReboundLocalBase {
                has_positive_model_evidence: true,
                ..
            }
        )
    })
}

fn class_occurrence_has_positive_model_evidence(
    class: &ExtractedClass,
    occurrences: &BTreeMap<ClassId, Vec<ClassOccurrence>>,
) -> bool {
    has_direct_model_evidence(class)
        || class.bases.iter().any(|base| {
            let ExtractedBaseRef::SameModule(name) = base.value() else {
                return false;
            };
            let base_class = ClassId::new(class.module_name.clone(), name.as_str());
            nearest_preceding_occurrence(occurrences, &base_class, base.span(), None)
                .is_some_and(|occurrence| occurrence.has_positive_model_evidence)
        })
}

fn has_direct_model_evidence(class: &ExtractedClass) -> bool {
    class.declared_model_kind != ModelKind::Concrete
        || class
            .bases
            .iter()
            .any(|base| matches!(base.value(), ExtractedBaseRef::DjangoModelRoot))
}

fn class_id(class: &ExtractedClass) -> ClassId {
    ClassId::new(class.module_name.clone(), class.name.value().as_str())
}

fn nearest_preceding_occurrence<'a>(
    occurrences: &'a BTreeMap<ClassId, Vec<ClassOccurrence>>,
    class: &ClassId,
    before: Span,
    excluded_span: Option<Span>,
) -> Option<&'a ClassOccurrence> {
    occurrences
        .get(class)?
        .iter()
        .filter(|occurrence| {
            Some(occurrence.span) != excluded_span && occurrence.span.start() < before.start()
        })
        .max_by_key(|occurrence| occurrence.span.start())
}

fn resolve_class_base_rebinding(
    extracted: &ExtractedClass,
    base: &ExtractedBaseRef,
    span: Span,
    occurrences: &BTreeMap<ClassId, Vec<ClassOccurrence>>,
    selected_spans: &BTreeMap<ClassId, Span>,
) -> Option<ResolvedClassBase> {
    let ExtractedBaseRef::SameModule(name) = base else {
        return None;
    };
    let class = ClassId::new(extracted.module_name.clone(), name.as_str());
    let nearest =
        nearest_preceding_occurrence(occurrences, &class, span, Some(extracted.name.span()))?;
    if selected_spans.get(&class) == Some(&nearest.span) {
        return None;
    }

    Some(ResolvedClassBase::ReboundLocalBase {
        class,
        has_positive_model_evidence: nearest.has_positive_model_evidence,
    })
}

fn resolve_class_base(
    extracted: &ExtractedClass,
    base: &ExtractedBaseRef,
    resolve_qualified: &mut impl FnMut(&PythonModuleName) -> ResolvedClassBase,
) -> ResolvedClassBase {
    match base {
        ExtractedBaseRef::DjangoModelRoot => ResolvedClassBase::DjangoModelRoot,
        ExtractedBaseRef::SameModule(name) => {
            ResolvedClassBase::Class(ClassId::new(extracted.module_name.clone(), name.as_str()))
        }
        ExtractedBaseRef::UnsupportedExpression => {
            ResolvedClassBase::Unresolved(ClassBaseUnresolvedReason::UnsupportedExpression)
        }
        ExtractedBaseRef::Qualified(path) => resolve_qualified(path),
        ExtractedBaseRef::MissingBinding { path } => {
            ResolvedClassBase::Unresolved(ClassBaseUnresolvedReason::MissingImportBinding {
                path: path.clone(),
            })
        }
        ExtractedBaseRef::ShadowedBinding { path } => {
            ResolvedClassBase::Unresolved(ClassBaseUnresolvedReason::ShadowedImportBinding {
                path: path.clone(),
            })
        }
        ExtractedBaseRef::InvalidTarget { target } => {
            ResolvedClassBase::Unresolved(ClassBaseUnresolvedReason::InvalidImportedTarget {
                target: target.clone(),
            })
        }
    }
}

fn resolve_project_qualified_base(
    db: &dyn Db,
    project: Project,
    path: &PythonModuleName,
    prefix_cache: &mut BTreeMap<PythonModuleName, crate::python::ResolvedPrefix>,
) -> ResolvedClassBase {
    let resolved = prefix_cache
        .entry(path.clone())
        .or_insert_with(|| resolve_prefix(db, project, path.as_str()));
    let Some(module) = &resolved.module else {
        return ResolvedClassBase::Unresolved(ClassBaseUnresolvedReason::ImportNotFound {
            requested: path.clone(),
        });
    };
    match resolved.unresolved_tail.as_slice() {
        [] => ResolvedClassBase::Unresolved(ClassBaseUnresolvedReason::ImportedTargetIsModule {
            module: module.name().clone(),
        }),
        [name] => ResolvedClassBase::Class(ClassId::new(module.name().clone(), name.as_str())),
        unresolved_tail => {
            ResolvedClassBase::Unresolved(ClassBaseUnresolvedReason::PartialImport {
                resolved_prefix: module.name().clone(),
                unresolved_tail: unresolved_tail.to_vec(),
            })
        }
    }
}

fn terminal_outcome(
    base: &Spanned<ResolvedClassBase>,
    class: &ResolvedClass,
    selected: &BTreeMap<ClassId, ResolvedClass>,
    admitted: &BTreeSet<ClassId>,
    policy: AdmissionPolicy,
) -> BaseOutcome {
    let span = base.span();
    match base.value() {
        ResolvedClassBase::DjangoModelRoot => BaseOutcome::DjangoModelRoot { span },
        ResolvedClassBase::Class(base_class) => {
            let known = selected.get(base_class).is_some_and(|target| match policy {
                AdmissionPolicy::Production => true,
                AdmissionPolicy::Local => target.extracted.file == class.extracted.file,
            });
            if !known {
                return BaseOutcome::Unresolved {
                    span,
                    reason: BaseUnresolvedReason::ClassNotFound {
                        class: base_class.clone(),
                    },
                };
            }
            if admitted.contains(base_class) {
                BaseOutcome::Model {
                    model: base_class.clone().into_admitted_model_id(),
                    span,
                }
            } else {
                BaseOutcome::NonModelClass {
                    class: base_class.clone(),
                    span,
                }
            }
        }
        ResolvedClassBase::ReboundLocalBase {
            class: base_class, ..
        } => BaseOutcome::Unresolved {
            span,
            reason: BaseUnresolvedReason::ReboundLocalBase {
                class: base_class.clone(),
            },
        },
        ResolvedClassBase::Unresolved(reason) => BaseOutcome::Unresolved {
            span,
            reason: match reason {
                ClassBaseUnresolvedReason::UnsupportedExpression => {
                    BaseUnresolvedReason::UnsupportedExpression
                }
                ClassBaseUnresolvedReason::MissingImportBinding { path } => {
                    BaseUnresolvedReason::MissingImportBinding { path: path.clone() }
                }
                ClassBaseUnresolvedReason::ShadowedImportBinding { path } => {
                    BaseUnresolvedReason::ShadowedImportBinding { path: path.clone() }
                }
                ClassBaseUnresolvedReason::InvalidImportedTarget { target } => {
                    BaseUnresolvedReason::InvalidImportedTarget {
                        target: target.clone(),
                    }
                }
                ClassBaseUnresolvedReason::ImportNotFound { requested } => {
                    BaseUnresolvedReason::ImportNotFound {
                        requested: requested.clone(),
                    }
                }
                ClassBaseUnresolvedReason::ImportedTargetIsModule { module } => {
                    BaseUnresolvedReason::ImportedTargetIsModule {
                        module: module.clone(),
                    }
                }
                ClassBaseUnresolvedReason::PartialImport {
                    resolved_prefix,
                    unresolved_tail,
                } => BaseUnresolvedReason::PartialImport {
                    resolved_prefix: resolved_prefix.clone(),
                    unresolved_tail: unresolved_tail.clone(),
                },
            },
        },
    }
}

fn compute_ancestry(
    id: &ClassId,
    bases_by_class: &BTreeMap<ClassId, Vec<BaseOutcome>>,
    memo: &mut BTreeMap<ClassId, ComputedAncestry>,
    visiting: &mut BTreeSet<ClassId>,
) -> ComputedAncestry {
    if let Some(outcome) = memo.get(id) {
        return outcome.clone();
    }
    if !visiting.insert(id.clone()) {
        return ComputedAncestry::Invalid {
            reason: InvalidAncestryReason::Cycle,
        };
    }

    let Some(bases) = bases_by_class.get(id) else {
        let outcome = ComputedAncestry::Complete {
            mro: vec![MroEntry::Class(id.clone())],
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
        let entry = match base {
            BaseOutcome::DjangoModelRoot { .. } => MroEntry::DjangoModelRoot,
            BaseOutcome::Model { model, .. } => MroEntry::Class(ClassId::from_model_id(model)),
            BaseOutcome::NonModelClass { class, .. } => MroEntry::Class(class.clone()),
            BaseOutcome::Unresolved { .. } => {
                has_unresolved = true;
                continue;
            }
        };
        if !seen.insert(entry.clone()) && duplicate.is_none() {
            duplicate = Some(match &entry {
                MroEntry::DjangoModelRoot => InvalidAncestryReason::DuplicateDjangoModelRoot,
                MroEntry::Class(class) => InvalidAncestryReason::DuplicateClassBase {
                    class: class.clone(),
                },
            });
        }
        direct_parents.push(entry);
    }

    let mut parent_mros = Vec::new();
    let mut parent_is_partial = false;
    let mut inherited_reason = None;
    for parent in &direct_parents {
        match parent {
            MroEntry::DjangoModelRoot => {
                parent_mros.push(vec![MroEntry::DjangoModelRoot]);
            }
            MroEntry::Class(parent) => {
                match compute_ancestry(parent, bases_by_class, memo, visiting) {
                    ComputedAncestry::Complete { mro } => parent_mros.push(mro),
                    ComputedAncestry::Partial { known_mro } => {
                        parent_mros.push(known_mro);
                        parent_is_partial = true;
                    }
                    ComputedAncestry::Invalid { reason } => {
                        inherited_reason.get_or_insert(reason);
                    }
                }
            }
        }
    }

    let outcome = if let Some(reason) = duplicate.or(inherited_reason) {
        ComputedAncestry::Invalid { reason }
    } else {
        match c3_merge(parent_mros, direct_parents) {
            None => ComputedAncestry::Invalid {
                reason: InvalidAncestryReason::InconsistentMethodResolutionOrder,
            },
            Some(mut tail) => {
                tail.insert(0, MroEntry::Class(id.clone()));
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

fn c3_merge(
    mut parent_mros: Vec<Vec<MroEntry>>,
    direct_parents: Vec<MroEntry>,
) -> Option<Vec<MroEntry>> {
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

    use super::MroEntry;
    use super::c3_merge;
    use super::resolve_local_model_graph;
    use crate::models::extract::extract_models_impl;
    use crate::models::graph::AncestryOutcome;
    use crate::models::graph::ClassId;
    use crate::models::graph::ModelGraph;
    use crate::models::graph::ModelId;
    use crate::python::PythonModuleName;
    use crate::python::import::ModuleKind;

    fn class(value: &str) -> MroEntry {
        let model = value
            .parse::<ModelId>()
            .expect("test class id should parse");
        MroEntry::Class(ClassId::from_model_id(&model))
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
        let left = class("app.models.Left");
        let right = class("app.models.Right");
        let root = MroEntry::DjangoModelRoot;
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
        let x = class("app.models.X");
        let y = class("app.models.Y");
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
                if mro == &[
                    ClassId::from_model_id(&child),
                    ClassId::from_model_id(&abstract_base),
                ]
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
