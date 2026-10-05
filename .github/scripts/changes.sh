#!/usr/bin/env bash
# Which jobs a change needs, for CI (ci.yml) and the image (publish.yml).
#
#   changes.sh pr <number>   the files a pull request changes
#   changes.sh since <sha>   the files changed between <sha> and HEAD
#   changes.sh stdin         the files listed on standard input (to try it locally)
#   changes.sh all           everything runs (started by hand, a tag, or no base to compare)
#
# Prints name=true|false lines for $GITHUB_OUTPUT. A job is skipped only when every
# changed file is one it can't be affected by, so a file nobody listed here (a new
# folder, a new build input) runs everything. When the list can't be had, everything
# runs too.
set -euo pipefail

parts=(server desktop proto docker image sdk)

everything() {
  for part in "${parts[@]}"; do echo "$part=true"; done
  exit 0
}

case "${1:-all}" in
  pr)
    # The pull request's own files (renames count both names). The API lists up to
    # 3000; a bigger one runs everything.
    [ "${CHANGED_FILES:-0}" -lt 3000 ] || everything
    files=$(gh api "repos/$GITHUB_REPOSITORY/pulls/$2/files" --paginate \
      --jq '.[] | .filename, (.previous_filename // empty)') || everything
    ;;
  since)
    base=$2
    [[ $base =~ ^[0-9a-f]{40}$ && $base != 0000000000000000000000000000000000000000 ]] || everything
    git fetch --no-tags --quiet --depth=1 origin "$base" 2>/dev/null || everything
    files=$(git diff --name-only --no-renames "$base" HEAD) || everything
    ;;
  stdin) files=$(cat) ;;
  *) everything ;;
esac

# Files that can't change a part's result.
top_md() { [[ $1 != */* && $1 == *.md ]]; }
not_for_server() {
  top_md "$1" && return 0
  case $1 in
    docs/* | desktop/* | sdk/* | .railway/* | deploy/* | LICENSE* | package.json | package-lock.json | \
    Dockerfile | .dockerignore | \
    .github/workflows/desktop.yml | .github/workflows/release.yml | .github/workflows/publish.yml | \
    .github/workflows/reproducible.yml | .github/workflows/railway-config.yml | .github/workflows/sdk-release.yml) return 0 ;;
  esac
  return 1
}
# The desktop app is its own workspace; it builds e2ee, and the server for its tests.
not_for_desktop() {
  top_md "$1" && return 0
  case $1 in
    # The desktop builds its standard emoji from this one.
    web/src/lib/emoji-data.json) return 1 ;;
    docs/* | web/* | sdk/* | e2ee-wasm/* | .railway/* | deploy/* | LICENSE* | package.json | package-lock.json | \
    Cargo.lock | Dockerfile | .dockerignore | buf.yaml | buf.lock | \
    .github/workflows/desktop.yml | .github/workflows/release.yml | .github/workflows/publish.yml | \
    .github/workflows/reproducible.yml | .github/workflows/railway-config.yml | .github/workflows/sdk-release.yml) return 0 ;;
  esac
  return 1
}
# buf lint reads only these.
for_proto() {
  case $1 in
    proto/* | buf.yaml | buf.lock | .github/workflows/ci.yml | .github/scripts/*) return 0 ;;
  esac
  return 1
}
# What the Dockerfile copies in to build with, besides the code itself: a pull
# request changing one of these builds the image too (fuwa#78 broke it that way).
for_docker() {
  case $1 in
    Dockerfile | .dockerignore | Cargo.toml | Cargo.lock | rustfmt.toml | \
    server/Cargo.toml | server/build.rs | e2ee/Cargo.toml | e2ee-wasm/Cargo.toml | voice/Cargo.toml | \
    web/package.json | web/pnpm-lock.yaml | web/pnpm-workspace.yaml | web/vite.config.ts | \
    web/tsconfig*.json | web/index.html | web/scripts/* | \
    .github/workflows/ci.yml | .github/workflows/publish.yml | .github/scripts/*) return 0 ;;
  esac
  return 1
}
# The image is built from the files .dockerignore lets in (.github is left out), so
# these can't change it.
not_for_image() {
  top_md "$1" && return 0
  case $1 in
    docs/* | desktop/* | sdk/* | .railway/* | deploy/* | LICENSE* | package.json | package-lock.json | \
    buf.yaml | buf.lock | \
    .github/workflows/ci.yml | .github/workflows/desktop.yml | .github/workflows/release.yml | \
    .github/workflows/reproducible.yml | .github/workflows/railway-config.yml | .github/workflows/sdk-release.yml) return 0 ;;
  esac
  return 1
}

# The SDK (sdk/): its own code, the protocol it's generated from, and how CI runs it.
# Its tests run a real instance, but a server change alone doesn't run them.
for_sdk() {
  case $1 in
    sdk/* | proto/* | buf.yaml | buf.lock | .github/workflows/ci.yml | .github/workflows/sdk-release.yml | \
    .github/scripts/*) return 0 ;;
  esac
  return 1
}

server=false desktop=false proto=false docker=false image=false sdk=false
count=0
while IFS= read -r f; do
  [ -n "$f" ] || continue
  count=$((count + 1))
  not_for_server "$f" || server=true
  not_for_desktop "$f" || desktop=true
  for_proto "$f" && proto=true
  for_docker "$f" && docker=true
  not_for_image "$f" || image=true
  for_sdk "$f" && sdk=true
done <<< "$files"
# Nothing changed (an empty commit, or a merge that only moved history): run nothing.
echo "changed files: $count" >&2
printf 'server=%s\ndesktop=%s\nproto=%s\ndocker=%s\nimage=%s\nsdk=%s\n' "$server" "$desktop" "$proto" "$docker" "$image" "$sdk"
