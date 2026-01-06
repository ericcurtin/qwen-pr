use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::process::Command;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckStatus {
    Pending,
    Success,
    Failure,
}

#[derive(Debug, Clone)]
pub struct CheckRun {
    pub name: String,
    pub status: CheckStatus,
    pub conclusion: Option<String>,
    pub link: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GhPrListItem {
    number: u64,
    #[serde(rename = "headRefName")]
    head_ref_name: String,
}

#[derive(Debug, Deserialize)]
struct GhCheckRun {
    name: String,
    state: String,
    link: Option<String>,
}

/// Ensure gh CLI is available and authenticated
pub fn ensure_gh_cli() -> Result<()> {
    let output = Command::new("gh")
        .args(["auth", "status"])
        .output()
        .context("Failed to execute gh auth status. Is gh CLI installed?")?;

    if !output.status.success() {
        bail!(
            "gh CLI not authenticated. Run 'gh auth login' first.\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    Ok(())
}

/// Find an existing PR for the given branch
pub fn find_existing_pr(branch: &str) -> Result<Option<u64>> {
    let output = Command::new("gh")
        .args(["pr", "list", "--head", branch, "--json", "number,headRefName"])
        .output()
        .context("Failed to execute gh pr list")?;

    if !output.status.success() {
        bail!(
            "gh pr list failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let prs: Vec<GhPrListItem> = serde_json::from_slice(&output.stdout)
        .context("Failed to parse gh pr list output")?;

    Ok(prs.into_iter().find(|pr| pr.head_ref_name == branch).map(|pr| pr.number))
}

/// Create a new PR for the current branch
pub fn create_pr(branch: &str) -> Result<u64> {
    println!("Creating pull request for branch '{}'...", branch);

    // Create PR with auto-generated title and body
    // gh pr create outputs the PR URL to stdout
    let output = Command::new("gh")
        .args(["pr", "create", "--fill"])
        .output()
        .context("Failed to execute gh pr create")?;

    if !output.status.success() {
        bail!(
            "gh pr create failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let url = String::from_utf8_lossy(&output.stdout).trim().to_string();
    println!("Created PR: {}", url);

    // Extract PR number from URL (e.g., https://github.com/owner/repo/pull/123)
    let pr_number = url
        .rsplit('/')
        .next()
        .and_then(|s| s.parse::<u64>().ok())
        .context("Failed to parse PR number from URL")?;

    Ok(pr_number)
}

/// Get the check runs for a PR
pub fn get_check_runs(pr_number: u64) -> Result<Vec<CheckRun>> {
    let output = Command::new("gh")
        .args([
            "pr", "checks",
            &pr_number.to_string(),
            "--json", "name,state,link",
        ])
        .output()
        .context("Failed to execute gh pr checks")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // If no checks exist yet, return empty
        if stderr.contains("no checks") {
            return Ok(vec![]);
        }
        bail!("gh pr checks failed: {}", stderr);
    }

    let checks: Vec<GhCheckRun> = serde_json::from_slice(&output.stdout)
        .context("Failed to parse gh pr checks output")?;

    Ok(checks
        .into_iter()
        .map(|c| {
            // state values: PENDING, SUCCESS, FAILURE, CANCELLED, SKIPPED, etc.
            let status = match c.state.to_uppercase().as_str() {
                "SUCCESS" | "SKIPPED" | "NEUTRAL" => CheckStatus::Success,
                "PENDING" | "QUEUED" | "IN_PROGRESS" | "WAITING" => CheckStatus::Pending,
                _ => CheckStatus::Failure,
            };
            CheckRun {
                name: c.name,
                status,
                conclusion: Some(c.state),
                link: c.link,
            }
        })
        .collect())
}

/// Extract run ID from a GitHub Actions URL
/// e.g., https://github.com/owner/repo/actions/runs/12345/job/67890 -> 12345
fn extract_run_id(url: &str) -> Option<&str> {
    let parts: Vec<&str> = url.split('/').collect();
    for (i, part) in parts.iter().enumerate() {
        if *part == "runs" && i + 1 < parts.len() {
            return Some(parts[i + 1]);
        }
    }
    None
}

/// Get failed job logs for a workflow run
pub fn get_failed_logs(run_url: &str) -> Result<String> {
    let run_id = extract_run_id(run_url)
        .context("Could not extract run ID from URL")?;

    let output = Command::new("gh")
        .args(["run", "view", run_id, "--log-failed"])
        .output()
        .context("Failed to execute gh run view")?;

    if !output.status.success() {
        // If no failed logs, return empty
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("no failed jobs") || stderr.contains("no logs") {
            return Ok(String::new());
        }
        bail!("gh run view --log-failed failed: {}", stderr);
    }

    let logs = String::from_utf8_lossy(&output.stdout).to_string();

    // Truncate logs if too long (keep last 10000 chars which usually has the error)
    if logs.len() > 15000 {
        Ok(format!("...[truncated]...\n{}", &logs[logs.len() - 10000..]))
    } else {
        Ok(logs)
    }
}

/// Get the URL for a PR
pub fn get_pr_url(pr_number: u64) -> Result<String> {
    let output = Command::new("gh")
        .args(["pr", "view", &pr_number.to_string(), "--json", "url"])
        .output()
        .context("Failed to execute gh pr view")?;

    if !output.status.success() {
        bail!(
            "gh pr view failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[derive(Deserialize)]
    struct PrView {
        url: String,
    }

    let pr: PrView = serde_json::from_slice(&output.stdout)
        .context("Failed to parse gh pr view output")?;

    Ok(pr.url)
}
