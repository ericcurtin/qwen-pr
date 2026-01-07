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

/// Fetch from a remote
pub fn git_fetch(remote: &str) -> Result<()> {
    println!("Fetching from {}...", remote);

    let status = Command::new("git")
        .args(["fetch", remote])
        .status()
        .context("Failed to execute git fetch")?;

    if !status.success() {
        bail!("git fetch {} failed", remote);
    }

    Ok(())
}

/// Check if there are merge conflicts in the working directory
fn has_conflicts() -> bool {
    let output = Command::new("git")
        .args(["diff", "--name-only", "--diff-filter=U"])
        .output();

    match output {
        Ok(out) => !out.stdout.is_empty(),
        Err(_) => false,
    }
}

/// Get list of conflicted files
fn get_conflicted_files() -> Vec<String> {
    let output = Command::new("git")
        .args(["diff", "--name-only", "--diff-filter=U"])
        .output();

    match output {
        Ok(out) => {
            String::from_utf8_lossy(&out.stdout)
                .lines()
                .map(|s| s.to_string())
                .collect()
        }
        Err(_) => vec![],
    }
}

/// Abort an in-progress rebase
fn git_rebase_abort() -> Result<()> {
    let _ = Command::new("git")
        .args(["rebase", "--abort"])
        .status();
    Ok(())
}

/// Continue a rebase after conflicts are resolved
fn git_rebase_continue() -> Result<bool> {
    // Stage all changes first
    let _ = Command::new("git")
        .args(["add", "-A"])
        .status();

    // Use GIT_EDITOR=true to skip commit message prompts
    let status = Command::new("git")
        .args(["rebase", "--continue"])
        .env("GIT_EDITOR", "true")
        .status()
        .context("Failed to execute git rebase --continue")?;

    Ok(status.success())
}

/// Check if there are any uncommitted changes (staged or unstaged)
fn has_uncommitted_changes() -> bool {
    let output = Command::new("git")
        .args(["status", "--porcelain"])
        .output();

    match output {
        Ok(out) => !out.stdout.is_empty(),
        Err(_) => false,
    }
}

/// Stash any uncommitted changes
fn git_stash_push() -> Result<bool> {
    if !has_uncommitted_changes() {
        return Ok(false); // Nothing to stash
    }

    println!("Stashing uncommitted changes...");
    let status = Command::new("git")
        .args(["stash", "push", "-m", "qwen-pr: auto-stash before rebase"])
        .status()
        .context("Failed to execute git stash push")?;

    if !status.success() {
        bail!("git stash push failed");
    }

    Ok(true)
}

/// Pop the stash
fn git_stash_pop() -> Result<()> {
    println!("Restoring stashed changes...");
    let status = Command::new("git")
        .args(["stash", "pop"])
        .status()
        .context("Failed to execute git stash pop")?;

    if !status.success() {
        // Stash pop can fail if there are conflicts - try to continue anyway
        eprintln!("Warning: git stash pop had conflicts, changes left in stash");
    }

    Ok(())
}

/// Check if we're in the middle of a rebase
fn is_rebasing() -> bool {
    let git_dir = Command::new("git")
        .args(["rev-parse", "--git-dir"])
        .output();

    match git_dir {
        Ok(out) => {
            let dir = String::from_utf8_lossy(&out.stdout).trim().to_string();
            std::path::Path::new(&dir).join("rebase-merge").exists()
                || std::path::Path::new(&dir).join("rebase-apply").exists()
        }
        Err(_) => false,
    }
}

/// Rebase current branch onto a target branch, using qwen to fix conflicts
/// Returns Ok(true) if rebase succeeded, Ok(false) if aborted, Err on failure
pub fn git_rebase_with_conflict_resolution(target_ref: &str, max_attempts: u32) -> Result<bool> {
    println!("Rebasing onto {}...", target_ref);

    // Stash any uncommitted changes first
    let stashed = git_stash_push()?;

    let status = Command::new("git")
        .args(["rebase", target_ref])
        .status()
        .context("Failed to execute git rebase")?;

    if status.success() {
        println!("Rebase completed successfully (no conflicts)");
        if stashed {
            git_stash_pop()?;
        }
        return Ok(true);
    }

    // Rebase failed - check if it's due to conflicts or something else
    if !is_rebasing() && !has_conflicts() {
        // Rebase failed but we're not in a rebase state - something went wrong
        eprintln!("Rebase failed unexpectedly");
        if stashed {
            git_stash_pop()?;
        }
        return Ok(false);
    }

    // We're in a rebase with conflicts
    let mut attempts = 0;

    while (has_conflicts() || is_rebasing()) && attempts < max_attempts {
        attempts += 1;
        let conflicted = get_conflicted_files();

        if conflicted.is_empty() && !is_rebasing() {
            break;
        }

        if !conflicted.is_empty() {
            println!(
                "Conflict in {} file(s), attempting fix ({}/{})...",
                conflicted.len(),
                attempts,
                max_attempts
            );

            // Build a prompt for qwen to fix the conflicts
            let prompt = format!(
                "# Merge Conflicts\n\n\
                The following files have merge conflicts that need to be resolved:\n\n\
                {}\n\n\
                Please resolve all merge conflicts in these files. \
                Look for conflict markers (<<<<<<, =======, >>>>>>) and choose the correct resolution. \
                Remove all conflict markers after resolving.",
                conflicted.iter().map(|f| format!("- {}", f)).collect::<Vec<_>>().join("\n")
            );

            // Run qwen to fix conflicts
            if let Err(e) = run_qwen_fix(&prompt) {
                eprintln!("Warning: qwen fix failed: {}", e);
            }
        }

        // Try to continue the rebase
        match git_rebase_continue() {
            Ok(true) => {
                // Check if rebase is complete
                if !is_rebasing() && !has_conflicts() {
                    println!("Rebase completed after resolving conflicts");
                    if stashed {
                        git_stash_pop()?;
                    }
                    return Ok(true);
                }
                // More conflicts from next commit, loop continues
            }
            Ok(false) => {
                // Rebase continue failed
                if !is_rebasing() && !has_conflicts() {
                    // Rebase is done (maybe it was already complete)
                    break;
                }
                // Still in rebase, loop continues
            }
            Err(e) => {
                eprintln!("Error during rebase continue: {}", e);
            }
        }
    }

    if has_conflicts() || is_rebasing() {
        eprintln!("Could not resolve all conflicts after {} attempts, aborting rebase", max_attempts);
        git_rebase_abort()?;
        if stashed {
            git_stash_pop()?;
        }
        return Ok(false);
    }

    println!("Rebase completed successfully");
    if stashed {
        git_stash_pop()?;
    }
    Ok(true)
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
