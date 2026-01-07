mod git;
mod github;
mod monitor;

use anyhow::Result;
use clap::{Parser, Subcommand};

use git::{ensure_git_repo, get_current_branch, git_amend, git_push_force, run_qwen_fix};
use github::{create_pr, ensure_gh_cli, find_existing_pr, get_pr_comments, get_pr_url, resolve_all_threads};
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
    Push {
        /// Address PR comments before monitoring builds
        #[arg(short, long)]
        comments: bool,

        /// Additional arguments to pass to git push (e.g., remote name)
        #[arg(trailing_var_arg = true)]
        args: Vec<String>,
    },
}

fn run_push(push_args: &[String], address_comments: bool) -> Result<()> {
    // Pre-flight checks
    ensure_git_repo()?;
    ensure_gh_cli()?;

    let branch = get_current_branch()?;
    println!("Current branch: {}", branch);

    if branch == "main" || branch == "master" {
        anyhow::bail!("Cannot push directly to {}. Create a feature branch first.", branch);
    }

    // Extract remote name if provided (first arg that doesn't start with -)
    let remote = push_args.first().filter(|a| !a.starts_with('-')).map(|s| s.as_str());

    // Force push
    git_push_force(push_args)?;

    // Find or create PR
    let pr_number = match find_existing_pr(&branch, remote)? {
        Some(pr) => {
            println!("Found existing PR #{}", pr);
            pr
        }
        None => create_pr(&branch, remote)?,
    };

    // Print PR URL
    let url = get_pr_url(pr_number)?;
    println!("PR URL: {}", url);

    // Address comments first if requested
    if address_comments {
        address_pr_comments(pr_number, push_args)?;
    }

    // Monitor and fix
    monitor_and_fix(pr_number, push_args)?;

    println!("\n🎉 PR #{} is ready!", pr_number);
    Ok(())
}

/// Address PR comments proactively before monitoring builds
fn address_pr_comments(pr_number: u64, push_args: &[String]) -> Result<()> {
    println!("\nAddressing PR comments...");

    let comments = get_pr_comments(pr_number)?;

    if comments.is_empty() {
        println!("  No comments to address");
        return Ok(());
    }

    println!("  Found {} comment(s) to address", comments.len());

    // Build prompt from comments
    let mut prompt = String::from("# Review Comments\n\n");
    prompt.push_str("The following review comments have been left on this PR. Please address them:\n\n");

    for comment in &comments {
        if let (Some(path), Some(line)) = (&comment.path, comment.line) {
            prompt.push_str(&format!("## {}:{} (@{})\n", path, line, comment.author));
        } else if let Some(path) = &comment.path {
            prompt.push_str(&format!("## {} (@{})\n", path, comment.author));
        } else {
            prompt.push_str(&format!("## Comment by @{}\n", comment.author));
        }
        prompt.push_str(&comment.body);
        prompt.push_str("\n\n");
    }

    prompt.push_str("\nPlease address all the review comments above.");

    // Run qwen to fix
    run_qwen_fix(&prompt)?;

    // Try to resolve review threads
    println!("Resolving review threads...");
    match resolve_all_threads(pr_number) {
        Ok(count) if count > 0 => println!("  Resolved {} thread(s)", count),
        _ => {}
    }

    // Amend and push
    git_amend()?;
    git_push_force(push_args)?;

    println!("Comments addressed and pushed");
    Ok(())
}

fn main() {
    let cli = Cli::parse();

    let result = match cli.command {
        Commands::Push { comments, args } => run_push(&args, comments),
    };

    if let Err(e) = result {
        eprintln!("Error: {:#}", e);
        std::process::exit(1);
    }
}
