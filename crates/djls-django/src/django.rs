use std::fmt;

#[derive(Debug)]
pub struct App(String);

impl App {
    pub fn name(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for App {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug)]
pub struct Apps(Vec<App>);

impl Default for Apps {
    fn default() -> Self {
        Self(Vec::new())
    }
}

impl Apps {
    pub fn from_strings(apps: Vec<String>) -> Self {
        Self(apps.into_iter().map(App).collect())
    }

    pub fn has_app(&self, name: &str) -> bool {
        self.0.iter().any(|app| app.0 == name)
    }

    pub fn iter(&self) -> impl Iterator<Item = &App> {
        self.0.iter()
    }
}

impl fmt::Display for Apps {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Installed Apps:")?;
        for app in &self.0 {
            writeln!(f, "  {}", app)?;
        }
        Ok(())
    }
}
