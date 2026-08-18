#!/bin/sh

set -eu

REPOSITORY_ROOT="${SRCROOT}/../../.."
TARGET="aarch64-apple-ios"

# Xcode exports compiler variables and an iPhoneOS SDKROOT. Cargo build-script
# executables are macOS host binaries; inheriting those target settings makes
# rustc try to link the host tools against the device SDK. Flutter then renders
# Rust's indented `cc` diagnostic as a bogus `%20...cc` source path. Clear the
# complete compiler/SDK environment and let rustc select the macOS host SDK and
# Apple target SDK independently through xcrun.
unset CC CXX LD AR CFLAGS CXXFLAGS CPPFLAGS LDFLAGS \
  SDKROOT IPHONEOS_DEPLOYMENT_TARGET MACOSX_DEPLOYMENT_TARGET

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
