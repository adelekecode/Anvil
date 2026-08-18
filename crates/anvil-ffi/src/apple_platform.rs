use std::ffi::{c_char, c_int, c_void, CString};

use anvil_core::audio::PcmFrame;
use anvil_core::platform::{
    AudioAdapter, Capabilities, DiscoveryAdapter, KeyStoreAdapter, TransportAdapter,
};
use anvil_core::transport::{Endpoint, PathKind};
use anvil_core::{AudioConfig, PathId, PlatformAdapter, PlatformError, Result};

pub(crate) type CapabilitiesCallback = unsafe extern "C" fn(*mut c_void) -> u32;
pub(crate) type InvokeCallback =
    unsafe extern "C" fn(*mut c_void, *const c_char, u64, *const u8, usize, *const c_char) -> c_int;
pub(crate) type LoadIdentityCallback = unsafe extern "C" fn(*mut c_void, *mut u8, usize) -> isize;
pub(crate) type ReleaseCallback = unsafe extern "C" fn(*mut c_void);

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct AnvilPlatformCallbacks {
    pub context: *mut c_void,
    pub capabilities: Option<CapabilitiesCallback>,
    pub invoke: Option<InvokeCallback>,
    pub load_identity: Option<LoadIdentityCallback>,
    pub release: Option<ReleaseCallback>,
}

pub(crate) struct ApplePlatform {
    callbacks: AnvilPlatformCallbacks,
}

// The retained Swift object is deliberately invoked from the engine thread.
// Its adapters marshal OS callbacks onto their own queues and only push events
// back into the engine inbox, so this pointer is never otherwise dereferenced.
unsafe impl Send for ApplePlatform {}
unsafe impl Sync for ApplePlatform {}

impl ApplePlatform {
    pub(crate) fn new(callbacks: AnvilPlatformCallbacks) -> Self {
        Self { callbacks }
    }

    fn invoke(&self, operation: &str, arg: u64, data: &[u8], text: Option<&str>) -> Result<()> {
        let callback = self.callbacks.invoke.ok_or(PlatformError::NoAdapter)?;
        let operation = CString::new(operation)
            .map_err(|_| PlatformError::Adapter("invalid platform operation".into()))?;
        let text = text
            .map(CString::new)
            .transpose()
            .map_err(|_| PlatformError::Adapter("platform argument contained NUL".into()))?;
        let result = unsafe {
            callback(
                self.callbacks.context,
                operation.as_ptr(),
                arg,
                data.as_ptr(),
                data.len(),
                text.as_ref().map_or(std::ptr::null(), |value| value.as_ptr()),
            )
        };
        if result == 0 {
            Ok(())
        } else {
            Err(PlatformError::Adapter(format!("Apple {operation:?} returned {result}")).into())
        }
    }

    fn kind(kind: PathKind) -> &'static str {
        match kind {
            PathKind::Lan => "lan",
            PathKind::WifiAware => "wifi-aware",
        }
    }
}

impl Drop for ApplePlatform {
    fn drop(&mut self) {
        if let Some(release) = self.callbacks.release {
            unsafe { release(self.callbacks.context) };
        }
    }
}

impl core::fmt::Debug for ApplePlatform {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ApplePlatform").finish_non_exhaustive()
    }
}

impl DiscoveryAdapter for ApplePlatform {
    fn start_lan_discovery(&self) -> Result<()> {
        self.invoke("startLanDiscovery", 0, &[], None)
    }
    fn stop_lan_discovery(&self) -> Result<()> {
        self.invoke("stopLanDiscovery", 0, &[], None)
    }
    fn start_aware_discovery(&self) -> Result<()> {
        self.invoke("startAwareDiscovery", 0, &[], None)
    }
    fn stop_aware_discovery(&self) -> Result<()> {
        self.invoke("stopAwareDiscovery", 0, &[], None)
    }
    fn advertise(&self, payload: &[u8]) -> Result<()> {
        self.invoke("advertise", 0, payload, None)
    }
    fn stop_advertising(&self) -> Result<()> {
        self.invoke("stopAdvertising", 0, &[], None)
    }
}

