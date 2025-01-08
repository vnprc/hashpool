set shell := ["bash", "-cu"]

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

