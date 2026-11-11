#!/usr/bin/env bash
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")"
package=brilliantis
previous=""
for argument in "$@"; do
  if [[ "$previous" == "--profile" && "$argument" == "neurofunken" ]]; then
    package=brilliantis-neurofunken
  fi
  previous="$argument"
done
exec cargo run --locked --release --package "$package" -- --demo "$@"
