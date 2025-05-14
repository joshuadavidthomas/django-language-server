use std::ops::Deref;

use pyo3::prelude::*;
use pyo3::types::PyDict;
use pyo3::types::PyList;

#[derive(Debug, Default, Clone)]
pub struct TemplateTags(Vec<TemplateTag>);

impl Deref for TemplateTags {
    type Target = Vec<TemplateTag>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl TemplateTags {
    fn new() -> Self {
        Self(Vec::new())
    }

    fn process_library(
        module_name: &str,
        library: &Bound<'_, PyAny>,
        tags: &mut Vec<TemplateTag>,
    ) -> PyResult<()> {
        let tags_dict = library.getattr("tags")?;
        let dict = tags_dict.downcast::<PyDict>()?;

        for (key, value) in dict.iter() {
            let tag_name = key.extract::<String>()?;
            let doc = value.getattr("__doc__")?.extract().ok();

            let library_name = if module_name.is_empty() {
                "builtins".to_string()
            } else {
                module_name.split('.').next_back().unwrap_or("").to_string()
            };

            tags.push(TemplateTag::new(tag_name, library_name, doc));
        }
        Ok(())
    }

    pub fn from_python(py: Python) -> PyResult<TemplateTags> {
        let mut template_tags = TemplateTags::new();

        let engine = py
            .import("django.template.engine")?
            .getattr("Engine")?
            .call_method0("get_default")?;

        // Built-in template tags
        let builtins_attr = engine.getattr("template_builtins")?;
        let builtins = builtins_attr.downcast::<PyList>()?;
        for builtin in builtins {
            Self::process_library("", &builtin, &mut template_tags.0)?;
        }

        // Custom template libraries
        let libraries_attr = engine.getattr("template_libraries")?;
        let libraries = libraries_attr.downcast::<PyDict>()?;
        for (module_name, library) in libraries.iter() {
            let module_name = module_name.extract::<String>()?;
            Self::process_library(&module_name, &library, &mut template_tags.0)?;
        }

        Ok(template_tags)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateTag {
    name: String,
    library: String,
    doc: Option<String>,
}

impl TemplateTag {
    fn new(name: String, library: String, doc: Option<String>) -> Self {
        Self { name, library, doc }
    }

    pub fn name(&self) -> &String {
        &self.name
    }

    pub fn library(&self) -> &String {
        &self.library
    }

    pub fn doc(&self) -> Option<&String> {
        self.doc.as_ref()
    }
}
