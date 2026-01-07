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
/// If remote is specified (e.g., "ericcurtin"), looks for PRs from that fork
pub fn find_existing_pr(branch: &str, remote: Option<&str>) -> Result<Option<u64>> {
    // For forks, we need to use "owner:branch" format
    let head = match remote {
        Some(r) if r != "origin" => format!("{}:{}", r, branch),
        _ => branch.to_string(),
    };

    let output = Command::new("gh")
        .args(["pr", "list", "--head", &head, "--json", "number,headRefName"])
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
/// If remote is specified (e.g., "ericcurtin"), creates PR from that fork
/// Returns the PR number (either newly created or existing if one already exists)
pub fn create_pr(branch: &str, remote: Option<&str>) -> Result<u64> {
    println!("Creating pull request for branch '{}'...", branch);

    let mut cmd = Command::new("gh");
    cmd.args(["pr", "create", "--fill"]);

    // For forks, specify the head as "owner:branch"
    if let Some(r) = remote {
        if r != "origin" {
            cmd.args(["--head", &format!("{}:{}", r, branch)]);
        }
    }

    let output = cmd.output().context("Failed to execute gh pr create")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);

        // Check if PR already exists - extract PR number from error message
        // Error format: "a pull request ... already exists:\nhttps://github.com/owner/repo/pull/123"
        if stderr.contains("already exists") {
            if let Some(pr_number) = extract_pr_number_from_text(&stderr) {
                println!("PR already exists: #{}", pr_number);
                return Ok(pr_number);
            }
        }

        bail!("gh pr create failed: {}", stderr);
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

/// Extract PR number from text containing a GitHub PR URL
fn extract_pr_number_from_text(text: &str) -> Option<u64> {
    // Look for pattern like "/pull/123" in the text
    for part in text.split("/pull/") {
        if let Some(num_str) = part.split(|c: char| !c.is_ascii_digit()).next() {
            if let Ok(num) = num_str.parse::<u64>() {
                if num > 0 {
                    return Some(num);
                }
            }
        }
    }
    None
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

/// A review comment on a PR
#[derive(Debug, Clone)]
pub struct PrComment {
    pub id: String,
    pub author: String,
    pub body: String,
    pub path: Option<String>,
    pub line: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct GhAuthor {
    login: String,
}

#[derive(Debug, Deserialize)]
struct GhComment {
    id: String,
    author: GhAuthor,
    body: String,
}

#[derive(Debug, Deserialize)]
struct GhReview {
    id: String,
    author: GhAuthor,
    #[serde(default)]
    body: String,
    state: String,
}

#[derive(Debug, Deserialize)]
struct GhPrComments {
    #[serde(default)]
    comments: Vec<GhComment>,
    #[serde(default)]
    reviews: Vec<GhReview>,
}

/// Inline review comment from the API
#[derive(Debug, Deserialize)]
struct GhInlineComment {
    id: u64,
    user: GhAuthor,
    body: String,
    path: Option<String>,
    #[serde(default)]
    line: Option<u64>,
    #[serde(default)]
    original_line: Option<u64>,
}

/// Get review comments on a PR (both general comments and inline code review comments)
pub fn get_pr_comments(pr_number: u64) -> Result<Vec<PrComment>> {
    let mut comments = Vec::new();

    // 1. Get general PR conversation comments
    let output = Command::new("gh")
        .args([
            "pr", "view",
            &pr_number.to_string(),
            "--json", "comments,reviews",
        ])
        .output()
        .context("Failed to execute gh pr view for comments")?;

    if output.status.success() {
        if let Ok(pr_data) = serde_json::from_slice::<GhPrComments>(&output.stdout) {
            // Add general PR comments (these don't have file/line info)
            for c in pr_data.comments {
                if !c.body.trim().is_empty() {
                    comments.push(PrComment {
                        id: c.id,
                        author: c.author.login,
                        body: c.body,
                        path: None,
                        line: None,
                    });
                }
            }

            // Add review bodies (overall review comments)
            for review in pr_data.reviews {
                if !review.body.trim().is_empty() {
                    comments.push(PrComment {
                        id: review.id.clone(),
                        author: review.author.login.clone(),
                        body: format!("[{}] {}", review.state, review.body),
                        path: None,
                        line: None,
                    });
                }
            }
        }
    }

    // 2. Get inline code review comments via API
    let api_output = Command::new("gh")
        .args([
            "api",
            &format!("repos/{{owner}}/{{repo}}/pulls/{}/comments", pr_number),
        ])
        .output()
        .context("Failed to execute gh api for inline comments")?;

    if api_output.status.success() {
        if let Ok(inline_comments) = serde_json::from_slice::<Vec<GhInlineComment>>(&api_output.stdout) {
            for c in inline_comments {
                if !c.body.trim().is_empty() {
                    let line = c.line.or(c.original_line);
                    comments.push(PrComment {
                        id: c.id.to_string(),
                        author: c.user.login,
                        body: c.body,
                        path: c.path,
                        line,
                    });
                }
            }
        }
    }

    Ok(comments)
}

/// A review thread that can be resolved
#[derive(Debug, Clone)]
pub struct ReviewThread {
    pub id: String,
}

#[derive(Debug, Deserialize)]
struct GhGraphQLResponse {
    data: Option<GhGraphQLData>,
}

#[derive(Debug, Deserialize)]
struct GhGraphQLData {
    repository: Option<GhRepository>,
}

#[derive(Debug, Deserialize)]
struct GhRepository {
    #[serde(rename = "pullRequest")]
    pull_request: Option<GhPullRequest>,
}

#[derive(Debug, Deserialize)]
struct GhPullRequest {
    #[serde(rename = "reviewThreads")]
    review_threads: GhReviewThreads,
}

#[derive(Debug, Deserialize)]
struct GhReviewThreads {
    nodes: Vec<GhReviewThread>,
}

#[derive(Debug, Deserialize)]
struct GhReviewThread {
    id: String,
    #[serde(rename = "isResolved")]
    is_resolved: bool,
}

/// Get all unresolved review threads for a PR
pub fn get_unresolved_threads(pr_number: u64) -> Result<Vec<ReviewThread>> {
    let query = r#"
        query($pr: Int!) {
            repository(owner: "", name: "") {
                pullRequest(number: $pr) {
                    reviewThreads(first: 100) {
                        nodes {
                            id
                            isResolved
                        }
                    }
                }
            }
        }
    "#;

    // First get repo info
    let repo_output = Command::new("gh")
        .args(["repo", "view", "--json", "owner,name"])
        .output()
        .context("Failed to get repo info")?;

    if !repo_output.status.success() {
        bail!("Failed to get repo info");
    }

    #[derive(Deserialize)]
    struct RepoInfo {
        owner: RepoOwner,
        name: String,
    }
    #[derive(Deserialize)]
    struct RepoOwner {
        login: String,
    }

    let repo_info: RepoInfo = serde_json::from_slice(&repo_output.stdout)
        .context("Failed to parse repo info")?;

    let query_with_repo = query
        .replace(r#"owner: """#, &format!(r#"owner: "{}""#, repo_info.owner.login))
        .replace(r#"name: """#, &format!(r#"name: "{}""#, repo_info.name));

    let output = Command::new("gh")
        .args([
            "api", "graphql",
            "-f", &format!("query={}", query_with_repo),
            "-F", &format!("pr={}", pr_number),
        ])
        .output()
        .context("Failed to execute gh api graphql")?;

    if !output.status.success() {
        bail!(
            "gh api graphql failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let response: GhGraphQLResponse = serde_json::from_slice(&output.stdout)
        .context("Failed to parse GraphQL response")?;

    let threads = response
        .data
        .and_then(|d| d.repository)
        .and_then(|r| r.pull_request)
        .map(|pr| pr.review_threads.nodes)
        .unwrap_or_default();

    Ok(threads
        .into_iter()
        .filter(|t| !t.is_resolved)
        .map(|t| ReviewThread { id: t.id })
        .collect())
}

/// Resolve a review thread by its ID
pub fn resolve_thread(thread_id: &str) -> Result<()> {
    let mutation = r#"
        mutation($threadId: ID!) {
            resolveReviewThread(input: {threadId: $threadId}) {
                thread {
                    isResolved
                }
            }
        }
    "#;

    let output = Command::new("gh")
        .args([
            "api", "graphql",
            "-f", &format!("query={}", mutation),
            "-f", &format!("threadId={}", thread_id),
        ])
        .output()
        .context("Failed to execute gh api graphql")?;

    if !output.status.success() {
        bail!(
            "Failed to resolve thread: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    Ok(())
}

/// Resolve all unresolved review threads for a PR
pub fn resolve_all_threads(pr_number: u64) -> Result<usize> {
    let threads = get_unresolved_threads(pr_number)?;
    let count = threads.len();

    for thread in threads {
        if let Err(e) = resolve_thread(&thread.id) {
            eprintln!("Warning: failed to resolve thread: {}", e);
        }
    }

    Ok(count)
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

/// Get the base (target) branch for a PR
pub fn get_pr_base_branch(pr_number: u64) -> Result<String> {
    let output = Command::new("gh")
        .args(["pr", "view", &pr_number.to_string(), "--json", "baseRefName"])
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
        #[serde(rename = "baseRefName")]
        base_ref_name: String,
    }

    let pr: PrView = serde_json::from_slice(&output.stdout)
        .context("Failed to parse gh pr view output")?;

    Ok(pr.base_ref_name)
}

/// Get the default branch for the repo (main/master)
pub fn get_default_branch() -> Result<String> {
    let output = Command::new("gh")
        .args(["repo", "view", "--json", "defaultBranchRef"])
        .output()
        .context("Failed to execute gh repo view")?;

    if !output.status.success() {
        bail!(
            "gh repo view failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[derive(Deserialize)]
    struct BranchRef {
        name: String,
    }

    #[derive(Deserialize)]
    struct RepoView {
        #[serde(rename = "defaultBranchRef")]
        default_branch_ref: BranchRef,
    }

    let repo: RepoView = serde_json::from_slice(&output.stdout)
        .context("Failed to parse gh repo view output")?;

    Ok(repo.default_branch_ref.name)
}
