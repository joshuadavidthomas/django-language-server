use camino::Utf8PathBuf;
use djls_source::Origin;
use serde::Serialize;
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct CorrelatedSettingCase<T, P> {
    case: SettingCase<T, P>,
    correlation: BranchConstraints,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SettingAlternatives<T, P> {
    cases: Vec<CorrelatedSettingCase<T, P>>,
}

impl<T, P> Serialize for SettingAlternatives<T, P>
where
    T: Serialize,
    P: Serialize,
{
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
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

    pub(super) fn from_correlated(cases: Vec<(SettingCase<T, P>, BranchConstraints)>) -> Self {
        let mut alternatives = Self { cases: Vec::new() };
        for (case, correlation) in cases {
            alternatives.add_with_correlation(case, correlation);
        }
        assert!(
            !alternatives.cases.is_empty(),
            "a setting must have at least one semantic case"
        );
        alternatives
    }

    fn cases_with_correlations(
        &self,
    ) -> impl Iterator<Item = (&SettingCase<T, P>, &BranchConstraints)> {
        self.cases
            .iter()
            .map(|case| (&case.case, &case.correlation))
    }

    pub(crate) fn add(&mut self, case: SettingCase<T, P>) {
        self.add_with_correlation(case, BranchConstraints::unconstrained());
    }

    fn add_with_correlation(&mut self, case: SettingCase<T, P>, correlation: BranchConstraints) {
        for existing in &mut self.cases {
            if existing.case.merge_evidence(&case) {
                existing.correlation.merge(correlation);
                return;
            }
        }
        if self.cases.len() < MAX_SETTING_ALTERNATIVES {
            self.cases.push(CorrelatedSettingCase { case, correlation });
            return;
        }
        if let SettingCase::Dynamic(additional) = case
            && let Some(existing) = self
                .cases
                .iter_mut()
                .rev()
                .find(|case| matches!(case.case, SettingCase::Dynamic(_)))
        {
            let SettingCase::Dynamic(remainder) = &mut existing.case else {
                unreachable!()
            };
            remainder.merge_dynamic_evidence(additional);
            // A capped remainder may represent several incompatible branches. Treating it as
            // unconstrained is conservative while keeping the existing cap.
            existing.correlation = BranchConstraints::unconstrained();
        }
    }
}

impl<T, P> MergeEvidence for SettingAlternatives<T, P>
where
    T: MergeEvidence,
    P: MergeEvidence + MergeDynamicEvidence,
{
    fn merge_evidence(&mut self, other: &Self) -> bool {
        if self.cases.len() != other.cases.len() {
            return false;
        }
        let mut merged = self.clone();
        for (case, other_case) in merged.cases.iter_mut().zip(&other.cases) {
            if !case.case.merge_evidence(&other_case.case) {
                return false;
            }
            case.correlation.merge(other_case.correlation.clone());
        }
        *self = merged;
        true
    }
}

pub(crate) trait MergeEvidence: Clone {
    /// Merge evidence from `other` when both values describe the same semantic case.
    fn merge_evidence(&mut self, other: &Self) -> bool;
}

pub(crate) trait MergeDynamicEvidence {
    /// Retain uncertainty causes from an additional dynamic case in a capped remainder.
    fn merge_dynamic_evidence(&mut self, other: Self);
}

macro_rules! equality_is_semantic {
    ($($ty:ty),+ $(,)?) => {$(
        impl MergeEvidence for $ty {
            fn merge_evidence(&mut self, other: &Self) -> bool {
                self == other
            }
        }
    )+};
}

impl<T: MergeEvidence> MergeEvidence for Option<T> {
    fn merge_evidence(&mut self, other: &Self) -> bool {
        match (self, other) {
            (Some(left), Some(right)) => left.merge_evidence(right),
            (None, None) => true,
            (Some(_), None) | (None, Some(_)) => false,
        }
    }
}

impl<T: MergeEvidence> MergeEvidence for Vec<T> {
    fn merge_evidence(&mut self, other: &Self) -> bool {
        if self.len() != other.len() {
            return false;
        }
        let mut merged = self.clone();
        if merged
            .iter_mut()
            .zip(other)
            .all(|(left, right)| left.merge_evidence(right))
        {
            *self = merged;
            true
        } else {
            false
        }
    }
}

impl<A: MergeEvidence, B: MergeEvidence> MergeEvidence for (A, B) {
    fn merge_evidence(&mut self, other: &Self) -> bool {
        let mut merged = self.clone();
        if merged.0.merge_evidence(&other.0) && merged.1.merge_evidence(&other.1) {
            *self = merged;
            true
        } else {
            false
        }
    }
}

impl<T: MergeEvidence, P: MergeEvidence> MergeEvidence for SettingCase<T, P> {
    fn merge_evidence(&mut self, other: &Self) -> bool {
        match (self, other) {
            (Self::Known(left), Self::Known(right)) => left.merge_evidence(right),
            (Self::Unset, Self::Unset) => true,
            (Self::Dynamic(left), Self::Dynamic(right))
            | (Self::Malformed(left), Self::Malformed(right)) => left.merge_evidence(right),
            _ => false,
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
        S: serde::Serializer,
    {
        let spans: Vec<_> = self.origins.iter().map(|origin| origin.span).collect();
        let mut state = serializer.serialize_struct("SettingIssue", 2)?;
        state.serialize_field("kind", &self.kind)?;
        state.serialize_field("spans", &spans)?;
        state.end()
    }
}

impl MergeEvidence for SettingIssue {
    fn merge_evidence(&mut self, other: &Self) -> bool {
        if self.kind != other.kind {
            return false;
        }
        self.origins.extend(other.origins.iter().copied());
        self.origins.sort_by(StructuralOrd::structural_cmp);
        self.origins.dedup();
        true
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WithOrigin<T> {
    pub(crate) value: T,
    origin: Origin,
    additional_origins: Vec<Origin>,
}

impl<T> WithOrigin<T> {
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
        std::iter::once(self.origin).chain(self.additional_origins.iter().copied())
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

impl<T: MergeEvidence + PartialEq> MergeEvidence for WithOrigin<T> {
    fn merge_evidence(&mut self, other: &Self) -> bool {
        if self.value != other.value {
            return false;
        }
        let _ = self.value.merge_evidence(&other.value);
        (self.origin, self.additional_origins) = normalize_origins(
            self.origin,
            self.additional_origins
                .iter()
                .copied()
                .chain(other.origins()),
        );
        true
    }
}

impl<T: Serialize> Serialize for WithOrigin<T> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let spans: Vec<_> = self.origins().map(|origin| origin.span).collect();
        let mut state = serializer.serialize_struct("WithOrigin", 2)?;
        state.serialize_field("value", &self.value)?;
        state.serialize_field("spans", &spans)?;
        state.end()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct InstalledAppsValue {
    pub(crate) apps: Vec<WithOrigin<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum InstalledAppEvidence {
    Known(WithOrigin<String>),
    Issue(SettingIssue),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct OrderedInstalledApps {
    pub(crate) evidence: Vec<InstalledAppEvidence>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct PartialInstalledApps {
    pub(crate) apps: OrderedInstalledApps,
}

pub(crate) type InstalledAppsAlternatives =
    SettingAlternatives<InstalledAppsValue, PartialInstalledApps>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct TemplatesValue {
    pub(crate) backends: Vec<TemplateBackend>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TemplateListEvidence {
    Backend(Box<PartialTemplateBackend>),
    Issue(SettingIssue),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct OrderedTemplateList {
    pub(crate) evidence: Vec<TemplateListEvidence>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct PartialTemplates {
    pub(crate) templates: OrderedTemplateList,
}

pub(crate) type TemplateAlternatives = SettingAlternatives<TemplatesValue, PartialTemplates>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct TemplateBackend {
    pub(crate) backend: WithOrigin<String>,
    pub(crate) dirs: Vec<WithOrigin<EvaluatedPath>>,
    pub(crate) app_dirs: Option<WithOrigin<bool>>,
    pub(crate) libraries: Vec<(String, WithOrigin<PythonModuleName>)>,
    pub(crate) builtins: Vec<WithOrigin<PythonModuleName>>,
    pub(crate) context_processors: Vec<WithOrigin<TemplateContextProcessorPath>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PathListEvidence {
    Known(WithOrigin<EvaluatedPath>),
    Issue(SettingIssue),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct OrderedPathList {
    pub(crate) evidence: Vec<PathListEvidence>,
}

impl OrderedPathList {
    pub(crate) fn new() -> Self {
        Self {
            evidence: Vec::new(),
        }
    }

    pub(crate) fn push_known(&mut self, path: WithOrigin<EvaluatedPath>) {
        self.evidence.push(PathListEvidence::Known(path));
    }

    pub(crate) fn push_issue(&mut self, issue: SettingIssue) {
        self.evidence.push(PathListEvidence::Issue(issue));
    }

    pub(crate) fn extend_issues(&mut self, issues: impl IntoIterator<Item = SettingIssue>) {
        self.evidence
            .extend(issues.into_iter().map(PathListEvidence::Issue));
    }

    fn has_issues(&self) -> bool {
        self.evidence
            .iter()
            .any(|evidence| matches!(evidence, PathListEvidence::Issue(_)))
    }

    fn issues(&self) -> impl Iterator<Item = &SettingIssue> {
        self.evidence.iter().filter_map(|evidence| match evidence {
            PathListEvidence::Known(_) => None,
            PathListEvidence::Issue(issue) => Some(issue),
        })
    }

    pub(crate) fn into_known(self) -> Vec<WithOrigin<EvaluatedPath>> {
        self.evidence
            .into_iter()
            .filter_map(|evidence| match evidence {
                PathListEvidence::Known(path) => Some(path),
                PathListEvidence::Issue(_) => None,
            })
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct PartialSettingField<T> {
    pub(crate) known: T,
    pub(crate) issues: Vec<SettingIssue>,
}

impl<T> PartialSettingField<T> {
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
    pub(super) correlation: BranchConstraints,
    pub(crate) backend: PartialSettingField<Option<WithOrigin<String>>>,
    pub(crate) dirs: OrderedPathList,
    pub(crate) app_dirs: PartialSettingField<Option<WithOrigin<bool>>>,
    pub(crate) options: PartialSettingField<()>,
    pub(crate) libraries: PartialSettingField<Vec<(String, WithOrigin<PythonModuleName>)>>,
    pub(crate) builtins: PartialSettingField<Vec<WithOrigin<PythonModuleName>>>,
    pub(crate) context_processors:
        PartialSettingField<Vec<WithOrigin<TemplateContextProcessorPath>>>,
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
    pub(crate) templates: TemplateAlternatives,
}

pub(crate) struct FeasibleConfiguration<'a> {
    pub(crate) installed_apps: &'a SettingCase<InstalledAppsValue, PartialInstalledApps>,
    pub(crate) templates: &'a SettingCase<TemplatesValue, PartialTemplates>,
}

impl DjangoSettings {
    pub(crate) fn feasible_configurations(&self) -> Vec<FeasibleConfiguration<'_>> {
        let mut configurations = Vec::new();
        for (installed_apps, app_correlation) in self.installed_apps.cases_with_correlations() {
            for (templates, template_correlation) in self.templates.cases_with_correlations() {
                if app_correlation.compatible_with(template_correlation) {
                    configurations.push(FeasibleConfiguration {
                        installed_apps,
                        templates,
                    });
                }
            }
        }
        configurations
    }

    pub(crate) fn unreadable() -> Self {
        let issue = SettingIssue {
            kind: SettingIssueKind::Unreadable,
            origins: Vec::new(),
        };
        Self {
            installed_apps: SettingAlternatives::new(vec![SettingCase::Dynamic(
                PartialInstalledApps {
                    apps: OrderedInstalledApps {
                        evidence: vec![InstalledAppEvidence::Issue(issue.clone())],
                    },
                },
            )]),
            templates: SettingAlternatives::new(vec![SettingCase::Dynamic(PartialTemplates {
                templates: OrderedTemplateList {
                    evidence: vec![TemplateListEvidence::Issue(issue)],
                },
            })]),
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
#[serde(rename_all = "lowercase")]
pub(crate) enum EvaluatedPath {
    Resolved(Utf8PathBuf),
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
            fn merge_evidence(&mut self, other: &Self) -> bool {
                let mut merged = self.clone();
                if true $(&& merged.$field.merge_evidence(&other.$field))+ {
                    *self = merged;
                    true
                } else {
                    false
                }
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
    EvaluatedPath,
    TemplateContextProcessorPath,
);

merge_struct_fields!(InstalledAppsValue { apps });
impl MergeEvidence for InstalledAppEvidence {
    fn merge_evidence(&mut self, other: &Self) -> bool {
        match (self, other) {
            (Self::Known(left), Self::Known(right)) => left.merge_evidence(right),
            (Self::Issue(left), Self::Issue(right)) => left.merge_evidence(right),
            (Self::Known(_), Self::Issue(_)) | (Self::Issue(_), Self::Known(_)) => false,
        }
    }
}
merge_struct_fields!(OrderedInstalledApps { evidence });
merge_struct_fields!(PartialInstalledApps { apps });
impl MergeDynamicEvidence for PartialInstalledApps {
    fn merge_dynamic_evidence(&mut self, other: Self) {
        for evidence in other.apps.evidence {
            if let InstalledAppEvidence::Issue(additional) = evidence {
                let mut issues = self.apps.evidence.iter_mut().filter_map(|evidence| {
                    if let InstalledAppEvidence::Issue(issue) = evidence {
                        Some(issue)
                    } else {
                        None
                    }
                });
                if let Some(existing) = issues.find(|issue| issue.kind == additional.kind) {
                    let _ = existing.merge_evidence(&additional);
                } else {
                    self.apps
                        .evidence
                        .push(InstalledAppEvidence::Issue(additional));
                }
            }
        }
    }
}
impl MergeEvidence for TemplatesValue {
    fn merge_evidence(&mut self, other: &Self) -> bool {
        if self.backends.len() != other.backends.len()
            || self
                .backends
                .iter()
                .zip(&other.backends)
                .any(|(left, right)| !same_path_origin_files(&left.dirs, &right.dirs))
        {
            return false;
        }
        self.backends.merge_evidence(&other.backends)
    }
}
impl MergeEvidence for TemplateListEvidence {
    fn merge_evidence(&mut self, other: &Self) -> bool {
        match (self, other) {
            (Self::Backend(left), Self::Backend(right)) => left.merge_evidence(right),
            (Self::Issue(left), Self::Issue(right)) => left.merge_evidence(right),
            (Self::Backend(_), Self::Issue(_)) | (Self::Issue(_), Self::Backend(_)) => false,
        }
    }
}
merge_struct_fields!(OrderedTemplateList { evidence });
merge_struct_fields!(PartialTemplates { templates });
impl MergeDynamicEvidence for PartialTemplates {
    fn merge_dynamic_evidence(&mut self, other: Self) {
        for evidence in other.templates.evidence {
            if let TemplateListEvidence::Issue(additional) = evidence {
                let existing = self.templates.evidence.iter_mut().find_map(|evidence| {
                    if let TemplateListEvidence::Issue(issue) = evidence
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
                    self.templates
                        .evidence
                        .push(TemplateListEvidence::Issue(additional));
                }
            }
        }
    }
}
impl MergeEvidence for TemplateBackend {
    fn merge_evidence(&mut self, other: &Self) -> bool {
        if !same_path_origin_files(&self.dirs, &other.dirs) {
            return false;
        }
        let mut merged = self.clone();
        if merged.backend.merge_evidence(&other.backend)
            && merged.dirs.merge_evidence(&other.dirs)
            && merged.app_dirs.merge_evidence(&other.app_dirs)
            && merged.libraries.merge_evidence(&other.libraries)
            && merged.builtins.merge_evidence(&other.builtins)
            && merged
                .context_processors
                .merge_evidence(&other.context_processors)
        {
            *self = merged;
            true
        } else {
            false
        }
    }
}
impl MergeEvidence for PathListEvidence {
    fn merge_evidence(&mut self, other: &Self) -> bool {
        match (self, other) {
            (Self::Known(left), Self::Known(right)) => left.merge_evidence(right),
            (Self::Issue(left), Self::Issue(right)) => left.merge_evidence(right),
            (Self::Known(_), Self::Issue(_)) | (Self::Issue(_), Self::Known(_)) => false,
        }
    }
}
impl MergeEvidence for OrderedPathList {
    fn merge_evidence(&mut self, other: &Self) -> bool {
        if self.evidence.len() != other.evidence.len()
            || self
                .evidence
                .iter()
                .zip(&other.evidence)
                .any(|(left, right)| match (left, right) {
                    (PathListEvidence::Known(left), PathListEvidence::Known(right)) => {
                        !same_origin_files(left.origins(), right.origins())
                    }
                    (PathListEvidence::Issue(left), PathListEvidence::Issue(right)) => {
                        !same_origin_files(
                            left.origins.iter().copied(),
                            right.origins.iter().copied(),
                        )
                    }
                    _ => true,
                })
        {
            return false;
        }
        self.evidence.merge_evidence(&other.evidence)
    }
}
impl<T: MergeEvidence> MergeEvidence for PartialSettingField<T> {
    fn merge_evidence(&mut self, other: &Self) -> bool {
        let mut merged = self.clone();
        if merged.known.merge_evidence(&other.known) && merged.issues.merge_evidence(&other.issues)
        {
            *self = merged;
            true
        } else {
            false
        }
    }
}
impl MergeEvidence for BranchConstraints {
    fn merge_evidence(&mut self, other: &Self) -> bool {
        self.merge(other.clone());
        true
    }
}
merge_struct_fields!(PartialTemplateBackend {
    correlation,
    backend,
    dirs,
    app_dirs,
    options,
    libraries,
    builtins,
    context_processors,
});
fn same_path_origin_files(
    left: &[WithOrigin<EvaluatedPath>],
    right: &[WithOrigin<EvaluatedPath>],
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
    use djls_source::Span;
    use salsa::plumbing::FromId as _;

    use super::MergeEvidence;
    use super::SettingIssue;
    use super::SettingIssueKind;
    use super::WithOrigin;

    fn origin(start: u32) -> djls_source::Origin {
        // SAFETY: The test index is below `salsa::Id::MAX_U32`; this synthetic
        // file is used only as an opaque identity and is never read.
        let file = File::from_id(unsafe { salsa::Id::from_index(0) });
        djls_source::Origin::new(file, Span::new(start, 1))
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
    fn with_origin_one_origin_accessors_and_serialization_are_total() {
        let first = origin(1);
        let value = WithOrigin::new("value".to_string(), first, []);

        assert_eq!(value.origin(), first);
        assert_eq!(value.origins().collect::<Vec<_>>(), [first]);
        assert_eq!(
            serde_json::to_value(&value).unwrap(),
            serde_json::json!({
                "value": "value",
                "spans": [first.span],
            })
        );
    }

    #[test]
    fn with_origin_construction_normalizes_and_deduplicates_origins() {
        let first = origin(1);
        let second = origin(2);
        let value = WithOrigin::new("value".to_string(), second, [second, first, second]);

        assert_eq!(value.origin(), first);
        assert_eq!(value.origins().collect::<Vec<_>>(), [first, second]);
        assert_eq!(
            serde_json::to_value(&value).unwrap(),
            serde_json::json!({
                "value": "value",
                "spans": [first.span, second.span],
            })
        );
    }

    #[test]
    fn with_origin_merge_is_reversed_and_idempotent() {
        let first = origin(1);
        let second = origin(2);
        let first_value = WithOrigin::new("value".to_string(), first, []);
        let second_value = WithOrigin::new("value".to_string(), second, []);

        let mut forward = second_value.clone();
        assert!(forward.merge_evidence(&first_value));
        let mut reversed = first_value;
        assert!(reversed.merge_evidence(&second_value));
        assert_eq!(forward, reversed);
        assert_eq!(forward.origins().collect::<Vec<_>>(), [first, second]);
        assert_eq!(
            serde_json::to_value(&forward).unwrap(),
            serde_json::to_value(&reversed).unwrap()
        );

        let merged = forward.clone();
        assert!(forward.merge_evidence(&merged));
        assert_eq!(forward, merged);
    }
}
