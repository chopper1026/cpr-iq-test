#!/usr/bin/env bash
set -euo pipefail
if [[ ${RUNNER_OS:?run this on a GitHub-hosted runner} == macOS ]]; then
  brew install postgresql@17 redis
  echo "$(brew --prefix postgresql@17)/bin" >> "$GITHUB_PATH"
else
  sudo apt-get update -qq
  sudo apt-get install -y postgresql redis-server
  echo "$(pg_config --bindir)" >> "$GITHUB_PATH"
fi
