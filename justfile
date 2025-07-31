# list available just commands
default:
	@just --list
	@echo "Run 'just <recipe>' to execute a command."

# format nix files
formatnix:
	alejandra .

# enter devenv shell
shell:
	devenv shell

# start development processes; pass 'backtrace' to enable RUST_BACKTRACE=1
up mode="":
    @if [ "{{mode}}" = "backtrace" ]; then \
        RUST_BACKTRACE=1 devenv up; \
    else \
        devenv up; \
    fi

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

# generate blocks in regtest
generate-blocks COUNT="1":
    @echo "Generating {{COUNT}} blocks in regtest..."
    @bitcoin-cli -datadir=.devenv/state/bitcoind -conf=$(pwd)/config/bitcoin.conf -rpcuser=username -rpcpassword=password -regtest -rpcwallet=regtest -generate {{COUNT}}

# Open cdk sqlite terminal client (wallet or mint)
db TYPE="":
    @if [ "{{TYPE}}" = "wallet" ]; then \
        sqlite3 -cmd ".mode line" .devenv/state/translator/wallet.sqlite; \
    elif [ "{{TYPE}}" = "mint" ]; then \
        sqlite3 -cmd ".mode line" .devenv/state/mint/mint.sqlite; \
    else \
        echo "Error: TYPE must be 'wallet' or 'mint'"; \
        exit 1; \
    fi

# delete persistent storage; options: cashu, regtest, testnet4
clean TYPE="":
    @if [ "{{TYPE}}" = "cashu" ]; then \
        echo "deleting all sqlite and redis data..."; \
        rm -f .devenv/state/translator/wallet.sqlite \
              .devenv/state/translator/wallet.sqlite-shm \
              .devenv/state/translator/wallet.sqlite-wal \
              .devenv/state/mint/mint.sqlite \
              .devenv/state/mint/mint.sqlite-shm \
              .devenv/state/mint/mint.sqlite-wal \
              .devenv/state/redis/dump.rdb; \
        echo "all sqlite and redis data deleted"; \
    elif [ "{{TYPE}}" = "regtest" ]; then \
        echo "deleting regtest data..."; \
        rm -rf .devenv/state/bitcoind/regtest .devenv/state/cln/regtest; \
        rm -f .devenv/state/cln/lightningd-regtest.pid; \
        echo "regtest data deleted"; \
    elif [ "{{TYPE}}" = "testnet4" ]; then \
        echo "deleting testnet4 data..."; \
        rm -rf .devenv/state/bitcoind/testnet4 .devenv/state/cln/testnet4; \
        rm -f .devenv/state/cln/lightningd-testnet4.pid; \
        echo "testnet4 data deleted"; \
    else \
        echo "Error: TYPE must be 'cashu', 'regtest', or 'testnet4'"; \
        exit 1; \
    fi
