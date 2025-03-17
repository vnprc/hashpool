# format nix files
formatnix:
	alejandra .

# start development processes
up:
	devenv up

# point cdk cargo dependencies to local repo
local-cdk:
    @if [ -z "$CDK_PATH" ]; then \
        echo "Error: CDK_PATH is not set. Please set it before running this command."; \
        echo "Example: export CDK_PATH=/absolute/path/to/cdk/crates/cdk"; \
        exit 1; \
    fi; \
    echo "Using CDK_PATH: $CDK_PATH"; \
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

# update bitcoind.nix with latest rev & hash
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
