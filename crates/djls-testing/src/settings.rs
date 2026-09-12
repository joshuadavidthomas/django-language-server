use std::collections::BTreeMap;

/// Django template backend settings used by Rust test fixtures.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectSettings {
    pub installed_apps: Vec<String>,
    pub dirs: Vec<String>,
    pub app_dirs: bool,
    pub builtins: Vec<String>,
    pub libraries: BTreeMap<String, String>,
}

impl Default for ProjectSettings {
    fn default() -> Self {
        Self {
            installed_apps: Vec::new(),
            dirs: vec!["/templates".to_string()],
            app_dirs: false,
            builtins: Vec::new(),
            libraries: BTreeMap::new(),
        }
    }
}

impl ProjectSettings {
    #[must_use]
    pub fn settings_py(&self) -> String {
        let installed_apps = python_list(&self.installed_apps);
        let dirs = python_list(&self.dirs);
        let builtins = python_list(&self.builtins);
        let libraries = self
            .libraries
            .iter()
            .map(|(name, module)| format!("{}: {}", python_string(name), python_string(module)))
            .collect::<Vec<_>>()
            .join(", ");
        let app_dirs = if self.app_dirs { "True" } else { "False" };

        format!(
            "INSTALLED_APPS = {installed_apps}\nTEMPLATES = [{{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': {dirs}, 'APP_DIRS': {app_dirs}, 'OPTIONS': {{'builtins': {builtins}, 'libraries': {{{libraries}}}}}}}]\n"
        )
    }
}

fn python_list(values: &[String]) -> String {
    let values = values
        .iter()
        .map(|value| python_string(value))
        .collect::<Vec<_>>()
        .join(", ");
    format!("[{values}]")
}

fn python_string(value: &str) -> String {
    format!("'{}'", value.replace('\\', "\\\\").replace('\'', "\\'"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_settings_python() {
        assert_eq!(
            ProjectSettings::default().settings_py(),
            "INSTALLED_APPS = []\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/templates'], 'APP_DIRS': False, 'OPTIONS': {'builtins': [], 'libraries': {}}}]\n"
        );

        let settings = ProjectSettings {
            installed_apps: vec!["django.contrib.admin".to_string()],
            dirs: vec!["/templates".to_string(), "/other".to_string()],
            app_dirs: true,
            builtins: vec!["custom_tags".to_string()],
            libraries: BTreeMap::from([("custom".to_string(), "custom_tags".to_string())]),
        };

        assert_eq!(
            settings.settings_py(),
            "INSTALLED_APPS = ['django.contrib.admin']\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/templates', '/other'], 'APP_DIRS': True, 'OPTIONS': {'builtins': ['custom_tags'], 'libraries': {'custom': 'custom_tags'}}}]\n"
        );
    }
}
