use std::sync::{Arc, RwLock};

use anvil_core::audio::PcmFrame;
use anvil_core::platform::{
    AudioAdapter, Capabilities, DiscoveryAdapter, KeyStoreAdapter, TransportAdapter,
};
use anvil_core::transport::{Endpoint, PathKind};
use anvil_core::{AudioConfig, PathId, PlatformAdapter, PlatformError, Result};

/// Stable adapter installed in the engine while the native host comes and goes.
///
/// Flutter creates the Rust session before its method channel can attach the
/// Android/iOS object. Keeping this proxy stable means the engine never needs
/// to be rebuilt and a detach cannot leave it holding a dead JNI/Swift object.
#[derive(Default)]
pub(crate) struct PlatformBridge {
    host: RwLock<Option<Arc<dyn PlatformAdapter>>>,
}

impl core::fmt::Debug for PlatformBridge {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("PlatformBridge")
            .field("attached", &self.host.read().map(|h| h.is_some()).unwrap_or(false))
            .finish()
    }
}

impl PlatformBridge {
    pub(crate) fn attach(&self, host: Arc<dyn PlatformAdapter>) {
        if let Ok(mut current) = self.host.write() {
            *current = Some(host);
        }
    }

    pub(crate) fn detach(&self) {
        if let Ok(mut current) = self.host.write() {
            *current = None;
        }
    }

    fn host(&self) -> Result<Arc<dyn PlatformAdapter>> {
        self.host
            .read()
            .ok()
            .and_then(|host| host.clone())
            .ok_or_else(|| PlatformError::NoAdapter.into())
    }
}

impl DiscoveryAdapter for PlatformBridge {
    fn start_lan_discovery(&self) -> Result<()> {
        self.host()?.start_lan_discovery()
    }
    fn stop_lan_discovery(&self) -> Result<()> {
        self.host().map_or(Ok(()), |host| host.stop_lan_discovery())
    }
    fn start_aware_discovery(&self) -> Result<()> {
        self.host()?.start_aware_discovery()
    }
    fn stop_aware_discovery(&self) -> Result<()> {
        self.host().map_or(Ok(()), |host| host.stop_aware_discovery())
    }
    fn advertise(&self, payload: &[u8]) -> Result<()> {
        self.host()?.advertise(payload)
    }
    fn stop_advertising(&self) -> Result<()> {
        self.host().map_or(Ok(()), |host| host.stop_advertising())
    }
}

impl TransportAdapter for PlatformBridge {
    fn connect(&self, path: PathId, endpoint: &Endpoint) -> Result<()> {
        self.host()?.connect(path, endpoint)
    }
    fn close(&self, path: PathId) -> Result<()> {
        self.host().map_or(Ok(()), |host| host.close(path))
    }
    fn send_datagram(&self, path: PathId, data: &[u8]) -> Result<()> {
        self.host()?.send_datagram(path, data)
    }
    fn send_reliable(&self, path: PathId, data: &[u8]) -> Result<()> {
        self.host()?.send_reliable(path, data)
    }
    fn listen(&self, kind: PathKind) -> Result<Endpoint> {
        self.host()?.listen(kind)
    }
}

impl AudioAdapter for PlatformBridge {
    fn start_capture(&self, config: &AudioConfig) -> Result<()> {
        self.host()?.start_capture(config)
    }
    fn stop_capture(&self) -> Result<()> {
        self.host().map_or(Ok(()), |host| host.stop_capture())
    }
    fn start_playback(&self, config: &AudioConfig) -> Result<()> {
        self.host()?.start_playback(config)
    }
    fn stop_playback(&self) -> Result<()> {
        self.host().map_or(Ok(()), |host| host.stop_playback())
    }
    fn play(&self, frame: &PcmFrame) -> Result<()> {
        self.host()?.play(frame)
    }
}

impl KeyStoreAdapter for PlatformBridge {
    fn load_identity(&self) -> Result<Option<Vec<u8>>> {
        self.host()?.load_identity()
    }
    fn store_identity(&self, bytes: &[u8]) -> Result<()> {
        self.host()?.store_identity(bytes)
    }
    fn clear_identity(&self) -> Result<()> {
        self.host().map_or(Ok(()), |host| host.clear_identity())
    }
}

impl PlatformAdapter for PlatformBridge {
    fn capabilities(&self) -> Capabilities {
        self.host().map(|host| host.capabilities()).unwrap_or_default()
    }

    fn request_permission(&self, capability: &'static str) -> Result<()> {
        self.host()?.request_permission(capability)
    }
}
