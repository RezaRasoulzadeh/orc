#!/bin/sh
set -eu

if [ "$#" -lt 3 ] || [ "$#" -gt 4 ]; then
  echo "usage: $0 TARGET_REPOSITORY RESULT_ROOT RUN_ID [ORC_BINARY]" >&2
  echo "TRIAL_PROFILE_PATH must name an authenticated provider profile directory." >&2
  exit 2
fi

target_repo=$1
result_root=$2
run_id=$3
orc_bin=${4:-orc}
profile_path=${TRIAL_PROFILE_PATH:-}

case "$orc_bin" in
  */*) orc_bin=$(CDPATH= cd -- "$(dirname -- "$orc_bin")" && pwd)/$(basename -- "$orc_bin") ;;
esac

if [ -z "$profile_path" ] || [ ! -d "$profile_path" ]; then
  echo "TRIAL_PROFILE_PATH must name an existing authenticated provider profile directory" >&2
  exit 2
fi

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
fixture_dir=$(dirname -- "$script_dir")
. "$fixture_dir/trial.env"

result_dir="$result_root/$run_id"
registry_path="$result_dir/registry/agents.db"
plan_path="$result_dir/task-plan.json"

if [ -e "$result_dir" ]; then
  echo "result directory already exists: $result_dir" >&2
  exit 1
fi

mkdir -p "$result_dir/registry"
"$script_dir/create-repository.sh" "$target_repo" > "$result_dir/seed-baseline-commit.txt"
"$script_dir/verify-baseline.sh" "$target_repo" > "$result_dir/baseline-validation.txt"
"$script_dir/build-plan.sh" "$plan_path"

export ORC_GLOBAL_REGISTRY_PATH="$registry_path"

(cd "$target_repo" && "$orc_bin" adopt)

git -C "$target_repo" add .orc .gitignore
GIT_AUTHOR_NAME="Orc Autonomy Fixture" \
GIT_AUTHOR_EMAIL="fixture@orc.invalid" \
GIT_AUTHOR_DATE="2026-09-01T00:01:00Z" \
GIT_COMMITTER_NAME="Orc Autonomy Fixture" \
GIT_COMMITTER_EMAIL="fixture@orc.invalid" \
GIT_COMMITTER_DATE="2026-09-01T00:01:00Z" \
  git -C "$target_repo" commit -q -m "Configure Orc pocket-ledger-v1 trial"

onboard_agent() {
  agent_id=$1
  model=$2
  effort=$3
  priority=$4
  set -- "$orc_bin" agent onboard "$agent_id" --backend "$TRIAL_PROVIDER" \
    --profile "$profile_path" --model "$model" --effort "$effort" \
    --priority "$priority" --approve
  for action in $TRIAL_ACTIONS; do
    set -- "$@" --role "$action"
  done
  for capability in $TRIAL_CAPABILITIES; do
    set -- "$@" --capability "$capability"
  done
  for permission in $TRIAL_PERMISSIONS; do
    set -- "$@" --permission "$permission"
  done
  (cd "$target_repo" && "$@")
  (cd "$target_repo" && "$orc_bin" agent attach "$agent_id")
}

onboard_agent "$DEFAULT_AGENT_ID" "$DEFAULT_MODEL" "$DEFAULT_REASONING_EFFORT" "$DEFAULT_PRIORITY"
onboard_agent "$ESCALATION_AGENT_ID" "$ESCALATION_MODEL" "$ESCALATION_REASONING_EFFORT" "$ESCALATION_PRIORITY"

(cd "$target_repo" && "$orc_bin" economy configure \
  --model-cost "$DEFAULT_MODEL=$DEFAULT_MODEL_COST" \
  --model-cost "$ESCALATION_MODEL=$ESCALATION_MODEL_COST" \
  --unknown-tier "$UNKNOWN_ECONOMY_TIER")
(cd "$target_repo" && "$orc_bin" apply-plan "$plan_path") > "$result_dir/task-id-map.json"
(cd "$target_repo" && "$orc_bin" agent list) > "$result_dir/agents.txt"
(cd "$target_repo" && "$orc_bin" economy show) > "$result_dir/economy-initial.json"
(cd "$target_repo" && "$orc_bin" schedule T-0001 --explain --mode automated) > "$result_dir/schedule-T-0001.txt"

if ! grep -q "$DEFAULT_AGENT_ID" "$result_dir/schedule-T-0001.txt"; then
  echo "default agent was not selected for representative task" >&2
  exit 1
fi
if grep -Eq 'src/App\.vue|Orc.*self-host|desktop control plane' "$target_repo/.orc/engineering.md"; then
  echo "adopted repository received Orc self-hosting instructions" >&2
  exit 1
fi
if [ -n "$(git -C "$target_repo" status --porcelain --untracked-files=all)" ]; then
  echo "prepared trial repository is not clean" >&2
  exit 1
fi

"$script_dir/write-manifest.sh" "$target_repo" "$result_dir" "$run_id" "$orc_bin"
printf '%s\n' "$result_dir"
