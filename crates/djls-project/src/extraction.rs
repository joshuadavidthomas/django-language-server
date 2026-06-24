//! Pure static recognition for Django project source.
//!
//! The extraction boundary is deliberately pure: callers provide Python source
//! text and answer star-import recursion through `SettingsSourceResolver`. This
//! module does not read files, resolve search paths, or depend on Salsa.

mod extractor;
mod paths;
pub(crate) mod registry;
mod settings;

pub use extractor::extract_settings;
pub use registry::RegistrationInfo;
pub use registry::RegistrationKind;
pub use registry::collect_registrations_from_body;
pub use settings::DjangoSettings;
pub use settings::InstalledAppsSetting;
pub use settings::SettingsSource;
pub use settings::SettingsSourceResolver;
pub use settings::SettingsStarImport;
pub use settings::StaticKnowledge;
pub use settings::TemplateBackend;
pub use settings::TemplateDirPath;
pub use settings::TemplateSettings;
