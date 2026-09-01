#!/bin/sh
set -eu

if [ "$#" -lt 2 ] || [ "$#" -gt 3 ]; then
  echo "usage: $0 GENERATED_REPOSITORY RESULT_DIRECTORY [ORC_BINARY]" >&2
  exit 2
fi

repo=$1
result_dir=$2
orc_bin=${3:-orc}
registry_path="$result_dir/registry/agents.db"

if [ ! -f "$result_dir/manifest.json" ] || [ ! -f "$registry_path" ]; then
  echo "result directory does not contain a prepared trial manifest and registry" >&2
  exit 1
fi

export ORC_GLOBAL_REGISTRY_PATH="$registry_path"
mkdir -p "$result_dir/tasks"

(cd "$repo" && "$orc_bin" task list) > "$result_dir/task-list.txt"
for task_id in T-0001 T-0002 T-0003 T-0004 T-0005; do
  (cd "$repo" && "$orc_bin" task show "$task_id") > "$result_dir/tasks/$task_id.txt"
done
(cd "$repo" && "$orc_bin" economy show) > "$result_dir/economy-final.json"
(cd "$repo" && "$orc_bin" economy context) > "$result_dir/context-final.json"
{
  printf 'head=%s\n' "$(git -C "$repo" rev-parse HEAD)"
  printf 'branch=%s\n' "$(git -C "$repo" branch --show-current)"
  git -C "$repo" status --short
  git -C "$repo" log --oneline --decorate -10
} > "$result_dir/git-final.txt"
