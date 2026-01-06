use anyhow::{Context, Result, bail};
use std::process::Command;

/// Get the current git branch name
pub fn get_current_branch() -> Result<String> {
    let output = Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .context("Failed to execute git rev-parse")?;

    if !output.status.success() {
        bail!(
            "Failed to get current branch: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Check if we're in a git repository
pub fn ensure_git_repo() -> Result<()> {
    let output = Command::new("git")
        .args(["rev-parse", "--git-dir"])
        .output()
        .context("Failed to execute git rev-parse")?;

    if !output.status.success() {
        bail!("Not in a git repository");
    }

    Ok(())
}

/// Force push the current branch to origin
pub fn git_push_force() -> Result<()> {
    println!("Pushing to origin...");

    let status = Command::new("git")
        .args(["push", "-f", "--set-upstream", "origin", "HEAD"])
        .status()
        .context("Failed to execute git push")?;

    if !status.success() {
        bail!("git push -f failed");
    }

    println!("Push successful");
    Ok(())
}

/// Amend the current commit without changing the message
pub fn git_amend() -> Result<()> {
    println!("Amending commit...");

    // First, stage all changes
    let status = Command::new("git")
        .args(["add", "-A"])
        .status()
        .context("Failed to execute git add")?;

    if !status.success() {
        bail!("git add -A failed");
    }

    // Then amend
    let status = Command::new("git")
        .args(["commit", "--amend", "--no-edit"])
        .status()
        .context("Failed to execute git commit --amend")?;

    if !status.success() {
        bail!("git commit --amend failed");
    }

    println!("Commit amended");
    Ok(())
}

/// Run the qwen AI assistant to attempt fixes
pub fn run_qwen_fix() -> Result<()> {
    println!("Running qwen to fix issues...");

    let status = Command::new("qwen")
        .args(["-y", "-p"])
        .status()
        .context("Failed to execute qwen -y -p")?;

    if !status.success() {
        bail!("qwen -y -p failed");
    }

    println!("Qwen fix attempt completed");
    Ok(())
}
