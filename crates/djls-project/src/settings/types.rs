use std::iter;

use camino::Utf8PathBuf;
use djls_source::Origin;
use serde::Serialize;
use serde::Serializer;
use serde::ser::SerializeStruct;

use crate::python::InvalidModuleName;
use crate::python::PythonModuleName;
use crate::python::evaluation::BranchConstraints;
use crate::python::evaluation::StructuralOrd;

pub(crate) const MAX_EXACT_SETTING_ALTERNATIVES: usize = 64;
const MAX_SETTING_ALTERNATIVES: usize = MAX_EXACT_SETTING_ALTERNATIVES + 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SettingCase<T, P> {
    Known(T),
    Unset,
    Dynamic(P),
    Malformed(P),
}

/// One feasible static settings value paired with its branch constraints.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ConstrainedSettingCase<T, P> {
    case: SettingCase<T, P>,
    constraints: BranchConstraints,
}

/// Non-empty feasible cases for one supported Django setting.
///
/// Cases retain branch constraints privately and collapse overflow into one conservative dynamic
/// remainder without exposing those constraints through serialization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SettingAlternatives<T, P> {
    cases: Vec<ConstrainedSettingCase<T, P>>,
}

impl<T, P> Serialize for SettingAlternatives<T, P>
where
    T: Serialize,
    P: Serialize,
{
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("SettingAlternatives", 1)?;
        state.serialize_field(
            "cases",
            &self.cases.iter().map(|case| &case.case).collect::<Vec<_>>(),
        )?;
        state.end()
    }
}

impl<T, P> SettingAlternatives<T, P>
where
    T: MergeEvidence,
    P: MergeEvidence + MergeDynamicEvidence,
{
    fn new(cases: Vec<SettingCase<T, P>>) -> Self {
        let mut alternatives = Self { cases: Vec::new() };
        for case in cases {
            alternatives.add(case);
        }
        assert!(
            !alternatives.cases.is_empty(),
            "a setting must have at least one semantic case"
        );
        alternatives
    }

    pub(super) fn from_constrained(cases: Vec<(SettingCase<T, P>, BranchConstraints)>) -> Self {
        let mut alternatives = Self { cases: Vec::new() };
        for (case, constraints) in cases {
            alternatives.add_with_constraints(case, constraints);
        }
        assert!(
            !alternatives.cases.is_empty(),
            "a setting must have at least one semantic case"
        );
        alternatives
    }

    fn constrained_cases(&self) -> impl Iterator<Item = (&SettingCase<T, P>, &BranchConstraints)> {
        self.cases
            .iter()
            .map(|case| (&case.case, &case.constraints))
    }

    pub(crate) fn add(&mut self, case: SettingCase<T, P>) {
        self.add_with_constraints(case, BranchConstraints::unconstrained());
    }

    fn add_with_constraints(&mut self, case: SettingCase<T, P>, constraints: BranchConstraints) {
        for existing in &mut self.cases {
            if existing.case.merge_evidence(&case) {
                existing.constraints.merge(constraints);
                return;
            }
        }
        if self.cases.len() < MAX_SETTING_ALTERNATIVES {
            self.cases
                .push(ConstrainedSettingCase { case, constraints });
            return;
        }
        if let SettingCase::Dynamic(additional) = case
            && let Some((remainder, constraints)) =
                self.cases
                    .iter_mut()
                    .rev()
                    .find_map(|case| match &mut case.case {
                        SettingCase::Dynamic(remainder) => Some((remainder, &mut case.constraints)),
                        SettingCase::Known(_) | SettingCase::Unset | SettingCase::Malformed(_) => {
                            None
                        }
                    })
        {
            remainder.merge_dynamic_evidence(additional);
            // A capped remainder may represent several incompatible branches. Treating it as
            // unconstrained is conservative while keeping the existing cap.
            *constraints = BranchConstraints::unconstrained();
        }
    }
}

