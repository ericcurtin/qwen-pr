use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::process::Command;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckStatus {
    Pending,
    Success,
    Failure,
}

#[derive(Debug)]
pub struct CheckRun {
    pub name: String,
    pub status: CheckStatus,
    pub conclusion: Option<String>,
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
    status: String,
    conclusion: String,
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
    let output = Command::new("gh")
        .args([
            "pr", "create",
            "--fill",
            "--json", "number",
        ])
        .output()
        .context("Failed to execute gh pr create")?;

    if !output.status.success() {
        bail!(
            "gh pr create failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[derive(Deserialize)]
    struct PrCreated {
        number: u64,
    }

    let pr: PrCreated = serde_json::from_slice(&output.stdout)
        .context("Failed to parse gh pr create output")?;

    println!("Created PR #{}", pr.number);
    Ok(pr.number)
}

/// Get the check runs for a PR
pub fn get_check_runs(pr_number: u64) -> Result<Vec<CheckRun>> {
    let output = Command::new("gh")
        .args([
            "pr", "checks",
            &pr_number.to_string(),
            "--json", "name,status,conclusion",
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
            let status = match c.status.as_str() {
                "completed" => match c.conclusion.as_str() {
                    "success" | "skipped" | "neutral" => CheckStatus::Success,
                    _ => CheckStatus::Failure,
                },
                _ => CheckStatus::Pending,
            };
            CheckRun {
                name: c.name,
                status,
                conclusion: if c.conclusion.is_empty() {
                    None
                } else {
                    Some(c.conclusion)
                },
            }
        })
        .collect())
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
