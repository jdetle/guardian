# Guardian Scripts

Utility scripts for managing and maintaining the Guardian repository.

## Available Scripts

### Installation & Setup

- **`install.sh`** - Main installation script for Guardian
- **`install-claude-hooks.sh`** - Install Claude-related git hooks
- **`install-codex-hooks.sh`** - Install Codex-related git hooks
- **`install-queue-watch.sh`** - Install queue watching service
- **`uninstall.sh`** - Uninstall Guardian and related services

### Operations & Management

- **`guardian-queue.sh`** - Manage the Guardian queue
- **`guardian-queue-watch.sh`** - Monitor the Guardian queue
- **`guardian-resume.sh`** - Resume Guardian operations

### Maintenance

- **`cleanup-worktrees.sh`** - Clean up old git worktrees and free up disk space

## Quick Start

### Clean Up Old Worktrees

To free up disk space by removing old worktrees:

```bash
# Preview what would be deleted
./scripts/cleanup-worktrees.sh --dry-run

# Remove stale worktrees (older than 30 days)
./scripts/cleanup-worktrees.sh

# Automatically remove worktrees older than 14 days
./scripts/cleanup-worktrees.sh --days 14 --auto
```

For detailed documentation, see [`../docs/WORKTREE_CLEANUP.md`](../docs/WORKTREE_CLEANUP.md).

### Install Guardian

```bash
./scripts/install.sh
```

### Monitor Queue

```bash
./scripts/guardian-queue-watch.sh
```

## Usage Tips

1. Always run scripts from the repository root or use absolute paths
2. Check script permissions with `ls -l scripts/`
3. Make scripts executable with `chmod +x scripts/script-name.sh`
4. Review script contents before running, especially installation scripts
5. Use `--help` flag for detailed usage information

## Contributing

When adding new scripts:

1. Add a brief description at the top of the file
2. Include help text (use `--help` flag)
3. Handle errors gracefully
4. Use meaningful exit codes
5. Document the script in this README
6. Consider adding detailed docs in `../docs/` for complex scripts
