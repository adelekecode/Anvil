use anvil_core::audio::PcmFrame;
use anvil_core::platform::{
    AudioAdapter, Capabilities, DiscoveryAdapter, KeyStoreAdapter, TransportAdapter,
};
use anvil_core::transport::{Endpoint, PathKind};
use anvil_core::{AudioConfig, PathId, PlatformAdapter, PlatformError, Result};
use jni::objects::{GlobalRef, JByteArray, JObject, JShortArray, JString, JValue};
use jni::{AttachGuard, JNIEnv, JavaVM};

pub(crate) struct AndroidPlatform {
    vm: JavaVM,
    object: GlobalRef,
}

impl AndroidPlatform {
    pub(crate) fn new(vm: JavaVM, object: GlobalRef) -> Self {
        Self { vm, object }
    }

    fn env(&self) -> Result<AttachGuard<'_>> {
        let mut guard = self
            .vm
            .attach_current_thread()
            .map_err(|error| PlatformError::Adapter(error.to_string()))?;

        // Safety net. If an earlier call left an exception pending — a path this
        // code tries hard not to take, but cannot prove it never does — clearing
        // it here turns a process abort into a logged error.
        if let Some(detail) = drain_exception(&mut guard) {
            tracing::warn!(%detail, "cleared a Java exception left pending by an earlier call");
        }
        Ok(guard)
    }

    fn call_void(&self, name: &str, signature: &str, args: &[JValue<'_, '_>]) -> Result<()> {
        let mut env = self.env()?;
        let result = env.call_method(self.object.as_obj(), name, signature, args);
        finish(&mut env, name, result).map(|_| ())
    }

    fn bytes(&self, bytes: &[u8]) -> Result<(AttachGuard<'_>, JByteArray<'_>)> {
        let env = self.env()?;
        let array = env
            .byte_array_from_slice(bytes)
            .map_err(|error| PlatformError::Adapter(error.to_string()))?;
        Ok((env, array))
    }

    fn kind(kind: PathKind) -> &'static str {
        match kind {
            PathKind::Lan => "lan",
            PathKind::WifiAware => "wifi-aware",
        }
    }
}

impl core::fmt::Debug for AndroidPlatform {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("AndroidPlatform").finish_non_exhaustive()
    }
}

impl DiscoveryAdapter for AndroidPlatform {
    fn start_lan_discovery(&self) -> Result<()> {
        self.call_void("startLanDiscovery", "()V", &[])
    }
    fn stop_lan_discovery(&self) -> Result<()> {
        self.call_void("stopLanDiscovery", "()V", &[])
    }
    fn start_aware_discovery(&self) -> Result<()> {
        self.call_void("startAwareDiscovery", "()V", &[])
    }
    fn stop_aware_discovery(&self) -> Result<()> {
        self.call_void("stopAwareDiscovery", "()V", &[])
    }
    fn advertise(&self, payload: &[u8]) -> Result<()> {
        let (mut env, array) = self.bytes(payload)?;
        let object = JObject::from(array);
        let result =
            env.call_method(self.object.as_obj(), "advertise", "([B)V", &[JValue::Object(&object)]);
        finish(&mut env, "advertise", result).map(|_| ())
    }
    fn stop_advertising(&self) -> Result<()> {
        self.call_void("stopAdvertising", "()V", &[])
    }
}

impl TransportAdapter for AndroidPlatform {
    fn connect(&self, path: PathId, endpoint: &Endpoint) -> Result<()> {
        let mut env = self.env()?;
        let kind = env.new_string(Self::kind(endpoint.kind)).map_err(adapter_error)?;
        let address = env.new_string(&endpoint.address).map_err(adapter_error)?;
        let kind_object = JObject::from(kind);
        let address_object = JObject::from(address);
        let result = env.call_method(
            self.object.as_obj(),
            "connect",
            "(JLjava/lang/String;Ljava/lang/String;)V",
            &[
                JValue::Long(path.0 as i64),
                JValue::Object(&kind_object),
                JValue::Object(&address_object),
            ],
        );
        finish(&mut env, "connect", result).map(|_| ())
    }

