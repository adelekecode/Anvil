use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};

use anvil_core::transport::{quic as protocol_quic, Endpoint as AnvilEndpoint, PathKind};
use anvil_core::{EngineHandle, PathId, PlatformError, PlatformEvent, Result};
use bytes::Bytes;
use quinn::crypto::rustls::{QuicClientConfig, QuicServerConfig};
use quinn::{ClientConfig, Connection, Endpoint, ServerConfig};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, PrivatePkcs8KeyDer, ServerName, UnixTime};

const LAN_PORT: u16 = 47_820;
const MAX_CONTROL_RECORD: usize = 64 * 1024;

/// Shared QUIC data plane used by both mobile hosts.
///
/// Native code discovers IP endpoints and handles radio-specific permissions;
/// QUIC lives here so reliable streams, datagrams, congestion behaviour and
/// failure semantics are identical on Android and iOS.
pub(crate) struct QuicTransport {
    runtime: Arc<tokio::runtime::Runtime>,
    endpoint: Mutex<Option<Endpoint>>,
    outgoing: Arc<Mutex<HashMap<PathId, (SocketAddr, Connection)>>>,
    pending_inbound: Arc<Mutex<HashMap<SocketAddr, Vec<Connection>>>>,
    handle: Arc<Mutex<Option<EngineHandle>>>,
}

impl core::fmt::Debug for QuicTransport {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("QuicTransport")
            .field("listening", &self.endpoint.lock().map(|e| e.is_some()).unwrap_or(false))
            .field("connections", &self.outgoing.lock().map(|c| c.len()).unwrap_or(0))
            .finish()
    }
}

