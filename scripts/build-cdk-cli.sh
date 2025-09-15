#!/bin/bash
set -euo pipefail

# CDK configuration
CDK_REPO="https://github.com/vnprc/cdk.git"
CDK_COMMIT="0315c1f2"

echo "Building CDK CLI from $CDK_REPO@$CDK_COMMIT..."

# Get project root
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

# Create temporary build directory
CDK_BUILD_DIR=$(mktemp -d)
cd "$CDK_BUILD_DIR"

# Clone and build
git clone "$CDK_REPO" .
git checkout "$CDK_COMMIT"
cargo build --release --bin cdk-cli

# Copy to hashpool bin directory
mkdir -p "$PROJECT_ROOT/bin"
cp target/release/cdk-cli "$PROJECT_ROOT/bin/cdk-cli"

# Cleanup
rm -rf "$CDK_BUILD_DIR"

echo "✅ CDK CLI built successfully"