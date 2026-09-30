#!/usr/bin/env bash
# Prepare one Lumos workspace release.
#
# Usage:
#   scripts/prepare-release.sh 0.1.2
#
# This script updates the workspace package version, every internal package
# constraint used by the workspace, and Cargo.lock. It does not commit, tag,
# or push; inspect the result before creating an immutable crates.io release.

set -euo pipefail

readonly internal_packages=(
  lumos
  lumos-core
  lumos-cli
  lumos-macros
  lumos-testing
  rusticate
  lumos-jsonapi
)

usage() {
  printf '%s\n' 'Usage: scripts/prepare-release.sh MAJOR.MINOR.PATCH'
}

if (($# != 1)); then
  usage >&2
  exit 2
fi

target_version="$1"
if [[ ! "$target_version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  printf '%s\n' 'Release versions must use MAJOR.MINOR.PATCH, for example 0.1.2.' >&2
  exit 2
fi

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

git diff --quiet || {
  printf '%s\n' 'Refusing to prepare a release from a worktree with unstaged changes.' >&2
  exit 1
}
git diff --cached --quiet || {
  printf '%s\n' 'Refusing to prepare a release from a worktree with staged changes.' >&2
  exit 1
}

manifest='Cargo.toml'
current_version="$(awk '
  /^\[workspace\.package\]$/ { in_workspace_package = 1; next }
  /^\[/ { in_workspace_package = 0 }
  in_workspace_package && $1 == "version" {
    gsub(/"/, "", $3)
    print $3
    exit
  }
' "$manifest")"

[[ -n "$current_version" ]] || {
  printf '%s\n' 'Could not read workspace.package.version from Cargo.toml.' >&2
  exit 1
}

[[ "$current_version" != "$target_version" ]] || {
  printf 'Workspace version is already %s.\n' "$target_version" >&2
  exit 1
}

temporary_manifest="$(mktemp "${manifest}.release.XXXXXX")"
trap 'rm -f "$temporary_manifest"' EXIT

awk -v target_version="$target_version" -v package_list="${internal_packages[*]}" '
  BEGIN {
    package_count = split(package_list, packages, " ")
  }

  /^\[workspace\.package\]$/ {
    in_workspace_package = 1
    print
    next
  }

  /^\[/ {
    in_workspace_package = 0
  }

  in_workspace_package && $1 == "version" {
    sub(/"[^"]+"/, "\"" target_version "\"")
    workspace_version_count += 1
    print
    next
  }

  {
    for (package_index = 1; package_index <= package_count; package_index += 1) {
      package = packages[package_index]
      pattern = "^" package "[[:space:]]*=[[:space:]]*\\{.*version[[:space:]]*="
      if ($0 ~ pattern) {
        sub(/version[[:space:]]*=[[:space:]]*"[^"]+"/, "version = \"" target_version "\"")
        package_version_count[package] += 1
      }
    }
    print
  }

  END {
    if (workspace_version_count != 1) {
      exit 1
    }

    for (package_index = 1; package_index <= package_count; package_index += 1) {
      if (package_version_count[packages[package_index]] != 1) {
        exit 1
      }
    }
  }
' "$manifest" > "$temporary_manifest" || {
  printf '%s\n' 'Cargo.toml did not contain the expected release-version fields.' >&2
  exit 1
}

mv "$temporary_manifest" "$manifest"
trap - EXIT

cargo metadata --no-deps --format-version 1 >/dev/null
cargo check --workspace

printf 'Prepared Lumos %s. Review the changes, then run:\n' "$target_version"
printf '  git add Cargo.toml Cargo.lock\n'
printf '  git commit -m "Release v%s"\n' "$target_version"
printf '  git tag v%s\n' "$target_version"
printf '  git push origin main v%s\n' "$target_version"
