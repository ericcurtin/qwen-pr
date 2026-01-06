use anyhow::{Result, bail};
use std::collections::HashSet;
use std::thread;
use std::time::Duration;

use crate::git::{git_amend, git_push_force, run_qwen_fix};
use crate::github::{get_check_runs, get_failed_logs, CheckRun, CheckStatus};

const POLL_INTERVAL_SECS: u64 = 30;
const MAX_FIX_ATTEMPTS: u32 = 10;

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

/// Build a prompt for qwen with failed check logs
fn build_fix_prompt(checks: &[CheckRun]) -> Result<String> {
    let failed_checks: Vec<_> = checks
        .iter()
        .filter(|c| c.status == CheckStatus::Failure)
        .collect();

    if failed_checks.is_empty() {
        return Ok("Fix the CI build failures.".to_string());
    }

    let mut prompt = String::from("The following CI checks have failed. Please fix the issues:\n\n");

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

    prompt.push_str("\nPlease analyze the errors and fix the code to make the CI pass.");

    Ok(prompt)
}

/// Monitor PR checks and attempt fixes on failure
pub fn monitor_and_fix(pr_number: u64) -> Result<()> {
    let mut fix_attempts = 0;

    loop {
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

                // Collect failed checks and their logs
                let prompt = build_fix_prompt(&checks)?;

                // Run qwen to fix
                run_qwen_fix(&prompt)?;

                // Amend and push
                git_amend()?;
                git_push_force()?;

                // Wait a bit for GitHub to register the new commit
                println!("Waiting for GitHub to process new commit...");
                thread::sleep(Duration::from_secs(10));
            }
        }
    }
}
