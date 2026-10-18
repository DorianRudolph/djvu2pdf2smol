#!/bin/sh
# Build the web version and publish it as the `pages` branch, which GitHub Pages serves
# (Settings -> Pages -> Deploy from a branch -> pages, / (root)).
#
#   web/deploy-pages.sh            build, replace `pages` with one fresh commit, force-push
#   web/deploy-pages.sh --no-push  build and update the local branch only
#
# The branch holds only the built site (web/dist) in a single commit without history, so
# old builds do not accumulate in the repository.
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
touch web/dist/.nojekyll # serve files as they are, no Jekyll processing

# commit web/dist as a root commit, using a throwaway index
index=$(mktemp)
trap 'rm -f "$index"' EXIT
rm -f "$index"
GIT_INDEX_FILE="$index" git --work-tree=web/dist add -A .
tree=$(GIT_INDEX_FILE="$index" git write-tree)
commit=$(git commit-tree "$tree" -m "Deploy web version built from $source_rev")
git update-ref refs/heads/pages "$commit"
echo "deploy-pages: pages -> $(git rev-parse --short "$commit") (built from $source_rev)"

if [ "$push" = 1 ]; then
  git push --force origin pages
fi
