#!/bin/sh

set -eu

REPOSITORY_ROOT="${SRCROOT}/../../.."
TARGET="aarch64-apple-ios"

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
