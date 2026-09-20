#!/usr/bin/env bash
# Prints the CHANGELOG.md section for one version: everything under the
# `## [<version>]` heading up to the next `## ` heading. Fails when there is
# no such section, so a tag without a changelog entry cannot be released.
#
# usage: .github/scripts/release-notes.sh <version>
set -euo pipefail

version=${1:?usage: release-notes.sh <version>}

notes=$(awk -v heading="## [$version]" '
  /^## / { on = index($0, heading) == 1; next }
  on     { print }
' CHANGELOG.md)

if [ -z "${notes//[[:space:]]/}" ]; then
  echo "CHANGELOG.md has no section for version $version" >&2
  exit 1
fi
printf '%s\n' "$notes"