impl TransportAdapter for ApplePlatform {
    fn connect(&self, path: PathId, endpoint: &Endpoint) -> Result<()> {
        self.invoke(
            match endpoint.kind {
                PathKind::Lan => "connectLan",
                PathKind::WifiAware => "connectAware",
            },
            path.0,
            &[],
            Some(&endpoint.address),
        )
    }
    fn close(&self, path: PathId) -> Result<()> {
        self.invoke("close", path.0, &[], None)
    }
    fn send_datagram(&self, path: PathId, data: &[u8]) -> Result<()> {
        self.invoke("sendDatagram", path.0, data, None)
    }
    fn send_reliable(&self, path: PathId, data: &[u8]) -> Result<()> {
        self.invoke("sendReliable", path.0, data, None)
    }
    fn listen(&self, kind: PathKind) -> Result<Endpoint> {
        self.invoke("listen", 0, &[], Some(Self::kind(kind)))?;
        Ok(Endpoint::new(kind, "native-listener"))
    }
}

impl AudioAdapter for ApplePlatform {
    fn start_capture(&self, config: &AudioConfig) -> Result<()> {
        self.invoke(
            "startCapture",
            config.sample_rate_hz as u64,
            &[],
            Some(&format!("{},{}", config.channels, config.frame_duration.as_millis())),
        )
    }
    fn stop_capture(&self) -> Result<()> {
        self.invoke("stopCapture", 0, &[], None)
    }
    fn start_playback(&self, config: &AudioConfig) -> Result<()> {
        self.invoke(
            "startPlayback",
            config.sample_rate_hz as u64,
            &[],
            Some(&config.channels.to_string()),
        )
    }
    fn stop_playback(&self) -> Result<()> {
        self.invoke("stopPlayback", 0, &[], None)
    }
    fn play(&self, frame: &PcmFrame) -> Result<()> {
        let bytes =
            frame.samples.iter().flat_map(|sample| sample.to_le_bytes()).collect::<Vec<_>>();
        self.invoke("play", frame.timestamp.0.into(), &bytes, None)
    }
}

impl KeyStoreAdapter for ApplePlatform {
    fn load_identity(&self) -> Result<Option<Vec<u8>>> {
        let callback = self.callbacks.load_identity.ok_or(PlatformError::NoAdapter)?;
        let length = unsafe { callback(self.callbacks.context, std::ptr::null_mut(), 0) };
        if length == 0 {
            return Ok(None);
        }
        if length < 0 {
            return Err(PlatformError::Adapter("Apple keychain load failed".into()).into());
        }
        let mut bytes = vec![0u8; length as usize];
        let written = unsafe { callback(self.callbacks.context, bytes.as_mut_ptr(), bytes.len()) };
        if written != length {
            return Err(PlatformError::Adapter(
                "Apple keychain returned an unstable length".into(),
            )
            .into());
        }
        Ok(Some(bytes))
    }
    fn store_identity(&self, bytes: &[u8]) -> Result<()> {
        self.invoke("storeIdentity", 0, bytes, None)
    }
    fn clear_identity(&self) -> Result<()> {
        self.invoke("clearIdentity", 0, &[], None)
    }
}

impl PlatformAdapter for ApplePlatform {
    fn capabilities(&self) -> Capabilities {
        let mask = self
            .callbacks
            .capabilities
            .map(|callback| unsafe { callback(self.callbacks.context) })
            .unwrap_or(0);
        Capabilities {
            lan: mask & 1 != 0,
            wifi_aware: mask & 2 != 0,
            microphone: mask & 4 != 0,
            nearby_devices: mask & 8 != 0,
            secure_key_storage: mask & 16 != 0,
        }
    }

    fn request_permission(&self, capability: &'static str) -> Result<()> {
        self.invoke("requestPermission", 0, &[], Some(capability))
    }
}
