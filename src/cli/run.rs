use crate::app::OrcApp;
use anyhow::Result;
use clap::Subcommand;

#[derive(Subcommand)]
pub enum RunCommand {
    Submit {
        run_id: i64,
        #[arg(long)]
        file: Option<String>,
    },
    /// Submit a git patch for a waiting manual run
    SubmitPatch {
        run_id: i64,
        /// Path to patch file (use - for stdin)
        patch_file: String,
    },
    Fail {
        run_id: i64,
        reason: Option<String>,
    },
}

pub fn run(command: RunCommand, db_path: &str) -> Result<()> {
    match command {
        RunCommand::Submit { run_id, file } => {
            let output = match file.as_deref() {
                Some(path) => std::fs::read_to_string(path)?,
                None => {
                    use std::io::Read;
                    let mut output = String::new();
                    std::io::stdin().read_to_string(&mut output)?;
                    output
                }
            };
            let task_id = OrcApp::open_global(db_path, ".")?.submit_manual_run(run_id, &output)?;
            println!(
                "Run {} completed; task {} moved to review.",
                run_id, task_id
            );
        }
        RunCommand::SubmitPatch { run_id, patch_file } => {
            let patch_content = if patch_file == "-" {
                use std::io::Read;
                let mut output = String::new();
                std::io::stdin().read_to_string(&mut output)?;
                output
            } else {
                std::fs::read_to_string(&patch_file).map_err(|e| {
                    anyhow::anyhow!("failed to read patch file '{}': {}", patch_file, e)
                })?
            };

            match OrcApp::open_global(db_path, ".")?.submit_patch(run_id, &patch_content) {
                Ok(outcome) => {
                    println!("Run {}", outcome.run_id);
                    println!("Patch: valid");
                    println!("Worktree: {}", outcome.worktree_path.display());
                    println!("Applied: yes\n");
                    println!("Validation: deferred to review");
                    println!("\nRun: completed");
                    println!("Task {}: review", outcome.task_id);
                }
                Err(e) => {
                    eprintln!("Submit patch failed: {:#}", e);
                    return Err(e);
                }
            }
        }
        RunCommand::Fail { run_id, reason } => {
            let task_id = OrcApp::open_global(db_path, ".")?
                .fail_manual_run(run_id, reason.as_deref().unwrap_or("manual run failed"))?;
            println!("Run {} failed; task {} moved to blocked.", run_id, task_id);
        }
    }

    Ok(())
}
