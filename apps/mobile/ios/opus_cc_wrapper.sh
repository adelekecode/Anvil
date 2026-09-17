#!/bin/sh

# audiopus-sys invokes Autoconf from a temporary directory whose basename is
# `opus`. Cargo also runs other C build scripts for the host while assembling
# the iOS target. Only the former must use the iPhoneOS SDK; sending iOS flags
# to host build scripts produces misleading failures such as "library System
# not found".

case "$(pwd)" in
  */opus|*/opus/*)
    IOS_SDK="$(xcrun --sdk iphoneos --show-sdk-path)"
    IOS_CLANG="$(xcrun --sdk iphoneos --find clang)"
    exec "${IOS_CLANG}" \
      -arch arm64 \
      -isysroot "${IOS_SDK}" \
      -miphoneos-version-min=10.0 \
      -fno-stack-check \
      -fno-stack-clash-protection \
      "$@"
    ;;
  *)
    exec /usr/bin/clang "$@"
    ;;
esac
