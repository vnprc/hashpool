# format nix files
formatnix:
	alejandra .

# start development processes
up:
	devenv up

# point cdk cargo dependencies to local repo
local-cdk:
    ./scripts/patch-cdk-path.sh

# restore cargo dependencies from .bak files
restore-deps:
    find . -name "Cargo.toml.bak" | while IFS= read -r bakfile; do \
        origfile="${bakfile%.bak}"; \
        echo "✅ Restoring $origfile from $bakfile"; \
        mv "$bakfile" "$origfile"; \
    done

# update cdk commit hash in all Cargo.toml files
update-cdk OLD_REV NEW_REV:
    @echo "Updating CDK revision from {{OLD_REV}} to {{NEW_REV}}..."
    @find . -name "Cargo.toml" | xargs grep -l "cdk.*git.*vnprc.*rev.*{{OLD_REV}}" | while IFS= read -r file; do \
        echo "✅ Updating $file"; \
        sed -i 's|rev = "{{OLD_REV}}"|rev = "{{NEW_REV}}"|g' "$file"; \
    done
    @echo "Done! CDK updated from {{OLD_REV}} to {{NEW_REV}}"

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

# create regtest wallet if it doesn't exist
create-wallet:
    @echo "Creating regtest wallet..."
    @bitcoin-cli -datadir=.devenv/state/bitcoind -conf=$(pwd)/bitcoin.conf -rpcuser=username -rpcpassword=password -regtest createwallet "regtest" || \
    (echo "Wallet exists, loading..." && bitcoin-cli -datadir=.devenv/state/bitcoind -conf=$(pwd)/bitcoin.conf -rpcuser=username -rpcpassword=password -regtest loadwallet "regtest")

# generate blocks in regtest
generate-blocks COUNT="1" CREATE_WALLET="false":
    @echo "Generating {{COUNT}} blocks in regtest..."
    @if [ "{{CREATE_WALLET}}" = "true" ]; then just create-wallet; fi
    @bitcoin-cli -datadir=.devenv/state/bitcoind -conf=$(pwd)/bitcoin.conf -rpcuser=username -rpcpassword=password -regtest -rpcwallet=regtest -generate {{COUNT}}

# generate blocks to a specific address in regtest
generate-to-address ADDRESS COUNT="1" CREATE_WALLET="false":
    @echo "Generating {{COUNT}} blocks to address {{ADDRESS}} in regtest..."
    @if [ "{{CREATE_WALLET}}" = "true" ]; then just create-wallet; fi
    @bitcoin-cli -datadir=.devenv/state/bitcoind -conf=$(pwd)/bitcoin.conf -rpcuser=username -rpcpassword=password -regtest -rpcwallet=regtest generatetoaddress {{COUNT}} {{ADDRESS}}
