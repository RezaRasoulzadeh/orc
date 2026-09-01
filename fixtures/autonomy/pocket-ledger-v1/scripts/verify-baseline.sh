#!/bin/sh
set -eu

if [ "$#" -ne 1 ]; then
  echo "usage: $0 GENERATED_REPOSITORY" >&2
  exit 2
fi

repo=$1
(cd "$repo" && cargo fmt --check)
(cd "$repo" && cargo clippy --all-targets -- -D warnings)
(cd "$repo" && cargo test)

if [ -n "$(git -C "$repo" status --porcelain --untracked-files=all)" ]; then
  echo "repository is not clean after baseline validation" >&2
  exit 1
fi

git -C "$repo" rev-parse HEAD