impl<T, P> MergeEvidence for SettingAlternatives<T, P>
where
    T: MergeEvidence,
    P: MergeEvidence + MergeDynamicEvidence,
{
    fn can_merge_evidence(&self, other: &Self) -> bool {
        self.cases.len() == other.cases.len()
            && self
                .cases
                .iter()
                .zip(&other.cases)
                .all(|(case, other_case)| case.case.can_merge_evidence(&other_case.case))
    }

    fn absorb_evidence(&mut self, other: &Self) {
        for (case, other_case) in self.cases.iter_mut().zip(&other.cases) {
            case.case.absorb_evidence(&other_case.case);
            case.constraints.merge(other_case.constraints.clone());
        }
    }
}

/// Evidence for one semantic case that can absorb evidence for the same case.
///
/// Checking before merging lets a failed merge leave `self` untouched without
/// cloning it first.
pub(crate) trait MergeEvidence {
    /// Whether `other` describes the same semantic case as `self`.
    fn can_merge_evidence(&self, other: &Self) -> bool;

    /// Merge evidence from `other`, which must pass [`Self::can_merge_evidence`].
    fn absorb_evidence(&mut self, other: &Self);

    /// Merge evidence from `other` when both values describe the same semantic case.
    fn merge_evidence(&mut self, other: &Self) -> bool {
        let mergeable = self.can_merge_evidence(other);
        if mergeable {
            self.absorb_evidence(other);
        }
        mergeable
    }
}

pub(crate) trait MergeDynamicEvidence {
    /// Retain uncertainty causes from an additional dynamic case in a capped remainder.
    fn merge_dynamic_evidence(&mut self, other: Self);
}

macro_rules! equality_is_semantic {
    ($($ty:ty),+ $(,)?) => {$(
        impl MergeEvidence for $ty {
            fn can_merge_evidence(&self, other: &Self) -> bool {
                self == other
            }

            fn absorb_evidence(&mut self, _other: &Self) {}
        }
    )+};
}

impl<T: MergeEvidence> MergeEvidence for Option<T> {
    fn can_merge_evidence(&self, other: &Self) -> bool {
        match (self, other) {
            (Some(left), Some(right)) => left.can_merge_evidence(right),
            (None, None) => true,
            (Some(_), None) | (None, Some(_)) => false,
        }
    }

    fn absorb_evidence(&mut self, other: &Self) {
        if let (Some(left), Some(right)) = (self, other) {
            left.absorb_evidence(right);
        }
    }
}

impl<T: MergeEvidence> MergeEvidence for Vec<T> {
    fn can_merge_evidence(&self, other: &Self) -> bool {
        self.len() == other.len()
            && self
                .iter()
                .zip(other)
                .all(|(left, right)| left.can_merge_evidence(right))
    }

    fn absorb_evidence(&mut self, other: &Self) {
        for (left, right) in self.iter_mut().zip(other) {
            left.absorb_evidence(right);
        }
    }
}

impl<A: MergeEvidence, B: MergeEvidence> MergeEvidence for (A, B) {
    fn can_merge_evidence(&self, other: &Self) -> bool {
        self.0.can_merge_evidence(&other.0) && self.1.can_merge_evidence(&other.1)
    }

    fn absorb_evidence(&mut self, other: &Self) {
        self.0.absorb_evidence(&other.0);
        self.1.absorb_evidence(&other.1);
    }
}