    fn close(&self, path: PathId) -> Result<()> {
        self.call_void("close", "(J)V", &[JValue::Long(path.0 as i64)])
    }

    fn send_datagram(&self, path: PathId, data: &[u8]) -> Result<()> {
        self.send_bytes("sendDatagram", path, data)
    }

    fn send_reliable(&self, path: PathId, data: &[u8]) -> Result<()> {
        self.send_bytes("sendReliable", path, data)
    }

    fn listen(&self, kind: PathKind) -> Result<Endpoint> {
        let mut env = self.env()?;
        let kind_string = env.new_string(Self::kind(kind)).map_err(adapter_error)?;
        let kind_object = JObject::from(kind_string);
        let result = env.call_method(
            self.object.as_obj(),
            "listen",
            "(Ljava/lang/String;)Ljava/lang/String;",
            &[JValue::Object(&kind_object)],
        );
        let value = finish(&mut env, "listen", result)?.l().map_err(adapter_error)?;
        if value.is_null() {
            return Err(PlatformError::Adapter("Android listen returned null".into()).into());
        }
        let value = JString::from(value);
        let address: String = env.get_string(&value).map_err(adapter_error)?.into();
        Ok(Endpoint::new(kind, address))
    }
}

impl AndroidPlatform {
    fn send_bytes(&self, method: &str, path: PathId, data: &[u8]) -> Result<()> {
        let (mut env, array) = self.bytes(data)?;
        let object = JObject::from(array);
        let result = env.call_method(
            self.object.as_obj(),
            method,
            "(J[B)V",
            &[JValue::Long(path.0 as i64), JValue::Object(&object)],
        );
        finish(&mut env, method, result).map(|_| ())
    }
}

impl AudioAdapter for AndroidPlatform {
    fn start_capture(&self, config: &AudioConfig) -> Result<()> {
        self.call_void(
            "startCapture",
            "(III)V",
            &[
                JValue::Int(config.sample_rate_hz as i32),
                JValue::Int(config.channels as i32),
                JValue::Int(config.frame_duration.as_millis() as i32),
            ],
        )
    }
    fn stop_capture(&self) -> Result<()> {
        self.call_void("stopCapture", "()V", &[])
    }
    fn start_playback(&self, config: &AudioConfig) -> Result<()> {
        self.call_void(
            "startPlayback",
            "(II)V",
            &[JValue::Int(config.sample_rate_hz as i32), JValue::Int(config.channels as i32)],
        )
    }
    fn stop_playback(&self) -> Result<()> {
        self.call_void("stopPlayback", "()V", &[])
    }
    fn play(&self, frame: &PcmFrame) -> Result<()> {
        let mut env = self.env()?;
        let array: JShortArray<'_> =
            env.new_short_array(frame.samples.len() as i32).map_err(adapter_error)?;
        env.set_short_array_region(&array, 0, &frame.samples).map_err(adapter_error)?;
        let object = JObject::from(array);
        let result =
            env.call_method(self.object.as_obj(), "play", "([S)V", &[JValue::Object(&object)]);
        finish(&mut env, "play", result).map(|_| ())
    }
}

impl KeyStoreAdapter for AndroidPlatform {
    fn load_identity(&self) -> Result<Option<Vec<u8>>> {
        let mut env = self.env()?;
        let result = env.call_method(self.object.as_obj(), "loadIdentity", "()[B", &[]);
        let value = finish(&mut env, "loadIdentity", result)?.l().map_err(adapter_error)?;
        if value.is_null() {
            return Ok(None);
        }
        let array = JByteArray::from(value);
        env.convert_byte_array(array).map(Some).map_err(adapter_error)
    }
    fn store_identity(&self, bytes: &[u8]) -> Result<()> {
        let (mut env, array) = self.bytes(bytes)?;
        let object = JObject::from(array);
        let result = env.call_method(
            self.object.as_obj(),
            "storeIdentity",
            "([B)V",
            &[JValue::Object(&object)],
        );
        finish(&mut env, "storeIdentity", result).map(|_| ())
    }
    fn clear_identity(&self) -> Result<()> {
        self.call_void("clearIdentity", "()V", &[])
    }
}

