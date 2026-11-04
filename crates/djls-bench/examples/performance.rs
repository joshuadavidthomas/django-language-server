//! Native wall-time samples for the nightly page, independent of `CodSpeed` instrumentation.

use std::error::Error;
use std::hint::black_box;
use std::time::Instant;

use djls_bench::Keystroke;
use djls_bench::RealProject;
use djls_testing::Corpus;

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args().skip(1);
    let name = args.next().ok_or("expected a corpus application name")?;
    let cold_samples: usize = args.next().as_deref().unwrap_or("5").parse()?;
    let edit_samples: usize = args.next().as_deref().unwrap_or("100").parse()?;
    if cold_samples == 0 || edit_samples == 0 || args.next().is_some() {
        return Err(
            "expected NAME [positive cold sample count] [positive edit sample count]".into(),
        );
    }
    let project = RealProject::load(&name)?.ok_or("project environment is not synchronized")?;
    let mut cold = Vec::with_capacity(cold_samples);
    let mut cold_diagnostics = Vec::with_capacity(cold_samples);
    for _ in 0..cold_samples {
        // Match the existing Divan measurement: empty host and database destruction are
        // untimed. A fresh database is not a cold filesystem cache or a new process.
        let mut host = project.host();
        let start = Instant::now();
        project.open(&mut host);
        let diagnostics = black_box(project.check_templates(&host));
        cold.push(start.elapsed().as_nanos());
        cold_diagnostics.push(diagnostics);
    }
    let mut keystroke = Keystroke::open(&project);
    let mut edits = Vec::with_capacity(edit_samples);
    let mut edit_diagnostics = Vec::with_capacity(edit_samples);
    for _ in 0..edit_samples {
        let start = Instant::now();
        let diagnostics = black_box(keystroke.type_and_check());
        edits.push(start.elapsed().as_nanos());
        edit_diagnostics.push(diagnostics);
    }
    let descriptor = Corpus::require()?.environment_project(&name)?;
    println!(
        "{}",
        serde_json::json!({
            "schema_version": 1,
            "app": name,
            "templates": project.templates.len(),
            "project": {
                "root": descriptor.project_root,
                "templates": project.templates,
                "initialization_options": {
                    "venv_path": descriptor.environment_root,
                    "django_settings_module": descriptor.django_settings_module,
                    "pythonpath": descriptor.source_roots,
                },
            },
            "cold_start_check_templates": {"samples_ns": cold, "diagnostics": cold_diagnostics},
            "keystroke_diagnostics": {"samples_ns": edits, "diagnostics": edit_diagnostics},
        })
    );
    Ok(())
}
