#!/bin/sh
# Build the web version and publish it as a commit on the `pages` branch, which GitHub Pages
# serves (Settings -> Pages -> Deploy from a branch -> pages, / (root)).
#
#   web/deploy-pages.sh            build, commit to `pages`, push
#   web/deploy-pages.sh --no-push  build and commit only
#
# The branch holds only the built site (web/dist); each deployment is one commit on top of
# the previous one, referencing the source commit it was built from.
set -eu
cd "$(dirname "$0")/.."

push=1
[ "${1:-}" = "--no-push" ] && push=0

if [ -n "$(git status --porcelain --untracked-files=no)" ]; then
  echo "deploy-pages: commit or stash your changes first (the site is built from HEAD)" >&2
  exit 1
fi
source_rev=$(git rev-parse --short HEAD)

web/build.sh

worktree=$(mktemp -d)
trap 'git worktree remove --force "$worktree" 2>/dev/null || true; rm -rf "$worktree"' EXIT

if git show-ref --verify --quiet refs/heads/pages; then
  git worktree add --quiet "$worktree" pages
elif git ls-remote --exit-code --heads origin pages >/dev/null 2>&1; then
  git fetch --quiet origin pages:pages
  git worktree add --quiet "$worktree" pages
else
  git worktree add --quiet --orphan -b pages "$worktree"
fi

# replace the branch contents with the fresh build
find "$worktree" -mindepth 1 -maxdepth 1 ! -name .git -exec rm -rf {} +
cp -R web/dist/. "$worktree/"
touch "$worktree/.nojekyll" # serve files as they are, no Jekyll processing

git -C "$worktree" add -A
if git -C "$worktree" diff --cached --quiet; then
  echo "deploy-pages: site unchanged"
else
  git -C "$worktree" commit --quiet -m "Deploy web version built from $source_rev"
  echo "deploy-pages: committed $(git -C "$worktree" rev-parse --short HEAD) to pages"
fi
if [ "$push" = 1 ]; then
  git push origin pages
fi
