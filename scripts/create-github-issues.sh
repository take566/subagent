#!/usr/bin/env bash
# GitHub Issue 一括作成（並行開発バックログ）
# Usage: ./scripts/create-github-issues.sh
# Requires: gh auth, write permission to the repo
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ISSUE_DIR="$ROOT/docs/issues"

if ! command -v gh >/dev/null; then
  echo "gh CLI が必要です" >&2
  exit 1
fi

created=0
for file in "$ISSUE_DIR"/[A-Z]*-*.md; do
  [[ -f "$file" ]] || continue
  id="$(basename "$file" .md)"
  # frontmatter 風の先頭メタを抽出
  title="$(awk '/^title:/{sub(/^title:[[:space:]]*/,""); print; exit}' "$file")"
  labels="$(awk '/^labels:/{sub(/^labels:[[:space:]]*/,""); print; exit}' "$file")"
  body_file="$(mktemp)"
  # YAML frontmatter 以降を本文に
  awk 'BEGIN{fm=0} /^---$/{fm++; next} fm>=2{print}' "$file" > "$body_file"

  if [[ -z "$title" ]]; then
    echo "skip (no title): $file" >&2
    rm -f "$body_file"
    continue
  fi

  label_args=()
  IFS=',' read -ra parts <<< "$labels"
  for l in "${parts[@]}"; do
    l="$(echo "$l" | xargs)"
    [[ -n "$l" ]] && label_args+=(--label "$l")
  done

  echo "Creating: $title"
  gh issue create --title "$title" --body-file "$body_file" "${label_args[@]}"
  rm -f "$body_file"
  created=$((created + 1))
done

echo "Created $created issues."
