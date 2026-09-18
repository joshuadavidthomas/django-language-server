use std::sync::Arc;

use djls_conf::DiagnosticSeverity;
use djls_conf::Settings;
use djls_db::DjangoDatabase;
use djls_semantic::Db as _;
use djls_source::InMemoryFileSystem;

#[test]
fn diagnostics_configuration_is_owned_by_each_database_snapshot() {
    let settings: Settings = serde_json::from_value(serde_json::json!({
        "django_settings_module": "project.settings",
        "pythonpath": ["/project/vendor", "/project/apps"],
        "tagspecs": {
            "version": "0.6.0", "engine": "django",
            "libraries": [{"module": "app.templatetags.custom", "tags": [
                {"name": "panel", "type": "block", "end": {"name": "endpanel"}}
            ]}]
        },
        "diagnostics": {"severity": {"S": "warning", "S100": "off"}}
    }))
    .expect("settings should deserialize");
    let mut db = DjangoDatabase::new(Arc::new(InMemoryFileSystem::new()), &settings, None);
    let snapshot = db.clone();
    let original = db.diagnostics_config();
    assert_eq!(original, settings.diagnostics().clone());
    assert_eq!(original.get_severity("S100"), DiagnosticSeverity::Off);
    assert_eq!(original.get_severity("S101"), DiagnosticSeverity::Warning);

    let replacement: Settings = serde_json::from_value(serde_json::json!({
        "diagnostics": {"severity": {"S100": "hint"}}
    }))
    .expect("replacement settings should deserialize");
    db.apply_project_settings(replacement);
    assert_eq!(
        db.diagnostics_config().get_severity("S100"),
        DiagnosticSeverity::Hint
    );
    assert_eq!(
        db.diagnostics_config().get_severity("S101"),
        DiagnosticSeverity::Error
    );
    assert_eq!(snapshot.diagnostics_config(), original);
    assert_eq!(snapshot.settings(), settings);

    let mut detached = db.diagnostics_config();
    detached.set_severity("S100", DiagnosticSeverity::Error);
    assert_eq!(
        db.diagnostics_config().get_severity("S100"),
        DiagnosticSeverity::Hint
    );
}
