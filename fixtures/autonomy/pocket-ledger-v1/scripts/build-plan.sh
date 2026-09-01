#!/bin/sh
set -eu

if [ "$#" -ne 1 ]; then
  echo "usage: $0 OUTPUT_JSON" >&2
  exit 2
fi

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
fixture_dir=$(dirname -- "$script_dir")
output_path=$1

{
  printf '%s\n' '{'
  printf '%s\n' '  "protocol_version": 1,'
  printf '%s\n' '  "objective": "Reproduce the pocket-ledger-v1 external autonomy benchmark.",'
  printf '%s\n' '  "assumptions": ["Provider credentials and live quota are external to the fixture."],'
  printf '%s\n' '  "risks": ["Semantic Review must distinguish Suspended from Active in Task 5."],'
  printf '%s\n' '  "questions": [],'
  printf '%s\n' '  "tasks": ['
  first=true
  for task_file in "$fixture_dir"/tasks/T-*.json; do
    if [ "$first" = true ]; then
      first=false
    else
      printf '%s\n' ','
    fi
    sed 's/^/    /' "$task_file"
  done
  printf '%s\n' '  ]'
  printf '%s\n' '}'
} > "$output_path"
