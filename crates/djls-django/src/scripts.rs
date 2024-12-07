use crate::templates::TemplateTag;
use djls_python::{include_script, ScriptRunner};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct DjangoSetup {
    apps: Vec<String>,
    tags: Vec<TemplateTag>,
}

impl ScriptRunner for DjangoSetup {
    const SCRIPT: &'static str = include_script!("django_setup");
}

impl DjangoSetup {
    pub fn apps(&self) -> &[String] {
        &self.apps
    }

    pub fn tags(&self) -> &[TemplateTag] {
        &self.tags
    }
}
