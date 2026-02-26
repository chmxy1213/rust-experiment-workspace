#!/bin/bash

set -e

echo "Building for Linux x86-64 (musl) using cargo-zigbuild..."
RUSTUP_DIST_SERVER="https://rsproxy.cn" RUSTUP_UPDATE_ROOT="https://rsproxy.cn/rustup" rustup target add x86_64-unknown-linux-musl
cargo zigbuild --target x86_64-unknown-linux-musl --release

echo "Building for Windows x86-64 (MSVC) using cargo-xwin..."
rustup component add llvm-tools-preview
cargo xwin build --target x86_64-pc-windows-msvc --release

echo "Building for Windows 7 x86-64 (MSVC) using cargo-xwin..."
# Rust 1.78+ dropped support for Windows 7. 
# We use Rust 1.77.2 with cargo-xwin to support Windows 7.
rustup toolchain install 1.77.2
rustup component add llvm-tools-preview --toolchain 1.77.2
cargo +1.77.2 xwin build --target x86_64-pc-windows-msvc --release

echo "Build complete! Binaries are in target/<target>/release/"
