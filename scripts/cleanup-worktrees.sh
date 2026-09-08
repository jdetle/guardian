#!/bin/bash

# cleanup-worktrees.sh
# Utility to identify and remove old git worktrees to free up disk space
# Usage: ./cleanup-worktrees.sh [--dry-run] [--days DAYS] [--auto]

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(git -C "$SCRIPT_DIR" rev-parse --show-toplevel)"

# Default values
DRY_RUN=false
DAYS_THRESHOLD=30
AUTO_MODE=false
VERBOSE=false

# Color codes for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Print colored messages
info() {
    echo -e "${BLUE}[INFO]${NC} $1"
}

success() {
    echo -e "${GREEN}[SUCCESS]${NC} $1"
}

warning() {
    echo -e "${YELLOW}[WARNING]${NC} $1"
}

error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

# Print usage
usage() {
    cat << EOF
Usage: $(basename "$0") [OPTIONS]

Clean up old git worktrees to free up disk space.

OPTIONS:
    --dry-run           Show what would be deleted without actually deleting
    --days DAYS         Consider worktrees older than DAYS as stale (default: 30)
    --auto              Automatically remove stale worktrees without prompting
    --verbose           Show detailed information
    --help              Show this help message

EXAMPLES:
    # Show what would be deleted (dry-run)
    $(basename "$0") --dry-run

    # Remove worktrees older than 30 days (prompt before each)
    $(basename "$0")

    # Automatically remove worktrees older than 14 days
    $(basename "$0") --days 14 --auto

EOF
}

# Parse command line arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        --dry-run)
            DRY_RUN=true
            shift
            ;;
        --days)
            DAYS_THRESHOLD="$2"
            shift 2
            ;;
        --auto)
            AUTO_MODE=true
            shift
            ;;
        --verbose)
            VERBOSE=true
            shift
            ;;
        --help)
            usage
            exit 0
            ;;
        *)
            error "Unknown option: $1"
            usage
            exit 1
            ;;
    esac
done

# Validate days threshold
if ! [[ "$DAYS_THRESHOLD" =~ ^[0-9]+$ ]]; then
    error "Days threshold must be a positive integer"
    exit 1
fi

# Get current timestamp
CURRENT_TIME=$(date +%s)
THRESHOLD_TIME=$((CURRENT_TIME - (DAYS_THRESHOLD * 86400)))

info "Scanning for worktrees older than $DAYS_THRESHOLD days..."
info "Current repository: $REPO_DIR"
echo ""

# Array to hold worktrees to remove
declare -a WORKTREES_TO_REMOVE
TOTAL_SIZE=0

# Get list of all worktrees
while IFS= read -r line; do
    if [ -z "$line" ]; then
        continue
    fi

    WORKTREE_PATH=$(echo "$line" | awk '{print $1}')
    WORKTREE_REF=$(echo "$line" | sed 's/^[^ ]* \(.*\)$/\1/')

    # Skip the main worktree
    if [ "$WORKTREE_PATH" = "$REPO_DIR" ]; then
        [[ "$VERBOSE" == true ]] && info "Skipping main worktree: $WORKTREE_PATH"
        continue
    fi

    # Check if worktree path exists
    if [ ! -d "$WORKTREE_PATH" ]; then
        if [[ "$VERBOSE" == true ]]; then
            warning "Worktree path missing but registered: $WORKTREE_PATH"
        fi
        WORKTREES_TO_REMOVE+=("$WORKTREE_PATH")
        continue
    fi

    # Get the last modification time of the worktree
    WORKTREE_MTIME=$(stat -f%m "$WORKTREE_PATH" 2>/dev/null || stat -c%Y "$WORKTREE_PATH" 2>/dev/null)

    if [ -z "$WORKTREE_MTIME" ]; then
        if [[ "$VERBOSE" == true ]]; then
            warning "Could not determine modification time for: $WORKTREE_PATH"
        fi
        continue
    fi

    # Calculate age in days
    AGE_SECONDS=$((CURRENT_TIME - WORKTREE_MTIME))
    AGE_DAYS=$((AGE_SECONDS / 86400))

    # Get size of worktree
    WORKTREE_SIZE=$(du -sh "$WORKTREE_PATH" 2>/dev/null | awk '{print $1}')
    WORKTREE_SIZE_BYTES=$(du -sb "$WORKTREE_PATH" 2>/dev/null | awk '{print $1}')

    if [ "$AGE_SECONDS" -gt "$((DAYS_THRESHOLD * 86400))" ]; then
        echo -e "${YELLOW}Stale:${NC} $WORKTREE_PATH"
        echo "  Branch: $WORKTREE_REF"
        echo "  Age: $AGE_DAYS days"
        echo "  Size: $WORKTREE_SIZE"
        WORKTREES_TO_REMOVE+=("$WORKTREE_PATH")
        TOTAL_SIZE=$((TOTAL_SIZE + WORKTREE_SIZE_BYTES))
    else
        [[ "$VERBOSE" == true ]] && echo -e "${GREEN}Active:${NC} $WORKTREE_PATH (Age: $AGE_DAYS days, Size: $WORKTREE_SIZE)"
    fi
done < <(git -C "$REPO_DIR" worktree list)

echo ""

if [ ${#WORKTREES_TO_REMOVE[@]} -eq 0 ]; then
    success "No stale worktrees found!"
    exit 0
fi

# Calculate total size in human-readable format
TOTAL_SIZE_MB=$((TOTAL_SIZE / 1024 / 1024))
if [ "$TOTAL_SIZE_MB" -lt 1024 ]; then
    TOTAL_SIZE_READABLE="${TOTAL_SIZE_MB}MB"
else
    TOTAL_SIZE_GB=$(echo "scale=2; $TOTAL_SIZE_MB / 1024" | bc)
    TOTAL_SIZE_READABLE="${TOTAL_SIZE_GB}GB"
fi

warning "Found ${#WORKTREES_TO_REMOVE[@]} stale worktree(s) that could free up $TOTAL_SIZE_READABLE"
echo ""

if [ "$DRY_RUN" = true ]; then
    info "DRY RUN: The following worktrees would be removed:"
    printf '  - %s\n' "${WORKTREES_TO_REMOVE[@]}"
    exit 0
fi

if [ "$AUTO_MODE" = false ]; then
    read -p "Remove stale worktrees? (y/n) " -n 1 -r
    echo
    if [[ ! $REPLY =~ ^[Yy]$ ]]; then
        info "Cleanup cancelled"
        exit 0
    fi
fi

# Remove stale worktrees
REMOVED_COUNT=0
for WORKTREE_PATH in "${WORKTREES_TO_REMOVE[@]}"; do
    if ! [ -d "$WORKTREE_PATH" ]; then
        if [[ "$VERBOSE" == true ]]; then
            info "Worktree already removed: $WORKTREE_PATH"
        fi
        ((REMOVED_COUNT++))
        continue
    fi

    if git -C "$REPO_DIR" worktree remove --force "$WORKTREE_PATH" 2>/dev/null; then
        success "Removed: $WORKTREE_PATH"
        ((REMOVED_COUNT++))
    else
        error "Failed to remove: $WORKTREE_PATH"
    fi
done

echo ""
success "Cleanup complete! Removed $REMOVED_COUNT worktree(s), freed up approximately $TOTAL_SIZE_READABLE"
