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