impl<T: MergeEvidence, P: MergeEvidence> MergeEvidence for SettingCase<T, P> {
    fn can_merge_evidence(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Known(left), Self::Known(right)) => left.can_merge_evidence(right),
            (Self::Unset, Self::Unset) => true,
            (Self::Dynamic(left), Self::Dynamic(right))
            | (Self::Malformed(left), Self::Malformed(right)) => left.can_merge_evidence(right),
            _ => false,
        }
    }

    fn absorb_evidence(&mut self, other: &Self) {
        match (self, other) {
            (Self::Known(left), Self::Known(right)) => left.absorb_evidence(right),
            (Self::Dynamic(left), Self::Dynamic(right))
            | (Self::Malformed(left), Self::Malformed(right)) => left.absorb_evidence(right),
            // `can_merge_evidence` rejects every other pairing.
            _ => {}
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SettingIssue {
    pub(crate) kind: SettingIssueKind,
    pub(crate) origins: Vec<Origin>,
}

impl Serialize for SettingIssue {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let spans: Vec<_> = self.origins.iter().map(|origin| origin.span).collect();
        let mut state = serializer.serialize_struct("SettingIssue", 2)?;
        state.serialize_field("kind", &self.kind)?;
        state.serialize_field("spans", &spans)?;
        state.end()
    }
}

impl MergeEvidence for SettingIssue {
    fn can_merge_evidence(&self, other: &Self) -> bool {
        self.kind == other.kind
    }

    fn absorb_evidence(&mut self, other: &Self) {
        self.origins.extend(other.origins.iter().copied());
        self.origins.sort_by(StructuralOrd::structural_cmp);
        self.origins.dedup();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SettingIssueKind {
    DynamicExpression,
    DynamicNamespace,
    UnknownElement,
    UnknownUnpack,
    UnsupportedMutation,
    InvalidShape,
    MissingBackend,
    InvalidModuleName,
    SyntaxError,
    Unreadable,
}

/// A value with one or more normalized source origins.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WithOrigins<T> {
    pub(crate) value: T,
    origin: Origin,
    additional_origins: Vec<Origin>,
}

impl<T> WithOrigins<T> {
    pub(crate) fn new(
        value: T,
        origin: Origin,
        additional_origins: impl IntoIterator<Item = Origin>,
    ) -> Self {
        let (origin, additional_origins) =
            normalize_origins(origin, additional_origins.into_iter());
        Self {
            value,
            origin,
            additional_origins,
        }
    }

    pub(crate) fn origin(&self) -> Origin {
        self.origin
    }

    fn origins(&self) -> impl Iterator<Item = Origin> + '_ {
        iter::once(self.origin).chain(self.additional_origins.iter().copied())
    }
}

fn normalize_origins(
    origin: Origin,
    additional_origins: impl Iterator<Item = Origin>,
) -> (Origin, Vec<Origin>) {
    let mut origins = Vec::from([origin]);
    origins.extend(additional_origins);
    origins.sort_by(StructuralOrd::structural_cmp);
    origins.dedup();
    let origin = origins.remove(0);
    (origin, origins)
}

impl<T: MergeEvidence + PartialEq> MergeEvidence for WithOrigins<T> {
    fn can_merge_evidence(&self, other: &Self) -> bool {
        self.value == other.value
    }

    fn absorb_evidence(&mut self, other: &Self) {
        self.value.absorb_evidence(&other.value);
        (self.origin, self.additional_origins) = normalize_origins(
            self.origin,
            self.additional_origins
                .iter()
                .copied()
                .chain(other.origins()),
        );
    }
}

impl<T: Serialize> Serialize for WithOrigins<T> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let spans: Vec<_> = self.origins().map(|origin| origin.span).collect();
        let mut state = serializer.serialize_struct("WithOrigins", 2)?;
        state.serialize_field("value", &self.value)?;
        state.serialize_field("spans", &spans)?;
        state.end()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct InstalledApps {
    pub(crate) apps: Vec<WithOrigins<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum InstalledAppEvidence {
    Known(WithOrigins<String>),
    Issue(SettingIssue),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct PartialInstalledApps {
    /// Evidence remains in source order because Django app order is significant.
    pub(crate) evidence: Vec<InstalledAppEvidence>,
}

pub(crate) type InstalledAppsAlternatives =
    SettingAlternatives<InstalledApps, PartialInstalledApps>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct TemplateBackends {
    pub(crate) backends: Vec<TemplateBackend>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TemplateBackendEvidence {
    Backend(Box<PartialTemplateBackend>),
    Issue(SettingIssue),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct PartialTemplateBackends {
    /// Evidence remains in source order because backend order is significant.
    pub(crate) evidence: Vec<TemplateBackendEvidence>,
}

pub(crate) type TemplateSettingAlternatives =
    SettingAlternatives<TemplateBackends, PartialTemplateBackends>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct TemplateBackend {
    pub(crate) backend: WithOrigins<String>,
    pub(crate) dirs: Vec<WithOrigins<TemplateDirectoryPath>>,
    pub(crate) app_dirs: Option<WithOrigins<bool>>,
    pub(crate) libraries: Vec<(String, WithOrigins<PythonModuleName>)>,
    pub(crate) builtins: Vec<WithOrigins<PythonModuleName>>,
    pub(crate) context_processors: Vec<WithOrigins<TemplateContextProcessorPath>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TemplateDirectoryEvidence {
    Known(WithOrigins<TemplateDirectoryPath>),
    Issue(SettingIssue),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct PartialTemplateDirectories {
    pub(crate) evidence: Vec<TemplateDirectoryEvidence>,
}

impl PartialTemplateDirectories {
    pub(crate) fn new() -> Self {
        Self {
            evidence: Vec::new(),
        }
    }

    pub(crate) fn push_known(&mut self, path: WithOrigins<TemplateDirectoryPath>) {
        self.evidence.push(TemplateDirectoryEvidence::Known(path));
    }

    pub(crate) fn push_issue(&mut self, issue: SettingIssue) {
        self.evidence.push(TemplateDirectoryEvidence::Issue(issue));
    }

    pub(crate) fn extend_issues(&mut self, issues: impl IntoIterator<Item = SettingIssue>) {
        self.evidence
            .extend(issues.into_iter().map(TemplateDirectoryEvidence::Issue));
    }

    fn has_issues(&self) -> bool {
        self.evidence
            .iter()
            .any(|evidence| matches!(evidence, TemplateDirectoryEvidence::Issue(_)))
    }

    fn issues(&self) -> impl Iterator<Item = &SettingIssue> {
        self.evidence.iter().filter_map(|evidence| match evidence {
            TemplateDirectoryEvidence::Known(_) => None,
            TemplateDirectoryEvidence::Issue(issue) => Some(issue),
        })
    }

    pub(crate) fn into_known(self) -> Vec<WithOrigins<TemplateDirectoryPath>> {
        self.evidence
            .into_iter()
            .filter_map(|evidence| match evidence {
                TemplateDirectoryEvidence::Known(path) => Some(path),
                TemplateDirectoryEvidence::Issue(_) => None,
            })
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct SettingFieldEvidence<T> {
    pub(crate) known: T,
    pub(crate) issues: Vec<SettingIssue>,
}

impl<T> SettingFieldEvidence<T> {
    pub(crate) fn new(known: T) -> Self {
        Self {
            known,
            issues: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct PartialTemplateBackend {
    #[serde(skip)]
    pub(super) constraints: BranchConstraints,
    pub(crate) backend: SettingFieldEvidence<Option<WithOrigins<String>>>,
    pub(crate) dirs: PartialTemplateDirectories,
    pub(crate) app_dirs: SettingFieldEvidence<Option<WithOrigins<bool>>>,
    pub(crate) options: SettingFieldEvidence<()>,
    pub(crate) libraries: SettingFieldEvidence<Vec<(String, WithOrigins<PythonModuleName>)>>,
    pub(crate) builtins: SettingFieldEvidence<Vec<WithOrigins<PythonModuleName>>>,
    pub(crate) context_processors:
        SettingFieldEvidence<Vec<WithOrigins<TemplateContextProcessorPath>>>,
}

impl PartialTemplateBackend {
    pub(crate) fn has_issues(&self) -> bool {
        !self.backend.issues.is_empty()
            || self.dirs.has_issues()
            || !self.app_dirs.issues.is_empty()
            || !self.options.issues.is_empty()
            || !self.libraries.issues.is_empty()
            || !self.builtins.issues.is_empty()
            || !self.context_processors.issues.is_empty()
    }

    pub(crate) fn is_malformed(&self) -> bool {
        self.issues().any(|issue| {
            matches!(
                issue.kind,
                SettingIssueKind::InvalidShape
                    | SettingIssueKind::MissingBackend
                    | SettingIssueKind::InvalidModuleName
            )
        })
    }

    fn issues(&self) -> impl Iterator<Item = &SettingIssue> {
        self.backend
            .issues
            .iter()
            .chain(self.dirs.issues())
            .chain(&self.app_dirs.issues)
            .chain(&self.options.issues)
            .chain(&self.libraries.issues)
            .chain(&self.builtins.issues)
            .chain(&self.context_processors.issues)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct DjangoSettings {
    pub(crate) installed_apps: InstalledAppsAlternatives,
    pub(crate) templates: TemplateSettingAlternatives,
}

pub(crate) struct FeasibleSettingsCase<'a> {
    pub(crate) installed_apps: &'a SettingCase<InstalledApps, PartialInstalledApps>,
    pub(crate) templates: &'a SettingCase<TemplateBackends, PartialTemplateBackends>,
}

impl DjangoSettings {
    pub(crate) fn feasible_cases(&self) -> Vec<FeasibleSettingsCase<'_>> {
        let mut cases = Vec::new();
        for (installed_apps, app_constraints) in self.installed_apps.constrained_cases() {
            for (templates, template_constraints) in self.templates.constrained_cases() {
                if app_constraints.compatible_with(template_constraints) {
                    cases.push(FeasibleSettingsCase {
                        installed_apps,
                        templates,
                    });
                }
            }
        }
        cases
    }

    pub(crate) fn unreadable() -> Self {
        let issue = SettingIssue {
            kind: SettingIssueKind::Unreadable,
            origins: Vec::new(),
        };
        Self {
            installed_apps: SettingAlternatives::new(vec![SettingCase::Dynamic(
                PartialInstalledApps {
                    evidence: vec![InstalledAppEvidence::Issue(issue.clone())],
                },
            )]),
            templates: SettingAlternatives::new(vec![SettingCase::Dynamic(
                PartialTemplateBackends {
                    evidence: vec![TemplateBackendEvidence::Issue(issue)],
                },
            )]),
        }
    }
}

impl Default for DjangoSettings {
    fn default() -> Self {
        Self {
            installed_apps: SettingAlternatives::new(vec![SettingCase::Unset]),
            templates: SettingAlternatives::new(vec![SettingCase::Unset]),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct TemplateDirectoryPath(Utf8PathBuf);

impl TemplateDirectoryPath {
    pub(crate) fn new(path: Utf8PathBuf) -> Self {
        Self(path)
    }

    pub(crate) fn path(&self) -> &camino::Utf8Path {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
pub(crate) struct TemplateContextProcessorPath(String);

impl TemplateContextProcessorPath {
    pub(crate) fn parse(path: &str) -> Result<Self, InvalidModuleName> {
        let name = PythonModuleName::parse(path)?;
        Ok(Self(name.into_string()))
    }
}

macro_rules! merge_struct_fields {
    ($ty:ty { $($field:ident),+ $(,)? }) => {
        impl MergeEvidence for $ty {
            fn can_merge_evidence(&self, other: &Self) -> bool {
                true $(&& self.$field.can_merge_evidence(&other.$field))+
            }

            fn absorb_evidence(&mut self, other: &Self) {
                $(self.$field.absorb_evidence(&other.$field);)+
            }
        }
    };
}

equality_is_semantic!(
    (),
    bool,
    String,
    SettingIssueKind,
    PythonModuleName,
    TemplateDirectoryPath,
    TemplateContextProcessorPath,
);

merge_struct_fields!(InstalledApps { apps });
impl MergeEvidence for InstalledAppEvidence {
    fn can_merge_evidence(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Known(left), Self::Known(right)) => left.can_merge_evidence(right),
            (Self::Issue(left), Self::Issue(right)) => left.can_merge_evidence(right),
            (Self::Known(_), Self::Issue(_)) | (Self::Issue(_), Self::Known(_)) => false,
        }
    }

    fn absorb_evidence(&mut self, other: &Self) {
        match (self, other) {
            (Self::Known(left), Self::Known(right)) => left.absorb_evidence(right),
            (Self::Issue(left), Self::Issue(right)) => left.absorb_evidence(right),
            // `can_merge_evidence` rejects mixed variants.
            (Self::Known(_), Self::Issue(_)) | (Self::Issue(_), Self::Known(_)) => {}
        }
    }
}
merge_struct_fields!(PartialInstalledApps { evidence });
impl MergeDynamicEvidence for PartialInstalledApps {
    fn merge_dynamic_evidence(&mut self, other: Self) {
        for evidence in other.evidence {
            if let InstalledAppEvidence::Issue(additional) = evidence {
                let mut issues = self.evidence.iter_mut().filter_map(|evidence| {
                    if let InstalledAppEvidence::Issue(issue) = evidence {
                        Some(issue)
                    } else {
                        None
                    }
                });
                if let Some(existing) = issues.find(|issue| issue.kind == additional.kind) {
                    let _ = existing.merge_evidence(&additional);
                } else {
                    self.evidence.push(InstalledAppEvidence::Issue(additional));
                }
            }
        }
    }
}
impl MergeEvidence for TemplateBackends {
    fn can_merge_evidence(&self, other: &Self) -> bool {
        self.backends.len() == other.backends.len()
            && self
                .backends
                .iter()
                .zip(&other.backends)
                .all(|(left, right)| same_path_origin_files(&left.dirs, &right.dirs))
            && self.backends.can_merge_evidence(&other.backends)
    }

    fn absorb_evidence(&mut self, other: &Self) {
        self.backends.absorb_evidence(&other.backends);
    }
}
impl MergeEvidence for TemplateBackendEvidence {
    fn can_merge_evidence(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Backend(left), Self::Backend(right)) => left.can_merge_evidence(right),
            (Self::Issue(left), Self::Issue(right)) => left.can_merge_evidence(right),
            (Self::Backend(_), Self::Issue(_)) | (Self::Issue(_), Self::Backend(_)) => false,
        }
    }

    fn absorb_evidence(&mut self, other: &Self) {
        match (self, other) {
            (Self::Backend(left), Self::Backend(right)) => left.absorb_evidence(right),
            (Self::Issue(left), Self::Issue(right)) => left.absorb_evidence(right),
            // `can_merge_evidence` rejects mixed variants.
            (Self::Backend(_), Self::Issue(_)) | (Self::Issue(_), Self::Backend(_)) => {}
        }
    }
}
merge_struct_fields!(PartialTemplateBackends { evidence });
impl MergeDynamicEvidence for PartialTemplateBackends {
    fn merge_dynamic_evidence(&mut self, other: Self) {
        for evidence in other.evidence {
            if let TemplateBackendEvidence::Issue(additional) = evidence {
                let existing = self.evidence.iter_mut().find_map(|evidence| {
                    if let TemplateBackendEvidence::Issue(issue) = evidence
                        && issue.kind == additional.kind
                    {
                        Some(issue)
                    } else {
                        None
                    }
                });
                if let Some(existing) = existing {
                    let _ = existing.merge_evidence(&additional);
                } else {
                    self.evidence
                        .push(TemplateBackendEvidence::Issue(additional));
                }
            }
        }
    }
}
impl MergeEvidence for TemplateBackend {
    fn can_merge_evidence(&self, other: &Self) -> bool {
        same_path_origin_files(&self.dirs, &other.dirs)
            && self.backend.can_merge_evidence(&other.backend)
            && self.dirs.can_merge_evidence(&other.dirs)
            && self.app_dirs.can_merge_evidence(&other.app_dirs)
            && self.libraries.can_merge_evidence(&other.libraries)
            && self.builtins.can_merge_evidence(&other.builtins)
            && self
                .context_processors
                .can_merge_evidence(&other.context_processors)
    }

    fn absorb_evidence(&mut self, other: &Self) {
        self.backend.absorb_evidence(&other.backend);
        self.dirs.absorb_evidence(&other.dirs);
        self.app_dirs.absorb_evidence(&other.app_dirs);
        self.libraries.absorb_evidence(&other.libraries);
        self.builtins.absorb_evidence(&other.builtins);
        self.context_processors
            .absorb_evidence(&other.context_processors);
    }
}
impl MergeEvidence for TemplateDirectoryEvidence {
    fn can_merge_evidence(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Known(left), Self::Known(right)) => left.can_merge_evidence(right),
            (Self::Issue(left), Self::Issue(right)) => left.can_merge_evidence(right),
            (Self::Known(_), Self::Issue(_)) | (Self::Issue(_), Self::Known(_)) => false,
        }
    }

    fn absorb_evidence(&mut self, other: &Self) {
        match (self, other) {
            (Self::Known(left), Self::Known(right)) => left.absorb_evidence(right),
            (Self::Issue(left), Self::Issue(right)) => left.absorb_evidence(right),
            // `can_merge_evidence` rejects mixed variants.
            (Self::Known(_), Self::Issue(_)) | (Self::Issue(_), Self::Known(_)) => {}
        }
    }
}
impl MergeEvidence for PartialTemplateDirectories {
    fn can_merge_evidence(&self, other: &Self) -> bool {
        if self.evidence.len() != other.evidence.len()
            || self
                .evidence
                .iter()
                .zip(&other.evidence)
                .any(|(left, right)| match (left, right) {
                    (
                        TemplateDirectoryEvidence::Known(left),
                        TemplateDirectoryEvidence::Known(right),
                    ) => !same_origin_files(left.origins(), right.origins()),
                    (
                        TemplateDirectoryEvidence::Issue(left),
                        TemplateDirectoryEvidence::Issue(right),
                    ) => !same_origin_files(
                        left.origins.iter().copied(),
                        right.origins.iter().copied(),
                    ),
                    _ => true,
                })
        {
            return false;
        }
        self.evidence.can_merge_evidence(&other.evidence)
    }

    fn absorb_evidence(&mut self, other: &Self) {
        self.evidence.absorb_evidence(&other.evidence);
    }
}
impl<T: MergeEvidence> MergeEvidence for SettingFieldEvidence<T> {
    fn can_merge_evidence(&self, other: &Self) -> bool {
        self.known.can_merge_evidence(&other.known) && self.issues.can_merge_evidence(&other.issues)
    }

    fn absorb_evidence(&mut self, other: &Self) {
        self.known.absorb_evidence(&other.known);
        self.issues.absorb_evidence(&other.issues);
    }
}
impl MergeEvidence for BranchConstraints {
    fn can_merge_evidence(&self, _other: &Self) -> bool {
        true
    }

    fn absorb_evidence(&mut self, other: &Self) {
        self.merge(other.clone());
    }
}
merge_struct_fields!(PartialTemplateBackend {
    constraints,
    backend,
    dirs,
    app_dirs,
    options,
    libraries,
    builtins,
    context_processors,
});
fn same_path_origin_files(
    left: &[WithOrigins<TemplateDirectoryPath>],
    right: &[WithOrigins<TemplateDirectoryPath>],
) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .all(|(left, right)| same_origin_files(left.origins(), right.origins()))
}

fn same_origin_files(
    left: impl Iterator<Item = Origin>,
    right: impl Iterator<Item = Origin>,
) -> bool {
    let mut left_files = Vec::new();
    for origin in left {
        if !left_files.contains(&origin.file) {
            left_files.push(origin.file);
        }
    }
    let mut right_files = Vec::new();
    for origin in right {
        if !right_files.contains(&origin.file) {
            right_files.push(origin.file);
        }
    }
    left_files == right_files
}
merge_struct_fields!(DjangoSettings {
    installed_apps,
    templates,
});

#[cfg(test)]
mod tests {
    use djls_source::File;
    use djls_source::Origin;
    use djls_source::Span;
    use salsa::Id;
    use salsa::plumbing::FromId as _;
    use serde_json::json;
    use serde_json::to_value;

    use super::MergeEvidence;
    use super::SettingIssue;
    use super::SettingIssueKind;
    use super::WithOrigins;

    fn origin(start: u32) -> Origin {
        // SAFETY: The test index is below `salsa::Id::MAX_U32`; this synthetic
        // file is used only as an opaque identity and is never read.
        let file = File::from_id(unsafe { Id::from_index(0) });
        Origin::new(file, Span::new(start, 1))
    }

    #[test]
    fn typed_provenance_order_setting_issue_merge_is_reversed_and_idempotent() {
        let first = origin(1);
        let second = origin(2);
        let first_issue = SettingIssue {
            kind: SettingIssueKind::UnknownUnpack,
            origins: vec![first],
        };
        let second_issue = SettingIssue {
            kind: SettingIssueKind::UnknownUnpack,
            origins: vec![second],
        };

        let mut forward = first_issue.clone();
        assert!(forward.merge_evidence(&second_issue));
        let mut reversed = second_issue;
        assert!(reversed.merge_evidence(&first_issue));
        assert_eq!(forward, reversed);
        assert_eq!(forward.origins, [first, second]);

        let merged = forward.clone();
        assert!(forward.merge_evidence(&merged));
        assert_eq!(forward, merged);
    }

    #[test]
    fn with_origins_one_origin_accessors_and_serialization_are_total() {
        let first = origin(1);
        let value = WithOrigins::new("value".to_string(), first, []);

        assert_eq!(value.origin(), first);
        assert_eq!(value.origins().collect::<Vec<_>>(), [first]);
        assert_eq!(
            to_value(&value).expect("test value should serialize to JSON"),
            json!({
                "value": "value",
                "spans": [first.span],
            })
        );
    }

    #[test]
    fn with_origins_construction_normalizes_and_deduplicates_origins() {
        let first = origin(1);
        let second = origin(2);
        let value = WithOrigins::new("value".to_string(), second, [second, first, second]);

        assert_eq!(value.origin(), first);
        assert_eq!(value.origins().collect::<Vec<_>>(), [first, second]);
        assert_eq!(
            to_value(&value).expect("test value should serialize to JSON"),
            json!({
                "value": "value",
                "spans": [first.span, second.span],
            })
        );
    }

    #[test]
    fn with_origins_merge_is_reversed_and_idempotent() {
        let first = origin(1);
        let second = origin(2);
        let first_value = WithOrigins::new("value".to_string(), first, []);
        let second_value = WithOrigins::new("value".to_string(), second, []);

        let mut forward = second_value.clone();
        assert!(forward.merge_evidence(&first_value));
        let mut reversed = first_value;
        assert!(reversed.merge_evidence(&second_value));
        assert_eq!(forward, reversed);
        assert_eq!(forward.origins().collect::<Vec<_>>(), [first, second]);
        assert_eq!(
            to_value(&forward).expect("test value should serialize to JSON"),
            to_value(&reversed).expect("test value should serialize to JSON")
        );

        let merged = forward.clone();
        assert!(forward.merge_evidence(&merged));
        assert_eq!(forward, merged);
    }
}
