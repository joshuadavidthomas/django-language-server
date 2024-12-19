use anyhow::Result;
use clap::{Parser, Subcommand};
use djls_ipc::all_message_schemas;
use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;
use std::process::{Command as ProcCommand, Stdio};
use tempfile::NamedTempFile;

#[derive(Parser)]
#[command(name = "djls-dev")]
#[command(version, about, long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Generate Pydantic models
    Pydantic {
        /// Optional output file (prints to stdout if not provided)
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
}

#[tokio::main]
async fn main() -> Result<ExitCode> {
    let cli = Cli::parse();
    match cli.command {
        Command::Pydantic { output } => {
            if let Some(path) = &output {
                std::fs::create_dir_all(path)?;
            }

            let (source_file, schemas) = all_message_schemas()?;

            for (name, schema) in schemas {
                let mut temp_file = NamedTempFile::new()?;
                serde_json::to_writer(&mut temp_file, &schema)?;
                temp_file.flush()?;

                let mut cmd_builder = ProcCommand::new("uv");
                cmd_builder.args([
                    "run",
                    "--with",
                    "datamodel-code-generator",
                    "datamodel-codegen",
                    "--input-file-type",
                    "jsonschema",
                    "--output-model-type",
                    "pydantic_v2.BaseModel",
                    "--use-title-as-name",
                    "--disable-timestamp",
                    "--target-python-version",
                    "3.12",
                    "--use-schema-description",
                    "--use-field-description",
                    "--reuse-model",
                    "--input",
                ]);

                cmd_builder.arg(temp_file.path());

                let output_path = if let Some(path) = &output {
                    let output_path = path.join(format!("{}.py", name));
                    cmd_builder.arg("--output").arg(&output_path);
                    Some(output_path)
                } else {
                    cmd_builder.stdout(Stdio::inherit());
                    None
                };

                let status = cmd_builder.spawn()?.wait()?;
                if !status.success() {
                    anyhow::bail!("Python generation failed for {}", name);
                }

                if let Some(output_path) = output_path {
                    let content = std::fs::read_to_string(&output_path)?;
                    let temp_filename = temp_file.path().file_name().unwrap().to_string_lossy();
                    let content = content.replace(
                        &format!("filename:  {}", temp_filename),
                        &format!("filename:  {}", source_file),
                    );
                    let lines: Vec<&str> = content.lines().collect();
                    let mut filtered_lines: Vec<&str> = Vec::new();
                    let mut skip_next = 0;

                    for line in lines {
                        if skip_next > 0 {
                            skip_next -= 1;
                            continue;
                        }

                        if line.contains(", RootModel") {
                            // Replace the line with one that only imports BaseModel
                            filtered_lines.push("from pydantic import BaseModel");
                            continue;
                        }

                        if line.contains("class Model(RootModel[Any]):")
                            || line.contains("root: Any")
                        {
                            skip_next = 3; // Skip the next two blank lines
                            continue;
                        }

                        filtered_lines.push(line);
                    }

                    let content = filtered_lines.join("\n");
                    std::fs::write(&output_path, content)?;
                }
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}
