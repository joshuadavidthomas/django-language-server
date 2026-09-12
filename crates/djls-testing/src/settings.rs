use std::collections::BTreeMap;

use serde::Deserialize;

/// Django template backend settings used by validation fixtures and mdtests.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]
pub struct ProjectSettings {
    pub dirs: Vec<String>,
    pub app_dirs: bool,
    pub builtins: Vec<String>,
    pub libraries: BTreeMap<String, String>,
    pub partial: bool,
}

impl Default for ProjectSettings {
    fn default() -> Self {
        Self {
            dirs: vec!["/templates".to_string()],
            app_dirs: false,
            builtins: Vec::new(),
            libraries: BTreeMap::new(),
            partial: false,
        }
    }
}

impl ProjectSettings {
    pub(crate) fn render_settings_py(&self) -> anyhow::Result<String> {
        let dirs = serde_json::to_string(&self.dirs)?;
        let builtins = serde_json::to_string(&self.builtins)?;
        let libraries = serde_json::to_string(&self.libraries)?;
        let app_dirs = if self.app_dirs { "True" } else { "False" };
        let partial = if self.partial {
            ", UNKNOWN: 'maybe'"
        } else {
            ""
        };

        Ok(format!(
            "INSTALLED_APPS = ['django.contrib.humanize']\nTEMPLATES = [{{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': {dirs}, 'APP_DIRS': {app_dirs}, 'OPTIONS': {{'builtins': {builtins}, 'libraries': {libraries}}}{partial}}}]\n"
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_settings_python() {
        let settings = ProjectSettings {
            dirs: vec!["/templates".to_string(), "/other".to_string()],
            app_dirs: true,
            builtins: vec!["custom_tags".to_string()],
            libraries: BTreeMap::from([("custom".to_string(), "custom_tags".to_string())]),
            partial: true,
        };

        assert_eq!(
            settings
                .render_settings_py()
                .expect("settings should render as Python"),
            "INSTALLED_APPS = ['django.contrib.humanize']\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [\"/templates\",\"/other\"], 'APP_DIRS': True, 'OPTIONS': {'builtins': [\"custom_tags\"], 'libraries': {\"custom\":\"custom_tags\"}}, UNKNOWN: 'maybe'}]\n"
        );
    }
}
