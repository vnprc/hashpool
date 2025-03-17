# format nix files
formatnix:
	alejandra .

# start development processes
up:
	devenv up

# point cdk cargo dependencies to local repo
local-cdk:
    @if [ ! -f devenv.local.nix ]; then \
        echo "Error: devenv.local.nix not found. Please create it by copying devenv.local.nix.default and updating CDK_PATH."; \
        echo "Example command: cp devenv.local.nix.default devenv.local.nix"; \
        exit 1; \
    fi; \
    CDK_PATH=$(nix eval --impure --raw --expr 'let cfg = import ./devenv.local.nix; in cfg.env.CDK_PATH') && \
    echo "CDK_PATH: $CDK_PATH" && \
    if [ -z "$CDK_PATH" ]; then \
        echo "Error: CDK_PATH is not set in devenv.local.nix. Please set it to your local CDK path."; \
        echo "Example: { env.CDK_PATH = \"/absolute/path/to/your/cdk\"; }"; \
        exit 1; \
    fi; \
    find . -name "Cargo.toml" -exec grep -l 'cdk = { git = "https://github.com/vnprc/cdk", rev = "[^"]*" }' {} + | while IFS= read -r file; do \
        echo "Updating $file"; \
        sed -i.bak "s|cdk = { git = \"https://github.com/vnprc/cdk\", rev = \"[^\"]*\" }|cdk = { path = \"$CDK_PATH\" }|" "$file"; \
    done

# restore cargo dependencies from .bak files
restore-deps:
    find . -name "Cargo.toml.bak" | while IFS= read -r bakfile; do \
        origfile="${bakfile%.bak}"; \
        echo "Restoring $origfile from $bakfile"; \
        mv "$bakfile" "$origfile"; \
    done

update-bitcoind:
    @echo "Fetching latest commit hash for sv2 branch..."
    @LATEST_COMMIT=$(curl -s "https://api.github.com/repos/Sjors/bitcoin/commits/sv2" | jq -r ".sha") && \
    echo "Latest commit: $LATEST_COMMIT" && \
    echo "Fetching new hash for Nix..." && \
    HASH_RAW=$(nix-prefetch-url --unpack "https://github.com/Sjors/bitcoin/archive/$LATEST_COMMIT.tar.gz") && \
    HASH=$(nix hash to-sri --type sha256 "$HASH_RAW") && \
    echo "Computed Nix SRI hash: $HASH" && \
    echo "Updating bitcoind.nix..." && \
    sed -i "s|rev = \".*\";|rev = \"$LATEST_COMMIT\";|" bitcoind.nix && \
    sed -i "s|hash = \".*\";|hash = \"$HASH\";|" bitcoind.nix && \
    echo "Done! bitcoind updated to commit $LATEST_COMMIT\nYou are now ready to test and commit"
