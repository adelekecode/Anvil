#!/bin/sh

set -eu

REPOSITORY_ROOT="${SRCROOT}/../../.."
TARGET="aarch64-apple-ios"

# Xcode exports compiler variables containing its formatted build-setting
# output. Cargo build-script executables are macOS host binaries; inheriting
# those iOS compiler/linker values makes crates with build scripts try to invoke
# a path containing leading spaces (rendered as `%20...cc`). Let rustc select
# the host and Apple target linkers through xcrun instead.
unset CC CXX LD AR CFLAGS CXXFLAGS CPPFLAGS LDFLAGS

CARGO_PATH="$(command -v cargo || true)"
if [ -z "${CARGO_PATH}" ]; then
  CARGO_PATH="${HOME}/.cargo/bin/cargo"
fi

if [ "${CONFIGURATION}" = "Debug" ]; then
  "${CARGO_PATH}" build \
    --manifest-path "${REPOSITORY_ROOT}/Cargo.toml" \
    --package anvil-ffi \
    --target "${TARGET}"
else
  "${CARGO_PATH}" build \
    --manifest-path "${REPOSITORY_ROOT}/Cargo.toml" \
    --package anvil-ffi \
    --target "${TARGET}" \
    --release
fi
