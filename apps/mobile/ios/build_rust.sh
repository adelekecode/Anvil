#!/bin/sh

set -eu

REPOSITORY_ROOT="${SRCROOT}/../../.."
TARGET="aarch64-apple-ios"

# Xcode exports compiler variables and an iPhoneOS SDKROOT. Cargo build-script
# executables are macOS host binaries; do not let target compiler settings leak
# into those host tools.
unset CC CXX LD AR CFLAGS CXXFLAGS CPPFLAGS LDFLAGS \
  SDKROOT IPHONEOS_DEPLOYMENT_TARGET MACOSX_DEPLOYMENT_TARGET

# Xcode 26's `cc` no longer discovers the macOS SDK when SDKROOT is empty.
# Build scripts run for the host, so give them the host SDK explicitly; rustc
# still supplies the iPhoneOS SDK when linking the aarch64-apple-ios target.
export SDKROOT="$(xcrun --sdk macosx --show-sdk-path)"
export MACOSX_DEPLOYMENT_TARGET="11.0"
# audiopus-sys runs Autoconf from a copied `opus` directory. The wrapper keeps
# that one build on iPhoneOS while allowing Cargo's host build scripts to use
# the macOS compiler and SDK.
export CC="${SRCROOT}/opus_cc_wrapper.sh"
export host_alias="${TARGET}"

CARGO_PATH="$(command -v cargo || true)"
if [ -z "${CARGO_PATH}" ]; then
  CARGO_PATH="${HOME}/.cargo/bin/cargo"
fi

if [ "${CONFIGURATION}" = "Debug" ]; then
  "${CARGO_PATH}" build \
    --manifest-path "${REPOSITORY_ROOT}/Cargo.toml" \
    --package anvil-ffi \
    --no-default-features \
    --features crypto,quic,opus \
    --target "${TARGET}"
else
  "${CARGO_PATH}" build \
    --manifest-path "${REPOSITORY_ROOT}/Cargo.toml" \
    --package anvil-ffi \
    --no-default-features \
    --features crypto,quic,opus \
    --target "${TARGET}" \
    --release
fi
