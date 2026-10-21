#!/bin/sh
# Invoked by src/meson.build's `cargo-build` custom_target. Builds the
# Rust binary offline-friendly (Flatpak vendors dependencies via
# cargo-sources.json ahead of time) and copies the resulting binary to
# the path Meson expects as its custom_target output.
#
# Arguments: $1 = meson build root, $2 = meson source root,
#            $3 = desired output path, $4 = build profile ("release"|"debug")
set -eu

export MESON_BUILD_ROOT="$1"
export MESON_SOURCE_ROOT="$2"
export OUTPUT_PATH="$3"
export PROFILE="$4"

export CARGO_TARGET_DIR="${MESON_BUILD_ROOT}/cargo-target"
export CARGO_HOME="${CARGO_HOME:-${MESON_BUILD_ROOT}/cargo-home}"
export RUST_BACKTRACE=1

if [ "${PROFILE}" = "release" ]; then
  cargo build --manifest-path "${MESON_SOURCE_ROOT}/Cargo.toml" --release
  cp "${CARGO_TARGET_DIR}/release/nexus" "${OUTPUT_PATH}"
else
  cargo build --manifest-path "${MESON_SOURCE_ROOT}/Cargo.toml"
  cp "${CARGO_TARGET_DIR}/debug/nexus" "${OUTPUT_PATH}"
fi
