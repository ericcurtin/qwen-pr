mod git;
mod github;
mod monitor;

use anyhow::Result;
use clap::{Parser, Subcommand};

use git::{ensure_git_repo, get_current_branch, git_push_force};
use github::{create_pr, ensure_gh_cli, find_existing_pr, get_pr_url};
use monitor::monitor_and_fix;

#[derive(Parser)]
#[command(name = "qwen-pr")]
#[command(about = "Automated PR workflow with AI-powered build failure remediation")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Push, create/update PR, and monitor builds (auto-fix failures with qwen)
    Push,
}

fn run_push() -> Result<()> {
    // Pre-flight checks
    ensure_git_repo()?;
    ensure_gh_cli()?;

    let branch = get_current_branch()?;
    println!("Current branch: {}", branch);

    if branch == "main" || branch == "master" {
        anyhow::bail!("Cannot push directly to {}. Create a feature branch first.", branch);
    }

    // Force push
    git_push_force()?;

    // Find or create PR
    let pr_number = match find_existing_pr(&branch)? {
        Some(pr) => {
            println!("Found existing PR #{}", pr);
            pr
        }
        None => create_pr(&branch)?,
    };

    // Print PR URL
    let url = get_pr_url(pr_number)?;
    println!("PR URL: {}", url);

    // Monitor and fix
    monitor_and_fix(pr_number)?;

    println!("\n🎉 PR #{} is ready!", pr_number);
    Ok(())
}

fn main() {
    let cli = Cli::parse();

    let result = match cli.command {
        Commands::Push => run_push(),
    };

    if let Err(e) = result {
        eprintln!("Error: {:#}", e);
        std::process::exit(1);
    }
}
