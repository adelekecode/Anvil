#!/bin/sh

set -eu

REPOSITORY_ROOT="$1"
JNI_LIBS_DIRECTORY="$2"

CARGO_PATH="$(command -v cargo || true)"
if [ -z "${CARGO_PATH}" ]; then
  CARGO_PATH="${HOME}/.cargo/bin/cargo"
fi

"${CARGO_PATH}" ndk \
  --target arm64-v8a \
  --output-dir "${JNI_LIBS_DIRECTORY}" \
  build \
  --manifest-path "${REPOSITORY_ROOT}/Cargo.toml" \
  --package anvil-ffi \
  --release
