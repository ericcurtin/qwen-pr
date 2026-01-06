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

/// Force push the current branch
/// If extra_args is empty, pushes to origin. Otherwise uses the provided args.
pub fn git_push_force(extra_args: &[String]) -> Result<()> {
    println!("Pushing...");

    let mut cmd = Command::new("git");
    cmd.arg("push").arg("-f");

    if extra_args.is_empty() {
        // Default: push to origin with upstream tracking
        cmd.args(["--set-upstream", "origin", "HEAD"]);
    } else {
        // Use provided args (e.g., "ericcurtin" becomes "git push -f ericcurtin HEAD")
        cmd.args(extra_args);
        cmd.arg("HEAD");
    }

    let status = cmd.status().context("Failed to execute git push")?;

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

/// Run the qwen AI assistant to attempt fixes with the given prompt
pub fn run_qwen_fix(prompt: &str) -> Result<()> {
    println!("Running qwen to fix issues...");

    let status = Command::new("qwen")
        .args(["-y", "-p", prompt])
        .status()
        .context("Failed to execute qwen -y -p")?;

    if !status.success() {
        bail!("qwen -y -p failed");
    }

    println!("Qwen fix attempt completed");
    Ok(())
}
