use camino::Utf8PathBuf;
use djls_source::Origin;
use serde::Serialize;
use serde::ser::SerializeStruct;

use crate::ExtractionStatus;
use crate::python::InvalidModuleName;
use crate::python::ParseStatus;
use crate::python::PythonModuleName;

const DJANGO_TEMPLATES_BACKEND: &str = "django.template.backends.django.DjangoTemplates";

/// Observed values for one extracted Django setting.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct SettingValues<T> {
    pub(crate) values: Vec<T>,
    pub(crate) extraction: ExtractionStatus,
}

impl<T> Default for SettingValues<T> {
    fn default() -> Self {
        Self::partial()
    }
}

impl<T> SettingValues<T> {
    pub(crate) fn partial() -> Self {
        Self::with_extraction(Vec::new(), ExtractionStatus::Partial)
    }

    pub(crate) fn with_extraction(values: Vec<T>, extraction: ExtractionStatus) -> Self {
        Self { values, extraction }
    }

    #[must_use]
    pub(crate) fn is_fully_extracted(&self) -> bool {
        self.extraction == ExtractionStatus::Complete
    }

    pub(crate) fn mark_partial(&mut self) {
        self.extraction = ExtractionStatus::Partial;
    }
}

pub(crate) type InstalledAppsSetting = SettingValues<String>;

/// The statically extracted subset of Django's `TEMPLATES` setting.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct TemplateSettings {
    pub(crate) backends: Vec<TemplateBackend>,
    pub(crate) extraction: ExtractionStatus,
}

impl Default for TemplateSettings {
    fn default() -> Self {
        Self::partial()
    }
}

impl TemplateSettings {
    pub(crate) fn partial() -> Self {
        Self {
            backends: Vec::new(),
            extraction: ExtractionStatus::Partial,
        }
    }

    #[must_use]
    pub(crate) fn is_fully_extracted(&self) -> bool {
        self.extraction == ExtractionStatus::Complete
    }

    pub(crate) fn mark_partial(&mut self) {
        self.extraction = ExtractionStatus::Partial;
    }
}

/// One entry in Django's `TEMPLATES` setting.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct TemplateBackend {
    pub(crate) backend: Option<String>,
    pub(crate) dirs: Vec<EvaluatedPath>,
    pub(crate) app_dirs: Option<bool>,
    pub(crate) libraries: Vec<(String, PythonModuleName)>,
    pub(crate) builtins: Vec<PythonModuleName>,
    pub(crate) context_processors: Vec<Originated<TemplateContextProcessorPath>>,
    pub(crate) extraction: ExtractionStatus,
}

impl Default for TemplateBackend {
    fn default() -> Self {
        Self {
            backend: None,
            dirs: Vec::new(),
            app_dirs: None,
            libraries: Vec::new(),
            builtins: Vec::new(),
            context_processors: Vec::new(),
            extraction: ExtractionStatus::Complete,
        }
    }
}

impl TemplateBackend {
    #[must_use]
    pub(crate) fn is_fully_extracted(&self) -> bool {
        self.extraction == ExtractionStatus::Complete
    }

    #[must_use]
    pub(crate) fn is_django_templates_backend(&self, backend_count: usize) -> bool {
        match self.backend.as_deref() {
            Some(DJANGO_TEMPLATES_BACKEND) => true,
            None if backend_count == 1 => true,
            _ => false,
        }
    }

    pub(crate) fn mark_partial(&mut self) {
        self.extraction = ExtractionStatus::Partial;
    }
}

/// A dotted context processor callable path from `TEMPLATES[*]["OPTIONS"]`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
pub(crate) struct TemplateContextProcessorPath(String);

impl TemplateContextProcessorPath {
    pub(crate) fn parse(path: &str) -> Result<Self, InvalidModuleName> {
        let name = PythonModuleName::parse(path)?;
        Ok(Self(name.into_string()))
    }

    #[must_use]
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

pub(crate) type ScalarSetting<T> = SettingValues<Originated<T>>;

/// The statically extracted subset of Django's staticfiles settings.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub(crate) struct StaticFilesSettings {
    pub(crate) static_url: ScalarSetting<String>,
    pub(crate) static_root: ScalarSetting<EvaluatedPath>,
    pub(crate) staticfiles_dirs: StaticFilesDirsSetting,
}

pub(crate) type StaticFilesDirsSetting = SettingValues<Originated<EvaluatedPath>>;

/// The statically extracted subset of a Django settings module.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub(crate) struct DjangoSettings {
    pub(crate) parse_status: ParseStatus,
    pub(crate) installed_apps: InstalledAppsSetting,
    pub(crate) templates: TemplateSettings,
    pub(crate) staticfiles: StaticFilesSettings,
}

/// A path expression evaluated against the settings file's own location.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum EvaluatedPath {
    Resolved(Utf8PathBuf),
    Unknown,
}

/// A settings value paired with the file and span where it was born.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Originated<T> {
    value: T,
    origin: Origin,
}

impl<T> Originated<T> {
    pub(crate) fn new(value: T, origin: Origin) -> Self {
        Self { value, origin }
    }

    #[must_use]
    pub(crate) fn value(&self) -> &T {
        &self.value
    }

    #[must_use]
    pub(crate) fn origin(&self) -> Origin {
        self.origin
    }
}

impl<T: Serialize> Serialize for Originated<T> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let origin = self.origin();
        let mut state = serializer.serialize_struct("Originated", 2)?;
        state.serialize_field("value", &self.value)?;
        state.serialize_field("span", &origin.span)?;
        state.end()
    }
}