impl QuicTransport {
    pub(crate) fn new() -> Result<Self> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .thread_name("anvil-quic")
            .build()
            .map_err(adapter)?;
        Ok(Self {
            runtime: Arc::new(runtime),
            endpoint: Mutex::new(None),
            outgoing: Arc::new(Mutex::new(HashMap::new())),
            pending_inbound: Arc::new(Mutex::new(HashMap::new())),
            handle: Arc::new(Mutex::new(None)),
        })
    }

    pub(crate) fn set_handle(&self, handle: EngineHandle) {
        if let Ok(mut current) = self.handle.lock() {
            *current = Some(handle);
        }
    }

    pub(crate) fn listen(&self, kind: PathKind) -> Result<AnvilEndpoint> {
        if kind != PathKind::Lan {
            return Err(PlatformError::Unsupported("QUIC listener for Wi-Fi Aware").into());
        }
        self.ensure_endpoint()?;
        Ok(AnvilEndpoint::new(kind, format!("0.0.0.0:{LAN_PORT}")))
    }

    pub(crate) fn connect(&self, path: PathId, endpoint: &AnvilEndpoint) -> Result<()> {
        if endpoint.kind != PathKind::Lan {
            return Err(PlatformError::Unsupported("QUIC over Wi-Fi Aware").into());
        }
        let remote: SocketAddr = endpoint
            .address
            .parse()
            .map_err(|error| PlatformError::Adapter(format!("invalid LAN endpoint: {error}")))?;
        let quic = self.ensure_endpoint()?;
        let connecting = quic
            .connect(remote, "anvil.local")
            .map_err(|error| PlatformError::Adapter(format!("QUIC connect: {error}")))?;
        let outgoing = self.outgoing.clone();
        let pending = self.pending_inbound.clone();
        let handle = self.handle.clone();
        self.runtime.spawn(async move {
            match connecting.await {
                Ok(connection) => {
                    if let Ok(mut connections) = outgoing.lock() {
                        connections.insert(path, (remote, connection.clone()));
                    }
                    emit(
                        &handle,
                        PlatformEvent::PathEstablished {
                            path,
                            max_datagram_size: connection.max_datagram_size().unwrap_or_else(
                                || protocol_quic::conservative_datagram_size(PathKind::Lan),
                            ),
                        },
                    );
                    spawn_readers(handle.clone(), path, connection);

                    let accepted = pending
                        .lock()
                        .ok()
                        .and_then(|mut pending| pending.remove(&remote))
                        .unwrap_or_default();
                    for connection in accepted {
                        spawn_readers(handle.clone(), path, connection);
                    }
                }
                Err(error) => {
                    emit(&handle, PlatformEvent::PathLost { path, reason: error.to_string() })
                }
            }
        });
        Ok(())
    }

    pub(crate) fn close(&self, path: PathId) -> Result<()> {
        if let Some((_, connection)) =
            self.outgoing.lock().ok().and_then(|mut all| all.remove(&path))
        {
            connection.close(0u32.into(), b"path closed");
        }
        Ok(())
    }

    pub(crate) fn has_path(&self, path: PathId) -> bool {
        self.outgoing.lock().map(|all| all.contains_key(&path)).unwrap_or(false)
    }

    pub(crate) fn send_datagram(&self, path: PathId, data: &[u8]) -> Result<()> {
        let connection = self.connection(path)?;
        connection
            .send_datagram(Bytes::copy_from_slice(data))
            .map_err(|error| PlatformError::Adapter(format!("QUIC datagram: {error}")).into())
    }

    pub(crate) fn send_reliable(&self, path: PathId, data: &[u8]) -> Result<()> {
        if data.len() > MAX_CONTROL_RECORD {
            return Err(PlatformError::Adapter("control record exceeds 64 KiB".into()).into());
        }
        let connection = self.connection(path)?;
        let bytes = data.to_vec();
        self.runtime.spawn(async move {
            if let Ok(mut stream) = connection.open_uni().await {
                let _ = stream.write_all(&bytes).await;
                let _ = stream.finish();
            }
        });
        Ok(())
    }

    fn connection(&self, path: PathId) -> Result<Connection> {
        self.outgoing
            .lock()
            .ok()
            .and_then(|all| all.get(&path).map(|(_, connection)| connection.clone()))
            .ok_or_else(|| {
                PlatformError::Adapter(format!("QUIC path {} is not connected", path.0)).into()
            })
    }

    fn ensure_endpoint(&self) -> Result<Endpoint> {
        // Hold the lock across the *entire* check-and-build sequence, not
        // just the read and the final write.
        //
        // `listen()` runs as soon as this device starts advertising, and
        // `connect()` runs as soon as discovery finds a peer — on a LAN both
        // can fire within milliseconds of each other, from different
        // threads. With the lock only guarding the read and the final
        // store (as this used to), both threads see `None`, both generate a
        // self-signed certificate, both try to install a process-wide rustls
        // crypto provider, and both try to bind 0.0.0.0:47820 — the second
        // bind fails. Holding the lock the whole way through instead makes
        // the second caller simply block until the first finishes and reuse
        // its endpoint.
        let mut slot = self
            .endpoint
            .lock()
            .map_err(|_| PlatformError::Adapter("QUIC endpoint lock poisoned".into()))?;
        if let Some(endpoint) = slot.as_ref() {
            return Ok(endpoint.clone());
        }

        let _ = rustls::crypto::ring::default_provider().install_default();
        let certificate =
            rcgen::generate_simple_self_signed(vec!["anvil.local".into()]).map_err(adapter)?;
        let certificate_der = certificate.cert.der().clone();
        let private_key = PrivatePkcs8KeyDer::from(certificate.key_pair.serialize_der());

        let mut server_crypto = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(vec![certificate_der], private_key.into())
            .map_err(adapter)?;
        server_crypto.alpn_protocols = vec![protocol_quic::ALPN.to_vec()];
        let server_crypto = QuicServerConfig::try_from(server_crypto).map_err(adapter)?;
        let server = ServerConfig::with_crypto(Arc::new(server_crypto));

        let mut client_crypto = rustls::ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(SkipServerVerification::new())
            .with_no_client_auth();
        client_crypto.alpn_protocols = vec![protocol_quic::ALPN.to_vec()];
        let client_crypto = QuicClientConfig::try_from(client_crypto).map_err(adapter)?;

        let _guard = self.runtime.enter();
        let mut endpoint =
            Endpoint::server(server, SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), LAN_PORT))
                .map_err(adapter)?;
        endpoint.set_default_client_config(ClientConfig::new(Arc::new(client_crypto)));

        let accept_endpoint = endpoint.clone();
        let outgoing = self.outgoing.clone();
        let pending = self.pending_inbound.clone();
        let handle = self.handle.clone();
        self.runtime.spawn(async move {
            while let Some(incoming) = accept_endpoint.accept().await {
                let Ok(connection) = incoming.await else { continue };
                let remote = connection.remote_address();
                let path = outgoing.lock().ok().and_then(|connections| {
                    connections
                        .iter()
                        .find(|(_, (address, _))| *address == remote)
                        .map(|(path, _)| *path)
                });
                if let Some(path) = path {
                    spawn_readers(handle.clone(), path, connection);
                } else if let Ok(mut pending) = pending.lock() {
                    pending.entry(remote).or_default().push(connection);
                }
            }
        });

        // `slot` is still the guard acquired at the top of this function —
        // std::sync::Mutex is not reentrant, so re-locking `self.endpoint`
        // here would deadlock the very race this function exists to avoid.
        *slot = Some(endpoint.clone());
        Ok(endpoint)
    }
}

fn spawn_readers(handle: Arc<Mutex<Option<EngineHandle>>>, path: PathId, connection: Connection) {
    let datagram_connection = connection.clone();
    let datagram_handle = handle.clone();
    tokio::spawn(async move {
        while let Ok(data) = datagram_connection.read_datagram().await {
            emit(&datagram_handle, PlatformEvent::DatagramReceived { path, data: data.to_vec() });
        }
    });

    tokio::spawn(async move {
        while let Ok(mut stream) = connection.accept_uni().await {
            match stream.read_to_end(MAX_CONTROL_RECORD).await {
                Ok(data) => emit(&handle, PlatformEvent::ReliableReceived { path, data }),
                Err(_) => break,
            }
        }
    });
}

fn emit(handle: &Mutex<Option<EngineHandle>>, event: PlatformEvent) {
    if let Ok(handle) = handle.lock() {
        if let Some(handle) = handle.as_ref() {
            let _ = handle.platform(event);
        }
    }
}

fn adapter(error: impl core::fmt::Display) -> anvil_core::Error {
    PlatformError::Adapter(error.to_string()).into()
}

#[derive(Debug)]
struct SkipServerVerification(Arc<rustls::crypto::CryptoProvider>);

impl SkipServerVerification {
    fn new() -> Arc<Self> {
        Arc::new(Self(Arc::new(rustls::crypto::ring::default_provider())))
    }
}

impl ServerCertVerifier for SkipServerVerification {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> core::result::Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> core::result::Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> core::result::Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}
