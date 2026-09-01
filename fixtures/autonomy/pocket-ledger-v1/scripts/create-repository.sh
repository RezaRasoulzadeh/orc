#!/bin/sh
set -eu

if [ "$#" -ne 1 ]; then
  echo "usage: $0 TARGET_DIRECTORY" >&2
  exit 2
fi

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
fixture_dir=$(dirname -- "$script_dir")
seed_dir="$fixture_dir/repository"
target_dir=$1

if [ -e "$target_dir" ]; then
  echo "target already exists: $target_dir" >&2
  exit 1
fi

mkdir -p "$target_dir"
cp -R "$seed_dir/." "$target_dir/"

git -C "$target_dir" init -q --initial-branch=main
git -C "$target_dir" config user.name "Orc Autonomy Fixture"
git -C "$target_dir" config user.email "fixture@orc.invalid"
git -C "$target_dir" config core.autocrlf false
git -C "$target_dir" config core.filemode true

(cd "$target_dir" && cargo generate-lockfile --offline)
git -C "$target_dir" add --all
GIT_AUTHOR_NAME="Orc Autonomy Fixture" \
GIT_AUTHOR_EMAIL="fixture@orc.invalid" \
GIT_AUTHOR_DATE="2026-09-01T00:00:00Z" \
GIT_COMMITTER_NAME="Orc Autonomy Fixture" \
GIT_COMMITTER_EMAIL="fixture@orc.invalid" \
GIT_COMMITTER_DATE="2026-09-01T00:00:00Z" \
  git -C "$target_dir" commit -q -m "Create pocket-ledger-v1 baseline"

if [ -n "$(git -C "$target_dir" status --porcelain --untracked-files=all)" ]; then
  echo "generated repository is not clean" >&2
  exit 1
fi

git -C "$target_dir" rev-parse HEAD

expected_commit=149d45a00b6d25d7bebcbcfaed398c59231dd376
actual_commit=$(git -C "$target_dir" rev-parse HEAD)
if [ "$actual_commit" != "$expected_commit" ]; then
  echo "seed baseline mismatch: expected $expected_commit, got $actual_commit" >&2
  exit 1
fi
