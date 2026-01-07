# qwen-pr

Automated PR workflow with AI-powered build failure remediation.

## What it does

`qwen-pr push` automates the entire PR cycle:

1. Force pushes your current branch
2. Creates a PR (or reuses an existing one)
3. Monitors GitHub Actions checks
4. If a check fails:
   - Fetches failed build logs from GitHub Actions
   - Fetches PR review comments (from bots and humans)
   - Sends everything to `qwen -y -p` with context to fix
   - Resolves all review threads (marks comments as addressed)
5. Amends the commit and force pushes again
6. Repeats until all checks pass (max 16 attempts)

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
                      git push -f
                           │
                           └──▶ (repeat monitoring)
```
