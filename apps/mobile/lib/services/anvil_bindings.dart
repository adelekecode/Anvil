// Raw dart:ffi bindings to libanvil_ffi.
//
// Nothing above this file should import `dart:ffi`. Pointers, allocation and
// NUL-terminated strings stop here; `AnvilApi` exposes Dart types and a stream.
//
// The symbols and their contract are defined in `crates/anvil-ffi/src/lib.rs`.
// If you change one, change both — there is no codegen keeping them in step,
// which is a deliberate trade (see that file's module docs) but does mean the
// two sides have to be edited together.

import 'dart:ffi';
import 'dart:io';

import 'package:ffi/ffi.dart';

/// Opaque session handle. Dart never looks inside.
final class AnvilSession extends Opaque {}

typedef InitNative = Pointer<AnvilSession> Function(Pointer<Utf8>);
typedef InitDart = Pointer<AnvilSession> Function(Pointer<Utf8>);

typedef CommandNative = Int32 Function(Pointer<AnvilSession>, Pointer<Utf8>);
typedef CommandDart = int Function(Pointer<AnvilSession>, Pointer<Utf8>);

typedef NextEventNative = Pointer<Utf8> Function(Pointer<AnvilSession>, Int32);
typedef NextEventDart = Pointer<Utf8> Function(Pointer<AnvilSession>, int);

typedef FreeStringNative = Void Function(Pointer<Utf8>);
typedef FreeStringDart = void Function(Pointer<Utf8>);

typedef ShutdownNative = Void Function(Pointer<AnvilSession>);
typedef ShutdownDart = void Function(Pointer<AnvilSession>);

typedef VersionNative = Int32 Function();
typedef VersionDart = int Function();

/// Result codes mirroring `ANVIL_*` in the Rust bridge.
abstract final class AnvilResult {
  static const int ok = 0;
  static const int invalidArgument = -1;
  static const int badCommand = -2;
  static const int engineStopped = -3;
  static const int panic = -4;

  static String describe(int code) => switch (code) {
        ok => 'ok',
        invalidArgument => 'invalid argument',
        badCommand => 'malformed or unknown command',
        engineStopped => 'engine has stopped',
        panic => 'panic caught at the FFI boundary',
        _ => 'unknown result $code',
      };
}

/// Resolved native symbols.
final class AnvilBindings {
  AnvilBindings._(DynamicLibrary library)
      : init = library.lookupFunction<InitNative, InitDart>('anvil_init'),
        command =
            library.lookupFunction<CommandNative, CommandDart>('anvil_command'),
        nextEvent = library
            .lookupFunction<NextEventNative, NextEventDart>('anvil_next_event'),
        freeString = library
            .lookupFunction<FreeStringNative, FreeStringDart>('anvil_free_string'),
        shutdown =
            library.lookupFunction<ShutdownNative, ShutdownDart>('anvil_shutdown'),
        protocolVersion = library
            .lookupFunction<VersionNative, VersionDart>('anvil_protocol_version');

  final InitDart init;
  final CommandDart command;
  final NextEventDart nextEvent;
  final FreeStringDart freeString;
  final ShutdownDart shutdown;
  final VersionDart protocolVersion;

  static AnvilBindings? _instance;

  /// Load the library for the current platform.
  ///
  /// On iOS the Rust code is a static library linked into the app binary, so
  /// symbols are looked up in the process itself. On Android it is a shared
  /// object loaded by name.
  static AnvilBindings load() {
    return _instance ??= AnvilBindings._(_open());
  }

  static DynamicLibrary _open() {
    if (Platform.isAndroid) {
      return DynamicLibrary.open('libanvil_ffi.so');
    }
    if (Platform.isIOS || Platform.isMacOS) {
      return DynamicLibrary.process();
    }
    if (Platform.isLinux) {
      return DynamicLibrary.open('libanvil_ffi.so');
    }
    if (Platform.isWindows) {
      return DynamicLibrary.open('anvil_ffi.dll');
    }
    throw UnsupportedError('Anvil has no native library for this platform');
  }
}

/// Run [body] with [text] as a native string, freeing it afterwards.
///
/// Every allocation on this side is paired here so there is exactly one place
/// to check for leaks.
T withNativeString<T>(String text, T Function(Pointer<Utf8>) body) {
  final pointer = text.toNativeUtf8();
  try {
    return body(pointer);
  } finally {
    calloc.free(pointer);
  }
}
