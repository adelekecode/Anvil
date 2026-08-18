//! Wi-Fi LAN path (§13, §20, §63).
//!
//! The easy transport, with one trap worth naming up front: **a router is not
//! the Internet**. Anvil's LAN mode must work on a router with the WAN cable
//! pulled, on a hotspot with no data plan, on a travel router in a field. So:
//!
//! * no DNS beyond mDNS/NSD,
//! * no NAT traversal, STUN or TURN — every peer is on the same subnet,
//! * no captive-portal or connectivity probing, and no reacting to the OS
//!   deciding the network "has no Internet". Android in particular will happily
//!   report a validated-less network and may steer sockets to cellular; the
//!   adapter must bind explicitly to the Wi-Fi network rather than trusting the
//!   default route.
//!
//! The last point is the one that will actually bite. It is an adapter
//! responsibility (`ConnectivityManager.bindProcessToNetwork` / `requestNetwork`
//! on Android, `NWParameters.requiredInterfaceType = .wifi` on iOS), and it is
//! why [`crate::platform::TransportAdapter::listen`] takes a [`PathKind`]
//! rather than letting the OS choose.
//!
//! ## Isolation
//!
//! Plenty of real networks enable client isolation (guest Wi-Fi, many
//! enterprise APs), where peers can reach the gateway but not each other.
//! Discovery may succeed while every connection attempt silently fails. The
//! core's answer is already correct — the LAN path never reaches `Ready`, so it
//! never scores, and Wi-Fi Aware wins — but the UI should be able to say why,
//! so this deserves a distinct diagnostic rather than a generic timeout.

use super::PathKind;

/// The transport family this module describes.
pub const KIND: PathKind = PathKind::Lan;

/// Default UDP port Anvil binds for LAN traffic.
///
/// Advertised in discovery rather than assumed, so a second instance on one
/// device (or a port conflict) is survivable. Registered with IANA: no. Chosen
/// from the dynamic range for exactly that reason.
pub const DEFAULT_PORT: u16 = 47_820;

/// Conservative datagram size for LAN.
///
/// 1200 bytes keeps a QUIC datagram inside the common 1500-byte Ethernet MTU
/// after IP, UDP and QUIC headers, with room for tunnelling overhead. Path MTU
/// discovery can raise this later; guessing high and fragmenting is worse than
/// guessing low, because a fragmented voice frame loses to a single dropped
/// fragment.
pub const CONSERVATIVE_DATAGRAM_SIZE: usize = 1_200;
