# Git Worktree Cleanup Guide

## Overview

The `cleanup-worktrees.sh` script is a utility designed to identify and remove old git worktrees, freeing up disk space in repositories where multiple worktrees are frequently created.

## Why Clean Up Worktrees?

Git worktrees are useful for working on multiple branches simultaneously, but they can accumulate over time and consume significant disk space. This script helps maintain a clean repository by automatically identifying and removing stale worktrees.

## Usage

### Basic Syntax

```bash
./scripts/cleanup-worktrees.sh [OPTIONS]
```

### Options

| Option | Description |
|--------|-------------|
| `--dry-run` | Show what would be deleted without actually deleting |
| `--days DAYS` | Consider worktrees older than DAYS as stale (default: 30) |
| `--auto` | Automatically remove stale worktrees without prompting |
| `--verbose` | Show detailed information about all worktrees |
| `--help` | Display the help message |

### Examples

#### 1. Preview what would be cleaned (safe, no changes)

```bash
./scripts/cleanup-worktrees.sh --dry-run
```

This will show all worktrees that are older than 30 days without removing them.

#### 2. Interactive cleanup (with prompts)

```bash
./scripts/cleanup-worktrees.sh
```

This will identify stale worktrees (older than 30 days) and prompt before removing each one.

#### 3. Automatic cleanup of older worktrees

```bash
./scripts/cleanup-worktrees.sh --days 14 --auto
```

This will automatically remove all worktrees older than 14 days without prompting.

#### 4. Verbose inspection

```bash
./scripts/cleanup-worktrees.sh --verbose --dry-run
```

This shows detailed information about both active and stale worktrees.

## How It Works

1. **Scans all worktrees**: Lists all registered git worktrees in the repository
2. **Skips main worktree**: Never removes the primary working directory
3. **Checks staleness**: Compares modification time against the threshold (default: 30 days)
4. **Calculates space**: Determines the disk space that would be freed
5. **Prompts for confirmation**: Asks before removing (unless `--auto` is used)
6. **Removes worktrees**: Uses `git worktree remove --force` for cleanup

## Output

The script provides colored output for easy reading:

- **[INFO]** (Blue): General information
- **[SUCCESS]** (Green): Successfully completed actions
- **[WARNING]** (Yellow): Stale worktrees or cautions
- **[ERROR]** (Red): Failed operations

## Safety Features

- **Dry-run mode**: Preview changes without making them
- **Interactive prompts**: Confirm before removing (unless `--auto` is used)
- **Main worktree protection**: Never removes the primary working directory
- **Error handling**: Gracefully handles missing or inaccessible worktrees
- **Missing worktrees**: Removes stale registry entries for missing directories

## Workflow Example

For regular maintenance:

```bash
# Step 1: See what would be cleaned
./scripts/cleanup-worktrees.sh --dry-run

# Step 2: If satisfied, run the cleanup
./scripts/cleanup-worktrees.sh --auto --days 21

# Step 3: Verify results
git worktree list
```

## Automating Cleanup

You can add a git hook or cron job to run cleanup periodically:

### Using a git hook (e.g., in `post-checkout`):

```bash
#!/bin/bash
# In .git/hooks/post-checkout
/path/to/scripts/cleanup-worktrees.sh --auto --days 30
```

### Using cron (daily cleanup):

```bash
# Run every day at 2 AM
0 2 * * * cd /path/to/guardian && ./scripts/cleanup-worktrees.sh --auto --days 30
```

## Troubleshooting

### "Could not determine modification time"

This warning appears when the script cannot read the modification time. This is typically harmless but you may want to manually inspect the worktree.

### Script returns permission denied

Make sure the script is executable:

```bash
chmod +x ./scripts/cleanup-worktrees.sh
```

### "Failed to remove" error

The worktree may be in use or locked. Ensure:

1. You're not currently in that worktree
2. No processes have files open in that worktree
3. The directory permissions allow deletion

You can try manually:

```bash
git worktree remove --force <path>
```

## Related Commands

View all worktrees:

```bash
git worktree list
```

Create a new worktree:

```bash
git worktree add <path> <branch>
```

Lock a worktree (prevent accidental removal):

```bash
git worktree lock <path>
```

Unlock a worktree:

```bash
git worktree unlock <path>
```

## See Also

- [Git Worktree Documentation](https://git-scm.com/docs/git-worktree)
- [Guardian Maintenance Guide](README.md)
