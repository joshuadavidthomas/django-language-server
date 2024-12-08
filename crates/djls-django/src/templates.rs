use serde::Deserialize;
use std::fmt;

#[derive(Clone, Debug, Deserialize)]
pub struct TemplateTag {
    name: String,
    library: String,
    doc: Option<String>,
}

impl fmt::Display for TemplateTag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let library = if self.library.is_empty() {
            "builtins"
        } else {
            &self.library
        };

        write!(f, "{} ({})", self.name, library)?;
        writeln!(f)?;

        if let Some(doc) = &self.doc {
            for line in doc.trim_end().split("\n") {
                writeln!(f, "{}", line)?;
            }
        }

        Ok(())
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct TemplateTags(Vec<TemplateTag>);

impl TemplateTags {
    pub fn tags(&self) -> &Vec<TemplateTag> {
        &self.0
    }

    fn iter(&self) -> impl Iterator<Item = &TemplateTag> {
        self.tags().iter()
    }

    pub fn filter_by_prefix<'a>(
        &'a self,
        prefix: &'a str,
    ) -> impl Iterator<Item = &'a TemplateTag> {
        self.iter().filter(move |tag| tag.name.starts_with(prefix))
    }

    pub fn get_by_name(&self, name: &str) -> Option<&TemplateTag> {
        self.iter().find(|tag| tag.name == name)
    }
}

impl fmt::Display for TemplateTags {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for tag in &self.0 {
            writeln!(f, "  {}", tag)?;
        }
        Ok(())
    }
}
