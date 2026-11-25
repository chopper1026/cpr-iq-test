#!/usr/bin/env bash
# The plugin checks the wire request itself; this independently checks CPR's no-rotation guarantee.
set -euo pipefail
source_directory=${1:?host source required}
toolchain=${2:?host Rust channel required}
export RUST_MIN_STACK=16777216
for test in required_account_disables_account_retry_after_stream_creation transient_retry_is_suppressed_for_required_accounts_and_committed_output; do
  output=$(cargo "+$toolchain" test --locked --manifest-path "$source_directory/backend/Cargo.toml" -p gateway-core --test main "$test" -- --nocapture)
  printf '%s\n' "$output"
  grep -q '^running 1 test$' <<<"$output"
  grep -Eq "^test .*::$test \.\.\. ok$" <<<"$output"
done
