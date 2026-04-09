---
name: worktrees
description: Worktree setup and parallel development workflow for this project. Use when creating an isolated workspace for a new feature branch.
---

# Worktrees — longbridge-terminal

## Convention

Worktrees live at `.claude/worktrees/<feature>/` (gitignored).

## Setup

```bash
# From repo root
git worktree add .claude/worktrees/<feature> -b feature/<feature>
cd .claude/worktrees/<feature>
cargo build
```

## Checklist

1. Verify `.claude/worktrees/` is gitignored:
   ```bash
   git check-ignore -q .claude/worktrees && echo "ok" || echo "ADD TO .gitignore"
   ```
2. Create worktree with new branch
3. `cargo build` to verify clean baseline
4. Work in the worktree (fully isolated branch + working directory)
5. After merge, clean up:
   ```bash
   git worktree remove .claude/worktrees/<feature>
   git branch -d feature/<feature>
   ```

## Sandbox Builds with boxsh

`boxsh` (installed at `/usr/local/bin/boxsh`) provides copy-on-write sandbox for safe experimental builds:

```bash
cd .claude/worktrees/<feature>
boxsh --try -c 'cargo build --release'
# All changes go to /tmp/boxsh-try-*/work; original worktree untouched
```

## List Worktrees

```bash
git worktree list
```

## Branch Naming

Follow project commit prefix conventions:
- `feature/cli-<name>` — CLI changes
- `feature/tui-<name>` — TUI changes
- `feature/<name>` — other

Examples: `feature/watchlist-pinned`, `feature/cli-export`
