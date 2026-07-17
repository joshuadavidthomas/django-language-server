use camino::Utf8PathBuf;
use djls_source::Origin;
use serde::Serialize;
use serde::ser::SerializeStruct;

use crate::python::InvalidModuleName;
use crate::python::PythonModuleName;
use crate::python::evaluation::BranchConstraints;
use crate::python::evaluation::origin_sort_key;

const DJANGO_TEMPLATES_BACKEND: &str = "django.template.backends.django.DjangoTemplates";
pub(crate) const MAX_EXACT_SETTING_ALTERNATIVES: usize = 64;
const MAX_SETTING_ALTERNATIVES: usize = MAX_EXACT_SETTING_ALTERNATIVES + 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SettingCase<T, D, I> {
    Known(T),
    Unset,
    Dynamic(D),
    Malformed(I),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CorrelatedSettingCase<T, D, I> {
    case: SettingCase<T, D, I>,
    correlation: BranchConstraints,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SettingAlternatives<T, D, I> {
    cases: Vec<CorrelatedSettingCase<T, D, I>>,
}

impl<T, D, I> Serialize for SettingAlternatives<T, D, I>
where
    T: Serialize,
    D: Serialize,
    I: Serialize,
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

impl<T, D, I> SettingAlternatives<T, D, I>
where
    T: MergeEvidence,
    D: MergeEvidence + MergeDynamicEvidence,
    I: MergeEvidence,
{
    fn new(cases: Vec<SettingCase<T, D, I>>) -> Self {
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

    pub(super) fn from_correlated(cases: Vec<(SettingCase<T, D, I>, BranchConstraints)>) -> Self {
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

    pub(crate) fn iter(&self) -> impl ExactSizeIterator<Item = &SettingCase<T, D, I>> {
        self.cases.iter().map(|case| &case.case)
    }

    fn cases_with_correlations(
        &self,
    ) -> impl Iterator<Item = (&SettingCase<T, D, I>, &BranchConstraints)> {
        self.cases
            .iter()
            .map(|case| (&case.case, &case.correlation))
    }

    pub(crate) fn add(&mut self, case: SettingCase<T, D, I>) {
        self.add_with_correlation(case, BranchConstraints::unconstrained());
    }

    fn add_with_correlation(&mut self, case: SettingCase<T, D, I>, correlation: BranchConstraints) {
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

impl<T, D, I> MergeEvidence for SettingAlternatives<T, D, I>
where
    T: MergeEvidence,
    D: MergeEvidence + MergeDynamicEvidence,
    I: MergeEvidence,
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

impl<T: MergeEvidence, D: MergeEvidence, I: MergeEvidence> MergeEvidence for SettingCase<T, D, I> {
    fn merge_evidence(&mut self, other: &Self) -> bool {
        match (self, other) {
            (Self::Known(left), Self::Known(right)) => left.merge_evidence(right),
            (Self::Unset, Self::Unset) => true,
            (Self::Dynamic(left), Self::Dynamic(right)) => left.merge_evidence(right),
            (Self::Malformed(left), Self::Malformed(right)) => left.merge_evidence(right),
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
        self.origins.sort_by_key(origin_sort_key);
        self.origins.dedup();
        true
    }
}

fn merge_issue_evidence(issues: &mut Vec<SettingIssue>, additional: SettingIssue) {
    if let Some(existing) = issues
        .iter_mut()
        .find(|existing| existing.kind == additional.kind)
    {
        let _ = existing.merge_evidence(&additional);
    } else {
        issues.push(additional);
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
    pub(crate) origins: Vec<Origin>,
}

impl<T> WithOrigin<T> {
    pub(crate) fn new(value: T, origins: Vec<Origin>) -> Self {
        Self { value, origins }
    }

    pub(crate) fn value(&self) -> &T {
        &self.value
    }

    pub(crate) fn origin(&self) -> Origin {
        *self.origins.first().expect("known values have an origin")
    }
}

impl<T: MergeEvidence + PartialEq> MergeEvidence for WithOrigin<T> {
    fn merge_evidence(&mut self, other: &Self) -> bool {
        if self.value != other.value {
            return false;
        }
        let _ = self.value.merge_evidence(&other.value);
        for origin in &other.origins {
            if !self.origins.contains(origin) {
                self.origins.push(*origin);
            }
        }
        true
    }
}

impl<T: Serialize> Serialize for WithOrigin<T> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let spans: Vec<_> = self.origins.iter().map(|origin| origin.span).collect();
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
pub(crate) struct DynamicInstalledApps {
    pub(crate) apps: OrderedInstalledApps,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct MalformedInstalledApps {
    pub(crate) apps: OrderedInstalledApps,
}

pub(crate) type InstalledAppsAlternatives =
    SettingAlternatives<InstalledAppsValue, DynamicInstalledApps, MalformedInstalledApps>;

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

/// Preserve source-list identity across all template projections.
///
/// Every list element owns exactly one backend slot. Consumers may disagree about whether the
/// element contributes roots or libraries, but must not renumber later elements based on that
/// decision.
pub(crate) fn template_backend_evidence_slots(
    evidence: &[TemplateListEvidence],
) -> impl Iterator<Item = (usize, &TemplateListEvidence)> {
    evidence.iter().enumerate()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct OrderedTemplateList {
    pub(crate) evidence: Vec<TemplateListEvidence>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct DynamicTemplates {
    pub(crate) templates: OrderedTemplateList,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct MalformedTemplates {
    pub(crate) templates: OrderedTemplateList,
}

pub(crate) type TemplateAlternatives =
    SettingAlternatives<TemplatesValue, DynamicTemplates, MalformedTemplates>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct TemplateBackend {
    pub(crate) backend: WithOrigin<String>,
    pub(crate) dirs: Vec<WithOrigin<EvaluatedPath>>,
    pub(crate) app_dirs: Option<WithOrigin<bool>>,
    pub(crate) libraries: Vec<(String, WithOrigin<PythonModuleName>)>,
    pub(crate) builtins: Vec<WithOrigin<PythonModuleName>>,
    pub(crate) context_processors: Vec<WithOrigin<TemplateContextProcessorPath>>,
}

impl TemplateBackend {
    pub(crate) fn is_django_templates_backend(&self) -> bool {
        self.backend.value == DJANGO_TEMPLATES_BACKEND
    }
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

    fn from_known(paths: Vec<WithOrigin<EvaluatedPath>>) -> Self {
        Self {
            evidence: paths.into_iter().map(PathListEvidence::Known).collect(),
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

    pub(crate) fn has_issues(&self) -> bool {
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
    pub(crate) fn from_complete(backend: TemplateBackend) -> Self {
        Self {
            correlation: BranchConstraints::unconstrained(),
            backend: PartialSettingField::new(Some(backend.backend)),
            dirs: OrderedPathList::from_known(backend.dirs),
            app_dirs: PartialSettingField::new(backend.app_dirs),
            options: PartialSettingField::new(()),
            libraries: PartialSettingField::new(backend.libraries),
            builtins: PartialSettingField::new(backend.builtins),
            context_processors: PartialSettingField::new(backend.context_processors),
        }
    }

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
pub(crate) struct StaticUrl(pub(crate) String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StaticRoot {
    path: EvaluatedPath,
}

impl StaticRoot {
    pub(crate) fn new(path: EvaluatedPath) -> Self {
        Self { path }
    }
}

impl Serialize for StaticRoot {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.path.serialize(serializer)
    }
}

impl MergeEvidence for StaticRoot {
    fn merge_evidence(&mut self, other: &Self) -> bool {
        self == other
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct DynamicScalarSetting {
    pub(crate) issues: Vec<SettingIssue>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct MalformedScalarSetting {
    pub(crate) issues: Vec<SettingIssue>,
}

pub(crate) type StaticUrlAlternatives =
    SettingAlternatives<WithOrigin<StaticUrl>, DynamicScalarSetting, MalformedScalarSetting>;
pub(crate) type StaticRootAlternatives =
    SettingAlternatives<WithOrigin<StaticRoot>, DynamicScalarSetting, MalformedScalarSetting>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct StaticFilesDirsValue {
    pub(crate) dirs: Vec<WithOrigin<EvaluatedPath>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct DynamicStaticFilesDirs {
    pub(crate) paths: OrderedPathList,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct MalformedStaticFilesDirs {
    pub(crate) paths: OrderedPathList,
}

pub(crate) type StaticFilesDirsAlternatives =
    SettingAlternatives<StaticFilesDirsValue, DynamicStaticFilesDirs, MalformedStaticFilesDirs>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct StaticFilesSettings {
    pub(crate) static_url: StaticUrlAlternatives,
    pub(crate) static_root: StaticRootAlternatives,
    pub(crate) staticfiles_dirs: StaticFilesDirsAlternatives,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct DjangoSettings {
    pub(crate) installed_apps: InstalledAppsAlternatives,
    pub(crate) templates: TemplateAlternatives,
    pub(crate) staticfiles: StaticFilesSettings,
}

pub(crate) struct FeasibleConfiguration<'a> {
    pub(crate) installed_apps:
        &'a SettingCase<InstalledAppsValue, DynamicInstalledApps, MalformedInstalledApps>,
    pub(crate) templates: &'a SettingCase<TemplatesValue, DynamicTemplates, MalformedTemplates>,
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
                DynamicInstalledApps {
                    apps: OrderedInstalledApps {
                        evidence: vec![InstalledAppEvidence::Issue(issue.clone())],
                    },
                },
            )]),
            templates: SettingAlternatives::new(vec![SettingCase::Dynamic(DynamicTemplates {
                templates: OrderedTemplateList {
                    evidence: vec![TemplateListEvidence::Issue(issue.clone())],
                },
            })]),
            staticfiles: StaticFilesSettings {
                static_url: SettingAlternatives::new(vec![SettingCase::Dynamic(
                    DynamicScalarSetting {
                        issues: vec![issue.clone()],
                    },
                )]),
                static_root: SettingAlternatives::new(vec![SettingCase::Dynamic(
                    DynamicScalarSetting {
                        issues: vec![issue.clone()],
                    },
                )]),
                staticfiles_dirs: SettingAlternatives::new(vec![SettingCase::Dynamic(
                    DynamicStaticFilesDirs {
                        paths: OrderedPathList {
                            evidence: vec![PathListEvidence::Issue(issue)],
                        },
                    },
                )]),
            },
        }
    }
}

impl Default for DjangoSettings {
    fn default() -> Self {
        Self {
            installed_apps: SettingAlternatives::new(vec![SettingCase::Unset]),
            templates: SettingAlternatives::new(vec![SettingCase::Unset]),
            staticfiles: StaticFilesSettings {
                static_url: SettingAlternatives::new(vec![SettingCase::Unset]),
                static_root: SettingAlternatives::new(vec![SettingCase::Unset]),
                staticfiles_dirs: SettingAlternatives::new(vec![SettingCase::Unset]),
            },
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

    pub(crate) fn as_str(&self) -> &str {
        &self.0
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
    StaticUrl,
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
merge_struct_fields!(DynamicInstalledApps { apps });
impl MergeDynamicEvidence for DynamicInstalledApps {
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
merge_struct_fields!(MalformedInstalledApps { apps });
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
merge_struct_fields!(DynamicTemplates { templates });
impl MergeDynamicEvidence for DynamicTemplates {
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
merge_struct_fields!(MalformedTemplates { templates });
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
                        !same_origin_files(&left.origins, &right.origins)
                    }
                    (PathListEvidence::Issue(left), PathListEvidence::Issue(right)) => {
                        !same_origin_files(&left.origins, &right.origins)
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
merge_struct_fields!(DynamicScalarSetting { issues });
impl MergeDynamicEvidence for DynamicScalarSetting {
    fn merge_dynamic_evidence(&mut self, other: Self) {
        for issue in other.issues {
            merge_issue_evidence(&mut self.issues, issue);
        }
    }
}
merge_struct_fields!(MalformedScalarSetting { issues });
impl MergeEvidence for StaticFilesDirsValue {
    fn merge_evidence(&mut self, other: &Self) -> bool {
        if !same_path_origin_files(&self.dirs, &other.dirs) {
            return false;
        }
        self.dirs.merge_evidence(&other.dirs)
    }
}

fn same_path_origin_files(
    left: &[WithOrigin<EvaluatedPath>],
    right: &[WithOrigin<EvaluatedPath>],
) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .all(|(left, right)| same_origin_files(&left.origins, &right.origins))
}

fn same_origin_files(left: &[Origin], right: &[Origin]) -> bool {
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
merge_struct_fields!(DynamicStaticFilesDirs { paths });
impl MergeDynamicEvidence for DynamicStaticFilesDirs {
    fn merge_dynamic_evidence(&mut self, other: Self) {
        for evidence in other.paths.evidence {
            if let PathListEvidence::Issue(additional) = evidence {
                let existing = self.paths.evidence.iter_mut().find_map(|evidence| {
                    if let PathListEvidence::Issue(issue) = evidence
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
                    self.paths
                        .evidence
                        .push(PathListEvidence::Issue(additional));
                }
            }
        }
    }
}
merge_struct_fields!(MalformedStaticFilesDirs { paths });
merge_struct_fields!(StaticFilesSettings {
    static_url,
    static_root,
    staticfiles_dirs,
});
merge_struct_fields!(DjangoSettings {
    installed_apps,
    templates,
    staticfiles,
});

#[cfg(test)]
mod tests {
    use djls_source::File;
    use djls_source::Span;
    use salsa::plumbing::FromId as _;

    use super::MergeEvidence;
    use super::SettingIssue;
    use super::SettingIssueKind;

    fn origin(start: u32) -> djls_source::Origin {
        // SAFETY: The test index is below `salsa::Id::MAX_U32`; this synthetic
        // file is used only as an opaque identity and is never read.
        let file = File::from_id(unsafe { salsa::Id::from_index(0) });
        djls_source::Origin::new(file, Span::new(start, 1))
    }

    #[test]
    fn canonical_unknown_origins_setting_issue_merge_is_reversed_and_idempotent() {
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
}
