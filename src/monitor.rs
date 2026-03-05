use anyhow::{Result, bail};
use std::collections::HashSet;
use std::thread;
use std::time::Duration;

use crate::git::{git_amend, git_fetch, git_push_force, git_rebase_with_conflict_resolution, run_qwen_fix};
use crate::github::{get_check_runs, get_failed_logs, get_pr_base_branch, get_pr_comments, resolve_all_threads, CheckRun, CheckStatus};

const POLL_INTERVAL_SECS: u64 = 128;
const MAX_FIX_ATTEMPTS: u32 = 16;
const MAX_REBASE_CONFLICT_ATTEMPTS: u32 = 5;

/// Fetch and rebase against the PR's base branch
fn fetch_and_rebase(pr_number: u64) -> Result<()> {
    let base_branch = get_pr_base_branch(pr_number)?;
    git_fetch("origin")?;

    let target_ref = format!("origin/{}", base_branch);
    let success = git_rebase_with_conflict_resolution(&target_ref, MAX_REBASE_CONFLICT_ATTEMPTS)?;

    if !success {
        anyhow::bail!("Rebase failed - could not resolve conflicts automatically");
    }

    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
enum OverallStatus {
    Pending,
    Success,
    Failure,
}

fn determine_overall_status(checks: &[crate::github::CheckRun]) -> OverallStatus {
    if checks.is_empty() {
        return OverallStatus::Pending;
    }

    let mut has_pending = false;
    let mut has_failure = false;

    for check in checks {
        match check.status {
            CheckStatus::Pending => has_pending = true,
            CheckStatus::Failure => has_failure = true,
            CheckStatus::Success => {}
        }
    }

    if has_failure {
        OverallStatus::Failure
    } else if has_pending {
        OverallStatus::Pending
    } else {
        OverallStatus::Success
    }
}

fn print_check_status(checks: &[crate::github::CheckRun]) {
    if checks.is_empty() {
        println!("  No checks found yet");
        return;
    }

    for check in checks {
        let icon = match check.status {
            CheckStatus::Pending => "⏳",
            CheckStatus::Success => "✅",
            CheckStatus::Failure => "❌",
        };
        let conclusion = check
            .conclusion
            .as_ref()
            .map(|c| format!(" ({})", c))
            .unwrap_or_default();
        println!("  {} {}{}", icon, check.name, conclusion);
    }
}

/// Build a prompt for qwen with failed check logs and PR comments
/// Returns the prompt and the IDs of comments that were included
fn build_fix_prompt(
    pr_number: u64,
    checks: &[CheckRun],
    addressed_comments: &HashSet<String>,
) -> Result<(String, Vec<String>)> {
    let mut prompt = String::new();
    let mut has_content = false;
    let mut new_comment_ids = Vec::new();

    // Fetch and include PR review comments (excluding already addressed ones)
    println!("Fetching PR comments...");
    match get_pr_comments(pr_number) {
        Ok(comments) => {
            // Filter out already addressed comments
            let new_comments: Vec<_> = comments
                .into_iter()
                .filter(|c| !addressed_comments.contains(&c.id))
                .collect();

            if !new_comments.is_empty() {
                has_content = true;
                prompt.push_str("# Review Comments\n\n");
                prompt.push_str("The following review comments have been left on this PR. Please address them:\n\n");

                for comment in &new_comments {
                    new_comment_ids.push(comment.id.clone());

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

                println!("  Found {} new comment(s) to address", new_comments.len());
            } else {
                println!("  No new comments to address");
            }
        }
        Err(e) => {
            println!("  Could not fetch comments: {}", e);
        }
    }

    // Include failed checks
    let failed_checks: Vec<_> = checks
        .iter()
        .filter(|c| c.status == CheckStatus::Failure)
        .collect();

    if !failed_checks.is_empty() {
        has_content = true;
        prompt.push_str("# CI Failures\n\n");
        prompt.push_str("The following CI checks have failed. Please fix the issues:\n\n");

        // Deduplicate run IDs to avoid fetching same logs multiple times
        let mut seen_runs = HashSet::new();

        for check in &failed_checks {
            prompt.push_str(&format!("## Failed: {}\n", check.name));

            if let Some(link) = &check.link {
                // Extract run ID to deduplicate
                let run_id = link.split("/runs/").nth(1).and_then(|s| s.split('/').next());

                if let Some(rid) = run_id {
                    if !seen_runs.contains(rid) {
                        seen_runs.insert(rid.to_string());

                        println!("Fetching logs for {}...", check.name);
                        match get_failed_logs(link) {
                            Ok(logs) if !logs.is_empty() => {
                                prompt.push_str("```\n");
                                prompt.push_str(&logs);
                                prompt.push_str("\n```\n\n");
                            }
                            Ok(_) => {
                                prompt.push_str("(No logs available)\n\n");
                            }
                            Err(e) => {
                                prompt.push_str(&format!("(Could not fetch logs: {})\n\n", e));
                            }
                        }
                    } else {
                        prompt.push_str("(Logs already included above)\n\n");
                    }
                }
            }
        }
    }

    if !has_content {
        return Ok(("Fix any issues with the code.".to_string(), new_comment_ids));
    }

    prompt.push_str("\nPlease analyze the errors and review comments, then fix the code to address all issues.");

    Ok((prompt, new_comment_ids))
}

/// Monitor PR checks and attempt fixes on failure
pub fn monitor_and_fix(pr_number: u64, push_args: &[String]) -> Result<()> {
    let mut fix_attempts = 0;
    let mut addressed_comments: HashSet<String> = HashSet::new();
    let mut first_check = true;

    loop {
        // Wait before checking (gives CI time to start on first check)
        if first_check {
            println!(
                "\nWaiting {} seconds for CI to start...",
                POLL_INTERVAL_SECS
            );
            first_check = false;
            thread::sleep(Duration::from_secs(POLL_INTERVAL_SECS));
        }

        println!("\nChecking PR #{} status...", pr_number);

        let checks = get_check_runs(pr_number)?;
        print_check_status(&checks);

        let status = determine_overall_status(&checks);

        match status {
            OverallStatus::Success => {
                println!("\n✅ All checks passed!");
                return Ok(());
            }
            OverallStatus::Pending => {
                println!(
                    "\n⏳ Checks still running, waiting {} seconds...",
                    POLL_INTERVAL_SECS
                );
                thread::sleep(Duration::from_secs(POLL_INTERVAL_SECS));
            }
            OverallStatus::Failure => {
                fix_attempts += 1;
                println!(
                    "\n❌ Check(s) failed. Attempting fix ({}/{})...",
                    fix_attempts, MAX_FIX_ATTEMPTS
                );

                if fix_attempts > MAX_FIX_ATTEMPTS {
                    bail!(
                        "Exceeded maximum fix attempts ({}). Giving up.",
                        MAX_FIX_ATTEMPTS
                    );
                }

                // Collect failed checks, logs, and PR comments (excluding already addressed)
                let (prompt, new_comment_ids) = build_fix_prompt(pr_number, &checks, &addressed_comments)?;

                // Run qwen to fix
                run_qwen_fix(&prompt)?;

                // Mark these comments as addressed
                for id in new_comment_ids {
                    addressed_comments.insert(id);
                }

                // Try to resolve review threads (some may not be resolvable, that's ok)
                println!("Resolving review threads...");
                match resolve_all_threads(pr_number) {
                    Ok(count) if count > 0 => println!("  Resolved {} thread(s)", count),
                    Ok(_) => {}
                    Err(_) => {} // Silently ignore - some threads can't be resolved
                }

                // Amend, rebase, and push
                git_amend()?;
                fetch_and_rebase(pr_number)?;
                git_push_force(push_args)?;

                // Wait a bit for GitHub to register the new commit
                println!("Waiting for GitHub to process new commit...");
                thread::sleep(Duration::from_secs(POLL_INTERVAL_SECS));
            }
        }
    }
}
