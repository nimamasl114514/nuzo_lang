#!/bin/bash
# Conventional Commits format check
# Usage: check_commit_msg.sh <commit-msg-file>

msg_file="$1"
first_line=$(head -n1 "$msg_file" | grep -v '^#')

# Allow merge/revert commits
if echo "$first_line" | grep -qE '^(Merge|Revert) '; then
    echo "OK: merge/revert commit"
    exit 0
fi

# Check Conventional Commits format
if echo "$first_line" | grep -qE '^(feat|fix|docs|refactor|test|chore|perf|build|ci|style|revert)(\(.+\))?: .+'; then
    echo "OK: commit message format correct"
    exit 0
fi

echo "ERROR: commit message must follow Conventional Commits"
echo "Format: <type>[(scope)]: <description>"
echo "Types: feat, fix, docs, refactor, test, chore, perf, build, ci, style, revert"
exit 1
