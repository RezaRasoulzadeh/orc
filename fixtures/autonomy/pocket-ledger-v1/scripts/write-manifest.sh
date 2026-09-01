#!/bin/sh
set -eu

if [ "$#" -ne 4 ]; then
  echo "usage: $0 GENERATED_REPOSITORY RESULT_DIRECTORY RUN_ID ORC_BINARY" >&2
  exit 2
fi

generated_repo=$1
result_dir=$2
run_id=$3
orc_bin=$4

case "$run_id" in
  *[!A-Za-z0-9._-]*|'') echo "run id must use only letters, digits, dot, underscore, or dash" >&2; exit 2 ;;
esac

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
fixture_dir=$(dirname -- "$script_dir")
orc_root=$(git -C "$script_dir" rev-parse --show-toplevel)
. "$fixture_dir/trial.env"

mkdir -p "$result_dir"
fixture_hash=$(cd "$fixture_dir" && find . -type f -not -path './repository/target/*' -print | LC_ALL=C sort | xargs sha256sum | sha256sum | awk '{print $1}')
validation_hash=$(sha256sum "$fixture_dir/repository/.orc/validation.toml" | awk '{print $1}')
agent_economy_hash=$(sha256sum "$fixture_dir/trial.env" | awk '{print $1}')
seed_commit=$(git -C "$generated_repo" rev-list --max-parents=0 HEAD)
baseline_commit=$(git -C "$generated_repo" rev-parse HEAD)
orc_commit=$(git -C "$orc_root" rev-parse HEAD)
if [ -z "$(git -C "$orc_root" status --porcelain --untracked-files=all)" ]; then
  orc_clean=true
else
  orc_clean=false
fi
created_at=$(date -u '+%Y-%m-%dT%H:%M:%SZ')
orc_version=$($orc_bin --version | tr -d '\r\n')

task_1=$(sha256sum "$fixture_dir/tasks/T-0001.json" | awk '{print $1}')
task_2=$(sha256sum "$fixture_dir/tasks/T-0002.json" | awk '{print $1}')
task_3=$(sha256sum "$fixture_dir/tasks/T-0003.json" | awk '{print $1}')
task_4=$(sha256sum "$fixture_dir/tasks/T-0004.json" | awk '{print $1}')
task_5=$(sha256sum "$fixture_dir/tasks/T-0005.json" | awk '{print $1}')

{
  printf '%s\n' '{'
  printf '  "fixture_version": "%s",\n' "$FIXTURE_VERSION"
  printf '  "fixture_tree_sha256": "%s",\n' "$fixture_hash"
  printf '  "orc_commit": "%s",\n' "$orc_commit"
  printf '  "orc_worktree_clean": %s,\n' "$orc_clean"
  printf '  "orc_version": "%s",\n' "$orc_version"
  printf '  "seed_baseline_commit": "%s",\n' "$seed_commit"
  printf '  "generated_external_repo_baseline_commit": "%s",\n' "$baseline_commit"
  printf '  "validation_config_sha256": "%s",\n' "$validation_hash"
  printf '  "agent_economy_config_sha256": "%s",\n' "$agent_economy_hash"
  printf '%s\n' '  "task_contract_sha256": {'
  printf '    "T-0001": "%s",\n' "$task_1"
  printf '    "T-0002": "%s",\n' "$task_2"
  printf '    "T-0003": "%s",\n' "$task_3"
  printf '    "T-0004": "%s",\n' "$task_4"
  printf '    "T-0005": "%s"\n' "$task_5"
  printf '%s\n' '  },'
  printf '  "provider": "%s",\n' "$TRIAL_PROVIDER"
  printf '  "default_model": "%s",\n' "$DEFAULT_MODEL"
  printf '  "escalation_model": "%s",\n' "$ESCALATION_MODEL"
  printf '  "created_at": "%s",\n' "$created_at"
  printf '  "trial_run_id": "%s"\n' "$run_id"
  printf '%s\n' '}'
} > "$result_dir/manifest.json"
