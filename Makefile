.PHONY: all linux-musl windows windows7 clean

# Default target
all: linux-musl windows windows7

# Build for Linux x86-64 (musl) using cargo-zigbuild (no Docker required)
linux-musl:
	@echo "Building for Linux x86-64 (musl)..."
	RUSTUP_DIST_SERVER="https://rsproxy.cn" RUSTUP_UPDATE_ROOT="https://rsproxy.cn/rustup" rustup target add x86_64-unknown-linux-musl
	cargo zigbuild --target x86_64-unknown-linux-musl --release

# Build for Windows x86-64 (MSVC) using cargo-xwin
windows:
	@echo "Building for Windows x86-64 (MSVC)..."
	RUSTUP_DIST_SERVER="https://rsproxy.cn" RUSTUP_UPDATE_ROOT="https://rsproxy.cn/rustup" rustup component add llvm-tools-preview
	RUSTUP_DIST_SERVER="https://rsproxy.cn" RUSTUP_UPDATE_ROOT="https://rsproxy.cn/rustup" rustup target add x86_64-pc-windows-msvc
	cargo xwin build --target x86_64-pc-windows-msvc --release

# Build for Windows 7 x86-64 (MSVC) using cargo-xwin
# Note: Rust 1.78+ dropped support for Windows 7.
# To build for Windows 7, we use Rust 1.77.2.
windows7:
	@echo "Building for Windows 7 x86-64 (MSVC) using Rust 1.77.2..."
	RUSTUP_DIST_SERVER="https://rsproxy.cn" RUSTUP_UPDATE_ROOT="https://rsproxy.cn/rustup" rustup component add llvm-tools-preview --toolchain nightly
	cargo +nightly xwin build --target x86_64-win7-windows-msvc --release -Z build-std

# Clean build artifacts
clean:
	cargo clean
