use camino::Utf8Path;
use serde_json::Value;

pub fn redact_repo_root(value: &mut Value, repo_root: &Utf8Path) {
    match value {
        Value::String(text) => {
            let normalized_text = text.replace('\\', "/");
            let normalized_repo_root = repo_root.as_str().replace('\\', "/");
            if let Ok(relative) =
                Utf8Path::new(&normalized_text).strip_prefix(Utf8Path::new(&normalized_repo_root))
            {
                *text = if relative.as_str().is_empty() {
                    "${REPO}".to_string()
                } else {
                    format!("${{REPO}}/{relative}")
                };
            }
        }
        Value::Array(values) => {
            for value in values {
                redact_repo_root(value, repo_root);
            }
        }
        Value::Object(values) => {
            for value in values.values_mut() {
                redact_repo_root(value, repo_root);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}
