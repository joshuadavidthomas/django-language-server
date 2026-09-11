use camino::Utf8Path;

#[path = "support/corpus_settings.rs"]
mod corpus_settings_support;

use corpus_settings_support::redact_repo_root;

#[test]
fn repo_root_redaction_rewrites_nested_string_values() {
    let mut value = serde_json::json!({
        "path": "/corpus/repo/templates",
        "nested": ["/corpus/repo", "unchanged"],
        "prefix_collision": "/corpus/repository/templates",
    });

    redact_repo_root(&mut value, Utf8Path::new("/corpus/repo"));

    assert_eq!(
        value,
        serde_json::json!({
            "path": "${REPO}/templates",
            "nested": ["${REPO}", "unchanged"],
            "prefix_collision": "/corpus/repository/templates",
        })
    );

    let mut windows_value = serde_json::json!({
        "path": r"C:\corpus\repo\templates",
        "prefix_collision": r"C:\corpus\repository\templates",
    });
    redact_repo_root(&mut windows_value, Utf8Path::new(r"C:\corpus\repo"));
    assert_eq!(
        windows_value,
        serde_json::json!({
            "path": "${REPO}/templates",
            "prefix_collision": r"C:\corpus\repository\templates",
        })
    );
}
