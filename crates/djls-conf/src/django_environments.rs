use serde::Deserialize;

#[derive(Debug, Deserialize, Default, PartialEq, Eq, Clone)]
pub struct DjangoEnvironmentConfig {
    root: String,
    django_settings_module: Option<String>,
}

impl DjangoEnvironmentConfig {
    #[must_use]
    pub fn root(&self) -> &str {
        self.root.trim()
    }

    #[must_use]
    pub fn django_settings_module(&self) -> Option<&str> {
        self.django_settings_module
            .as_deref()
            .map(str::trim)
            .filter(|module| !module.is_empty())
    }
}

#[cfg(test)]
impl DjangoEnvironmentConfig {
    pub(crate) fn new(root: impl Into<String>, django_settings_module: Option<String>) -> Self {
        Self {
            root: root.into(),
            django_settings_module,
        }
    }
}
