#!/usr/bin/env bash
set -euo pipefail

if [[ -z "${CDK_PATH:-}" ]]; then
  echo "Error: CDK_PATH is not set."
  echo "Example: export CDK_PATH=/absolute/path/to/cdk/crates/cdk"
  exit 1
fi

echo "Using CDK_PATH: $CDK_PATH"

find . -name Cargo.toml | while read -r file; do
  if grep -qE 'cdk(-[a-z0-9_-]+)? = \{ git = "https://github.com/vnprc/cdk' "$file"; then
    echo "✅ Patching: $file"
    cp "$file" "$file.bak"

    awk -v cdk_path="$CDK_PATH" '
      function get_realpath(base, subcrate, result, cmd) {
        cmd = "realpath " base "/../" subcrate
        cmd | getline result
        close(cmd)
        return result
      }

      {
        if ($0 ~ /^cdk = \{ git = "https:\/\/github.com\/vnprc\/cdk/) {
          print "cdk = { path = \"" cdk_path "\" }"
        }
        else if ($0 ~ /^cdk-[a-z0-9_-]+ = \{ git = "https:\/\/github.com\/vnprc\/cdk.*package = "[^"]+"/) {
          match($0, /^cdk-([a-z0-9_-]+) = .*package = "([^"]+)"/, m)
          crate = m[2]
          resolved = get_realpath(cdk_path, crate)
          print "cdk-" m[1] " = { path = \"" resolved "\" }"
        }
        else {
          print
        }
      }
    ' "$file.bak" > "$file"
  fi
done
