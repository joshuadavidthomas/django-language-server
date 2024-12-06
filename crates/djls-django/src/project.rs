use crate::django::Apps;
        &self.python_env
    }

    fn settings_module(&self) -> &String {
        &self.settings_module
    }
}

impl fmt::Display for DjangoProject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Django Project")?;
        writeln!(f, "Settings Module: {}", self.settings_module)?;
        write!(f, "{}", self.installed_apps)?;
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ProjectError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON parsing error: {0}")]
    Json(#[from] serde_json::Error),
}
