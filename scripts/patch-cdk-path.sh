#!/usr/bin/env bash
set -euo pipefail

if [[ "${IN_DEVENV:-}" != "1" ]]; then
  echo "Error: Not inside devenv shell. Run 'devenv shell' first."
  exit 1
fi

if [[ -z "${CDK_PATH:-}" ]]; then
  echo "Error: CDK_PATH is not set."
  echo "Example: export CDK_PATH=/absolute/path/to/cdk  (root of cashubtc/cdk repo)"
  exit 1
fi

echo "Using CDK_PATH: $CDK_PATH"

patched=0

# Patch [patch.crates-io] sections in workspace Cargo.toml files.
# Individual crates use version deps (e.g. cdk = "0.15.1") resolved via these patches.
for file in roles/Cargo.toml protocols/Cargo.toml; do
  if [[ ! -f "$file" ]]; then
    continue
  fi

  if ! grep -qE 'cashubtc/cdk' "$file"; then
    continue
  fi

  echo "✅ Patching: $file"
  cp "$file" "$file.bak"

  awk -v cdk_path="$CDK_PATH" '
    /cashubtc\/cdk/ {
      match($0, /^([[:space:]]*)([a-z][a-z0-9_-]*)/, m)
      crate = m[2]
      printf "%s%s = { path = \"%s/crates/%s\" }\n", m[1], crate, cdk_path, crate
      next
    }
    { print }
  ' "$file.bak" > "$file"
  patched=1
done

# Optionally patch direct cdk-ehash git dep (set CDK_EHASH_PATH to enable)
if [[ -n "${CDK_EHASH_PATH:-}" ]]; then
  while IFS= read -r file; do
    if grep -qE 'vnprc/cdk-ehash' "$file"; then
      echo "✅ Patching cdk-ehash in: $file"
      cp "$file" "$file.bak"
      sed -i "s|cdk-ehash[[:space:]]*=.*vnprc/cdk-ehash.*|cdk-ehash = { path = \"$CDK_EHASH_PATH\" }|" "$file"
      patched=1
    fi
  done < <(find . -name Cargo.toml)
fi

if [[ $patched -eq 0 ]]; then
  echo "No CDK git entries found to patch."
fi
