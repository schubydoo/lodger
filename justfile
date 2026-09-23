# Lodger task runner. Run `just --list` to see every recipe.
# Recipes marked "not ready" get their real commands in later tasks.

# Install the toolchains and system packages
setup:
    @echo "not ready yet: scripts/bootstrap.sh comes later" && exit 1

# Run the backend on the libvirt test driver plus the Vite dev server
dev:
    @echo "not ready yet: comes with the web app" && exit 1

# Run the backend only, with no Node. Serves web/build from disk if it exists,
# or a stub page if it does not.
dev-api:
    cargo run -p lodger -- serve

# Format, lint, and test
check:
    cargo fmt --all --check
    cargo clippy --workspace --all-targets -- -D warnings
    cargo test --workspace

# Needs cargo-about 0.9.2 with the `cli` feature. The web build joins this part
# with the web and copied notices.
# Generate the Rust part of /third-party-notices.txt
notices:
    cargo about generate about.hbs -o web/notices/rust.txt

# Build the web UI with its notices, then the release binary that embeds it
build: notices
    cd web && pnpm install --frozen-lockfile && pnpm run build
    cargo build --release -p lodger