impl PlatformAdapter for AndroidPlatform {
    fn capabilities(&self) -> Capabilities {
        let Ok(mut env) = self.env() else {
            return Capabilities::default();
        };
        let result = env.call_method(self.object.as_obj(), "capabilitiesMask", "()I", &[]);
        let Ok(value) = finish(&mut env, "capabilitiesMask", result) else {
            return Capabilities::default();
        };
        let Ok(mask) = value.i() else {
            return Capabilities::default();
        };
        Capabilities {
            lan: mask & 1 != 0,
            wifi_aware: mask & 2 != 0,
            microphone: mask & 4 != 0,
            nearby_devices: mask & 8 != 0,
            secure_key_storage: mask & 16 != 0,
        }
    }

    fn request_permission(&self, capability: &'static str) -> Result<()> {
        let mut env = self.env()?;
        let capability = env.new_string(capability).map_err(adapter_error)?;
        let object = JObject::from(capability);
        let result = env.call_method(
            self.object.as_obj(),
            "requestPermission",
            "(Ljava/lang/String;)V",
            &[JValue::Object(&object)],
        );
        finish(&mut env, "requestPermission", result).map(|_| ())
    }
}

fn adapter_error(error: jni::errors::Error) -> anvil_core::Error {
    PlatformError::Adapter(error.to_string()).into()
}

/// Complete a JNI call, guaranteeing no Java exception is left pending.
///
/// **This is not optional bookkeeping — omitting it crashes the process.**
///
/// When a Java method invoked over JNI throws, the exception stays *pending on
/// the calling thread*. The `jni` crate surfaces `Err(JavaException)` but
/// deliberately does not clear it, because only the caller knows whether it
/// wants to inspect it. ART then aborts the entire process on the *next* JNI
/// call made from that thread:
///
/// ```text
/// JNI DETECTED ERROR IN APPLICATION: JNI CallVoidMethodA called with
/// pending exception java.lang.IllegalArgumentException
/// ```
///
/// The abort is a SIGABRT with no Dart stack trace and no Rust backtrace, and
/// it lands at whichever call site happened to come next — not the one that
/// threw. That displacement is what makes this class of bug so hard to read
/// from a crash report, and why every call site funnels through here.
fn finish<T>(env: &mut JNIEnv<'_>, method: &str, result: jni::errors::Result<T>) -> Result<T> {
    match result {
        // A call can return Ok and still leave an exception pending if one was
        // raised while marshalling arguments, so check on both paths.
        Ok(value) => match drain_exception(env) {
            None => Ok(value),
            Some(detail) => {
                Err(PlatformError::Adapter(format!("Android {method}: {detail}")).into())
            }
        },
        Err(error) => {
            let detail = drain_exception(env).unwrap_or_else(|| error.to_string());
            Err(PlatformError::Adapter(format!("Android {method}: {detail}")).into())
        }
    }
}

/// Clear a pending Java exception, returning a description of it.
///
/// `exception_describe` writes the Java stack trace to logcat, which is the
/// only place the original throw is visible — the Rust error carries just the
/// summary line.
fn drain_exception(env: &mut JNIEnv<'_>) -> Option<String> {
    if !env.exception_check().unwrap_or(false) {
        return None;
    }

    let throwable = env.exception_occurred().ok();
    let _ = env.exception_describe();
    // Must happen before any further JNI call, including the toString() below.
    let _ = env.exception_clear();

    let throwable = throwable?;
    let described = env.call_method(&throwable, "toString", "()Ljava/lang/String;", &[]);
    // toString() can itself throw; never leave one pending.
    let _ = env.exception_clear();

    let object = described.ok()?.l().ok()?;
    if object.is_null() {
        return None;
    }
    env.get_string(&JString::from(object)).ok().map(Into::into)
}
