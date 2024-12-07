use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
pub struct TemplateTag {
    pub name: String,
    pub library: String,
    #[serde(default)]
    pub doc: Option<String>,
}
