#!/usr/bin/env bash
# Copies a release's latest.json (written by tauri-action) onto the `updates`
# branch as <channel>.json for each channel given. The app reads
#   https://raw.githubusercontent.com/<repo>/updates/<channel>.json
#
# usage: publish-channel.sh <release tag> <channel> [channel ...]
# needs: GH_TOKEN, GITHUB_REPOSITORY
set -euo pipefail

tag="$1"; shift
work="$(mktemp -d)"

gh release download "$tag" --repo "$GITHUB_REPOSITORY" --pattern latest.json --dir "$work/in"

remote="https://x-access-token:${GH_TOKEN}@github.com/${GITHUB_REPOSITORY}.git"
if git ls-remote --exit-code --heads "$remote" updates >/dev/null 2>&1; then
  git clone --quiet --depth 1 --branch updates "$remote" "$work/updates"
else
  # First time: start an empty branch that only holds the manifests.
  git init --quiet -b updates "$work/updates"
  git -C "$work/updates" remote add origin "$remote"
  printf '# Update manifests\n\nWritten by the release workflows. The app reads <channel>.json from here. Do not edit by hand.\n' \
    > "$work/updates/README.md"
fi

for channel in "$@"; do
  cp "$work/in/latest.json" "$work/updates/$channel.json"
done

cd "$work/updates"
git config user.name "github-actions[bot]"
git config user.email "41898282+github-actions[bot]@users.noreply.github.com"
git add -A
if git diff --cached --quiet; then
  echo "manifests already up to date"
  exit 0
fi
git commit --quiet -m "$tag -> $*"
git push --quiet origin updates
echo "published $tag to: $*"
