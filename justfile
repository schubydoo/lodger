# Lodger task runner. Run `just --list` to see every recipe.
# Recipes marked "not ready" get their real commands in later tasks.

# Install the toolchains and system packages
setup:
    @echo "not ready yet: scripts/bootstrap.sh comes later" && exit 1

# Run the backend on the libvirt test driver plus the Vite dev server
dev:
    @echo "not ready yet: comes with the web app" && exit 1

# Run the backend only, with no Node
dev-api:
    cargo run -p lodger

# Format, lint, and test
check:
    cargo fmt --all --check
    cargo clippy --workspace --all-targets -- -D warnings
    cargo test --workspace

# Build the release binary
build:
    cargo build --release -p lodger
