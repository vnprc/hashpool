#!/bin/bash
# Validation script for deployment and health check scripts

echo "Validating Hashpool deployment scripts..."
echo "========================================"

# Check 1: Package names in deploy script match actual Cargo.toml
echo "✓ Checking package names..."
SCRIPT_PACKAGES=$(grep -o "cargo build.*-p [^ ]*" scripts/deploy.sh | cut -d' ' -f4)
ACTUAL_PACKAGES=$(find . -name "Cargo.toml" -path "./*/Cargo.toml" -exec grep -l "bin.*=" {} \; | xargs grep "^name" | cut -d'"' -f2 | sort -u)

echo "Script packages: $SCRIPT_PACKAGES"
echo "Actual packages: $ACTUAL_PACKAGES"

# Check 2: Config file names in deploy script exist
echo "✓ Checking config file references..."
CONFIG_FILES="mint.config.toml pool.config.toml tproxy.config.toml jds.config.toml jdc.config.toml"
for file in $CONFIG_FILES; do
    if [ -f "config/$file" ]; then
        echo "  ✓ $file exists"
    else
        echo "  ✗ $file MISSING"
    fi
done

# Check 3: Port numbers in health check match config files  
echo "✓ Checking port numbers..."
HEALTH_PORTS=$(grep -o '"[0-9]*:' scripts/health-check.sh | cut -d'"' -f2 | cut -d':' -f1)
echo "Health check ports: $HEALTH_PORTS"

# Check 4: Binary names match between build commands and install commands
echo "✓ Checking binary consistency..."
BUILD_BINS=$(grep "cargo build.*--bin" scripts/deploy.sh | sed 's/.*--bin //' | sort)
INSTALL_BINS=$(grep "target/release/" scripts/deploy.sh | sed 's/.*target\/release\///' | sed 's/ .*//' | sort)

echo "Build binaries: $BUILD_BINS"
echo "Install binaries: $INSTALL_BINS"

if [ "$BUILD_BINS" = "$INSTALL_BINS" ]; then
    echo "  ✓ Binary names consistent"
else
    echo "  ✗ Binary names INCONSISTENT"
fi

# Check 5: Service dependencies make sense
echo "✓ Checking systemd service dependencies..."
grep -A5 -B1 "After=\|Wants=" scripts/deploy.sh

echo
echo "Validation complete. Review any issues above before deployment."