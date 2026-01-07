mod git;
mod github;
mod monitor;

use anyhow::Result;
use clap::{Parser, Subcommand};
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    prelude::*,
    widgets::{Block, Borders, List, ListItem, Paragraph},
};
use std::io::stdout;

use git::{ensure_git_repo, get_current_branch, git_amend, git_fetch, git_push_force, git_rebase_with_conflict_resolution, run_qwen_fix};
use github::{create_pr, ensure_gh_cli, find_existing_pr, get_default_branch, get_pr_base_branch, get_pr_comments, get_pr_url, resolve_all_threads, PrComment};
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

const MAX_REBASE_CONFLICT_ATTEMPTS: u32 = 5;

/// Fetch and rebase against the target branch
/// If pr_number is provided, uses the PR's base branch; otherwise uses the default branch
fn fetch_and_rebase(pr_number: Option<u64>) -> Result<()> {
    // Get the target branch
    let base_branch = match pr_number {
        Some(pr) => get_pr_base_branch(pr)?,
        None => get_default_branch()?,
    };

    println!("Target branch: {}", base_branch);

    // Fetch from origin (the upstream repo)
    git_fetch("origin")?;

    // Rebase onto origin/<base_branch>
    let target_ref = format!("origin/{}", base_branch);
    let success = git_rebase_with_conflict_resolution(&target_ref, MAX_REBASE_CONFLICT_ATTEMPTS)?;

    if !success {
        anyhow::bail!("Rebase failed - could not resolve conflicts automatically");
    }

    Ok(())
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

    // Check if PR already exists to get target branch
    let existing_pr = find_existing_pr(&branch, remote)?;

    // Fetch and rebase before pushing
    fetch_and_rebase(existing_pr)?;

    // Force push
    git_push_force(push_args)?;

    // Find or create PR
    let pr_number = match existing_pr {
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

/// Format a comment header (location and author)
fn format_comment_header(comment: &PrComment, index: usize) -> String {
    let location = if let (Some(path), Some(line)) = (&comment.path, comment.line) {
        format!("[{}:{}]", path, line)
    } else if let Some(path) = &comment.path {
        format!("[{}]", path)
    } else {
        String::new()
    };

    if location.is_empty() {
        format!("{}. @{}", index + 1, comment.author)
    } else {
        format!("{}. {} @{}", index + 1, location, comment.author)
    }
}

/// TUI state for comment selection
struct CommentSelector {
    comments: Vec<PrComment>,
    selected: Vec<bool>,
    cursor: usize,
    scroll_offset: usize,
}

impl CommentSelector {
    fn new(comments: Vec<PrComment>) -> Self {
        let selected = vec![true; comments.len()]; // All selected by default
        Self {
            comments,
            selected,
            cursor: 0,
            scroll_offset: 0,
        }
    }

    fn toggle_current(&mut self) {
        if !self.comments.is_empty() {
            self.selected[self.cursor] = !self.selected[self.cursor];
        }
    }

    fn next(&mut self) {
        if !self.comments.is_empty() {
            self.cursor = (self.cursor + 1) % self.comments.len();
        }
    }

    fn previous(&mut self) {
        if !self.comments.is_empty() {
            self.cursor = if self.cursor == 0 {
                self.comments.len() - 1
            } else {
                self.cursor - 1
            };
        }
    }

    fn get_selected_comments(&self) -> Vec<&PrComment> {
        self.comments
            .iter()
            .enumerate()
            .filter(|(i, _)| self.selected[*i])
            .map(|(_, c)| c)
            .collect()
    }
}

/// Run the TUI for comment selection, returns selected indices or None if cancelled
fn run_comment_tui(comments: Vec<PrComment>) -> Result<Option<Vec<PrComment>>> {
    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut selector = CommentSelector::new(comments);
    let result: Option<Vec<PrComment>>;

    loop {
        terminal.draw(|f| {
            let area = f.area();

            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(3),  // Comment list (top)
                    Constraint::Min(5),     // Selected comment detail (middle)
                    Constraint::Length(3),  // Help (bottom)
                ])
                .split(area);

            // Top: scrollable list of comment headers
            let visible_height = chunks[0].height.saturating_sub(2) as usize; // Account for borders

            // Adjust scroll to keep cursor visible
            if selector.cursor < selector.scroll_offset {
                selector.scroll_offset = selector.cursor;
            } else if selector.cursor >= selector.scroll_offset + visible_height {
                selector.scroll_offset = selector.cursor - visible_height + 1;
            }

            let items: Vec<ListItem> = selector
                .comments
                .iter()
                .enumerate()
                .skip(selector.scroll_offset)
                .take(visible_height.max(1))
                .map(|(i, comment)| {
                    let checkbox = if selector.selected[i] { "[x] " } else { "[ ] " };
                    let header = format_comment_header(comment, i);
                    let prefix = if i == selector.cursor { "> " } else { "  " };
                    let style = if i == selector.cursor {
                        Style::default().add_modifier(Modifier::REVERSED)
                    } else {
                        Style::default()
                    };
                    ListItem::new(format!("{}{}{}", prefix, checkbox, header)).style(style)
                })
                .collect();

            let list = List::new(items)
                .block(Block::default().borders(Borders::ALL).title(format!(
                    "Comments ({}/{})",
                    selector.cursor + 1,
                    selector.comments.len()
                )));

            f.render_widget(list, chunks[0]);

            // Middle: full content of selected comment
            if !selector.comments.is_empty() {
                let comment = &selector.comments[selector.cursor];
                let header = format_comment_header(comment, selector.cursor);
                let full_text = format!("{}\n\n{}", header, comment.body);

                let detail = Paragraph::new(full_text)
                    .block(Block::default().borders(Borders::ALL).title("Comment Detail"))
                    .wrap(ratatui::widgets::Wrap { trim: false });

                f.render_widget(detail, chunks[1]);
            }

            // Bottom: help
            let selected_count = selector.selected.iter().filter(|&&s| s).count();
            let help = Paragraph::new(format!(
                "↑/↓: navigate | Space: toggle | Enter: confirm ({} selected) | Esc/q: cancel",
                selected_count
            ))
            .block(Block::default().borders(Borders::ALL));
            f.render_widget(help, chunks[2]);
        })?;

        if event::poll(std::time::Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    match key.code {
                        KeyCode::Esc | KeyCode::Char('q') => {
                            result = None;
                            break;
                        }
                        KeyCode::Char('c')
                            if key.modifiers.contains(crossterm::event::KeyModifiers::CONTROL) =>
                        {
                            result = None;
                            break;
                        }
                        KeyCode::Enter => {
                            result = Some(
                                selector
                                    .get_selected_comments()
                                    .into_iter()
                                    .cloned()
                                    .collect(),
                            );
                            break;
                        }
                        KeyCode::Up | KeyCode::Char('k') => selector.previous(),
                        KeyCode::Down | KeyCode::Char('j') => selector.next(),
                        KeyCode::Char(' ') => selector.toggle_current(),
                        _ => {}
                    }
                }
            }
        }
    }

    // Restore terminal - this happens regardless of how we exit
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;

    Ok(result)
}

/// Address PR comments proactively before monitoring builds
fn address_pr_comments(pr_number: u64, push_args: &[String]) -> Result<()> {
    println!("\nFetching PR comments...");

    let comments = get_pr_comments(pr_number)?;

    if comments.is_empty() {
        println!("  No comments to address");
        return Ok(());
    }

    println!("  Found {} comment(s)", comments.len());

    // Run TUI for selection
    let selected_comments = match run_comment_tui(comments)? {
        Some(comments) => comments,
        None => {
            println!("\nCancelled.");
            return Ok(());
        }
    };

    if selected_comments.is_empty() {
        println!("\nNo comments selected, skipping...");
        return Ok(());
    }

    println!("\nAddressing {} comment(s)...", selected_comments.len());

    // Build prompt from selected comments
    let mut prompt = String::from("# Review Comments\n\n");
    prompt.push_str("The following review comments have been left on this PR. Please address them:\n\n");

    for comment in &selected_comments {
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

    // Amend, rebase, and push
    git_amend()?;
    fetch_and_rebase(Some(pr_number))?;
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
