# Default recipe - build and sign
default: build

# Build debug binary and sign it
build:
    cargo build
    codesign -s - target/debug/qwen-pr

# Build release binary and sign it
release:
    cargo build --release
    codesign -s - target/release/qwen-pr

# Install to ~/.cargo/bin and sign it
install:
    cargo install --path .
    codesign -s - ~/.cargo/bin/qwen-pr

# Run clippy lints
lint:
    cargo clippy

# Clean build artifacts
clean:
    cargo clean
