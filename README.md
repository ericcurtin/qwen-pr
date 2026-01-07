# qwen-pr

Automated PR workflow with AI-powered build failure remediation.

## What it does

`qwen-pr push` automates the entire PR cycle:

1. Fetches from origin and rebases onto the target branch
   - If conflicts occur, uses `qwen` to resolve them automatically
2. Force pushes your current branch
3. Creates a PR (or reuses an existing one)
4. Monitors GitHub Actions checks
5. If a check fails:
   - Fetches failed build logs from GitHub Actions
   - Fetches PR review comments (from bots and humans)
   - Sends everything to `qwen -y -p` with context to fix
   - Resolves all review threads (marks comments as addressed)
   - Rebases again before pushing (in case target branch changed)
6. Amends the commit and force pushes again
7. Repeats until all checks pass (max 16 attempts)

## Prerequisites

- [Rust](https://rustup.rs/) toolchain
- [GitHub CLI](https://cli.github.com/) (`gh`) installed and authenticated

## Installation

```bash
cargo install --path .
```

## Usage

```bash
# From a feature branch (not main/master)
qwen-pr push
```

## How it works

```
git fetch origin
     │
     ▼
git rebase origin/<base> ──▶ conflicts? ──▶ qwen fixes them
     │
     ▼
git push -f
     │
     ▼
┌─────────────┐
│ PR exists?  │──No──▶ gh pr create
└─────────────┘
     │ Yes
     ▼
Monitor checks (every 30s)
     │
     ├── All pass ──▶ Done!
     │
     └── Any fail ──▶ Fetch failed logs
                           │
                           ▼
                      Fetch PR comments
                           │
                           ▼
                      qwen -y -p "<prompt with logs + comments>"
                           │
                           ▼
                      Resolve review threads
                           │
                           ▼
                      git commit --amend
                           │
                           ▼
                      git fetch + rebase (resolve conflicts if any)
                           │
                           ▼
                      git push -f
                           │
                           └──▶ (repeat monitoring)
```
