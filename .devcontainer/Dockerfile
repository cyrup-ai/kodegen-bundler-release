# Multi-stage Dockerfile for incremental testing
# Test stages independently to catch failures early:
#   docker build --target=base-packages     (30 seconds)
#   docker build --target=rust-toolchain    (5 minutes)
#   docker build .                          (full build)

# ============================================================================
# Stage 1: base-packages
# Quick test: package installation
# Test with: docker build --target=base-packages -t kodegen-test-base .
# ============================================================================
FROM rust:1.83-bookworm AS base-packages

# Install system dependencies for building all platforms
RUN apt-get update && apt-get install -y \
    # Build essentials
    build-essential \
    pkg-config \
    libssl-dev \
    git \
    curl \
    wget \
    unzip \
    # NSIS native Linux compiler for Windows installers
    nsis \
    # Linux package tools
    rpm \
    fakeroot \
    dpkg-dev \
    # AppImage dependencies
    file \
    desktop-file-utils \
    # General utilities
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Verify base packages installed correctly
RUN makensis -VERSION

# ============================================================================
# Stage 2: user-setup
# Create build user with proper permissions
# ============================================================================
FROM base-packages AS user-setup

# Create build user matching typical host UID/GID
ARG USER_UID=1000
ARG USER_GID=1000

RUN groupadd --gid $USER_GID builder \
    && useradd --uid $USER_UID --gid $USER_GID -m builder \
    && apt-get update \
    && apt-get install -y sudo \
    && echo builder ALL=\(root\) NOPASSWD:ALL > /etc/sudoers.d/builder \
    && chmod 0440 /etc/sudoers.d/builder \
    && rm -rf /var/lib/apt/lists/*

# Switch to build user
USER builder
ENV HOME=/home/builder
ENV PATH=$HOME/.cargo/bin:$PATH

# ============================================================================
# Stage 3: rust-toolchain
# Install Rust toolchain and components
# Test with: docker build --target=rust-toolchain -t kodegen-test-rust .
# ============================================================================
FROM user-setup AS rust-toolchain

# Install Rust nightly (matching rust-toolchain.toml)
RUN rustup default nightly-2024-10-20 \
    && rustup component add rustfmt clippy

# Verify Rust installation
RUN rustc --version && cargo --version

# ============================================================================
# Stage 4: full-builder (default)
# Complete build environment ready for compilation
# ============================================================================
FROM rust-toolchain AS full-builder

# Create cache directory for NSIS downloads
RUN mkdir -p $HOME/.cache/cyrup

WORKDIR /workspace

# Final verification of all tools
RUN rustc --version && \
    cargo --version && \
    makensis -VERSION

# Build instructions:
# - Stage testing: docker build --target=<stage> -t kodegen-test-<stage> .
# - Full image: docker build -t kodegen-release-builder .
# - With cache: docker build --pull -t kodegen-release-builder .
