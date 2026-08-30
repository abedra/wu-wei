#!/usr/bin/env bash
# Bump the project version in one shot so a release can't go out half-done:
#
#   1. rewrite [package] version in Cargo.toml
#   2. sync Cargo.lock to match      (otherwise `cargo build --locked` fails in CI)
#   3. sanity-check the lockfile is consistent (the same check CI's --locked runs)
#   4. commit Cargo.toml + Cargo.lock as "Release vX.Y.Z"
#   5. create annotated tag vX.Y.Z   (pushing it triggers .github/workflows/release.yml,
#                                     which asserts the tag matches Cargo.toml)
#
# Nothing is pushed unless --push is given; otherwise the exact push command is
# printed for you to run.
#
# Usage:
#   scripts/bump-version.sh 0.1.4            # explicit version
#   scripts/bump-version.sh patch            # 0.1.3 -> 0.1.4
#   scripts/bump-version.sh minor            # 0.1.3 -> 0.2.0
#   scripts/bump-version.sh major            # 0.1.3 -> 1.0.0
#   scripts/bump-version.sh patch --push     # also push the branch and the tag
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

push=0
arg=""
for a in "$@"; do
	case "$a" in
		--push) push=1 ;;
		-h|--help)
			echo "usage: bump-version.sh <X.Y.Z|patch|minor|major> [--push]"
			exit 0
			;;
		-*) echo "unknown flag: $a" >&2; exit 2 ;;
		*)
			[ -z "$arg" ] || { echo "unexpected extra argument: $a" >&2; exit 2; }
			arg="$a"
			;;
	esac
done
[ -n "$arg" ] || { echo "usage: bump-version.sh <X.Y.Z|patch|minor|major> [--push]" >&2; exit 2; }

pkg=$(awk -F'"' '/^name = "/ { print $2; exit }' Cargo.toml)
current=$(awk -F'"' '/^version = "/ { print $2; exit }' Cargo.toml)
IFS=. read -r major minor patch <<<"$current"

case "$arg" in
	major) new="$((major + 1)).0.0" ;;
	minor) new="${major}.$((minor + 1)).0" ;;
	patch) new="${major}.${minor}.$((patch + 1))" ;;
	*)     new="$arg" ;;
esac

if ! [[ "$new" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
	echo "not a version or a bump keyword (patch|minor|major): $arg" >&2
	exit 2
fi

# --- preconditions -------------------------------------------------------------
if [ -n "$(git status --porcelain)" ]; then
	echo "working tree is not clean; commit or stash first" >&2
	exit 1
fi
if [ "$(git symbolic-ref --short HEAD)" != "master" ]; then
	echo "not on master (on $(git symbolic-ref --short HEAD)); switch before tagging a release" >&2
	exit 1
fi
if git rev-parse -q --verify "refs/tags/v$new" >/dev/null; then
	echo "tag v$new already exists" >&2
	exit 1
fi
if [ "$new" = "$current" ]; then
	echo "version is already $new" >&2
	exit 1
fi

echo "bumping $current -> $new"

# --- 1. Cargo.toml (only the first `version = ` line, i.e. [package]) ----------
awk -v v="$new" '
	!done && /^version = "/ { sub(/"[^"]*"/, "\"" v "\""); done = 1 }
	{ print }
' Cargo.toml > Cargo.toml.tmp && mv Cargo.toml.tmp Cargo.toml

# --- 2. Cargo.lock -----------------------------------------------------------
cargo update --offline --quiet -p "$pkg" 2>/dev/null || cargo update --quiet -p "$pkg"

# --- 3. verify the lockfile is what CI's `--locked` will accept ---------------
if ! cargo metadata --format-version 1 --locked >/dev/null; then
	echo "Cargo.lock is still out of sync after the bump; aborting without committing" >&2
	git checkout -- Cargo.toml Cargo.lock
	exit 1
fi

# --- 4 + 5. commit and tag --------------------------------------------------
git add Cargo.toml Cargo.lock
git commit -m "Release v$new"
git tag -a "v$new" -m "Release v$new"

echo
echo "committed and tagged v$new"
if [ "$push" -eq 1 ]; then
	git push origin HEAD
	git push origin "v$new"
	echo "pushed; release workflow will build v$new"
else
	echo "next:  git push origin HEAD && git push origin v$new"
fi
