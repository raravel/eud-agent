#!/usr/bin/env bash
# Lint for multi-project scoping (not acceptance evidence; the behaviour tests
# are). It scans production Rust only: a file is read up to its first
# `#[cfg(test)]` module, and contract test files are skipped.
#
#   (a) the configured project path is read only by config, the registry, and
#       project setup (open, create, import, recents, launcher status);
#   (b) no new caller hashes a path into a project id: `ProjectId::of` is the
#       only way from an opened project to its id.
#
# Usage: scripts/check-project-scope.sh   (from the repository root)
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
src="$root/src-tauri/src"
status=0

# Print "<file>:<line>:<text>" for production lines of `file` matching `pattern`.
production_matches() {
  local pattern="$1" file="$2"
  awk -v pattern="$pattern" -v file="$file" '
    /^#\[cfg\(test\)\]/ { test_attr = NR; next }
    test_attr == NR - 1 && /^(pub(\([a-z]+\))? )?mod / { exit }
    $0 ~ pattern { printf "%s:%d:%s\n", file, NR, $0 }
  ' "$file"
}

check() {
  local label="$1" pattern="$2"
  shift 2
  local allowed=("$@")
  local found=0
  while IFS= read -r file; do
    local relative="${file#"$src"/}"
    case "$relative" in
      provider_runtime/contract_tests/*) continue ;;
    esac
    local skip=0
    for allow in "${allowed[@]}"; do
      [[ "$relative" == "$allow" ]] && skip=1
    done
    [[ $skip -eq 1 ]] && continue
    local hits
    hits="$(production_matches "$pattern" "$file")"
    if [[ -n "$hits" ]]; then
      echo "$hits"
      found=1
    fi
  done < <(find "$src" -name '*.rs' | sort)
  if [[ $found -eq 1 ]]; then
    echo "check-project-scope: $label" >&2
    status=1
  fi
}

check "(a) the configured project path is read outside config/registry/setup" \
  '[.]project_path([^a-z_]|$)|configured_project_root[(]' \
  config.rs project_registry.rs setup.rs native_runtime.rs

check "(b) a path is hashed into a project id outside ProjectId::of" \
  'project_id_for_path[(]' \
  map_context.rs project_id.rs project_rebind.rs

if [[ $status -eq 0 ]]; then
  echo "check-project-scope: ok"
fi
exit $status
