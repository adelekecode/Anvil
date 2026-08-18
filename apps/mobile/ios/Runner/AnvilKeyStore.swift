import Foundation
import LocalAuthentication
import Security

/// Identity key storage on iOS (§82).
///
/// The device identity key belongs in the Keychain with
/// `kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly` — Anvil must work when
/// the phone is locked and in a pocket, but the key should never sync to iCloud
/// or restore onto a different device. A restored identity would mean two
/// phones claiming to be the same peer, which the protocol has no way to
/// resolve.
///
/// Secure Enclave protection is preferable where the key type allows it. Note
/// the Enclave supports P-256, not Ed25519, so hardware-backed protection here
/// means wrapping the Ed25519 private key with an Enclave-held key rather than
/// generating it inside the Enclave. That is still meaningfully stronger than a
/// plain Keychain item, and `hasSecureEnclave()` should report what is actually
/// in use so the diagnostics view does not overstate the guarantee.
///
/// PHASE2.
enum AnvilKeyStore {

    static func hasSecureEnclave() -> Bool {
        LAContext().canEvaluatePolicy(.deviceOwnerAuthenticationWithBiometrics, error: nil)
    }

    static func loadIdentity() -> Data? {
        // PHASE2
        nil
    }

    static func storeIdentity(_ data: Data) {
        // PHASE2
    }

    static func clearIdentity() {
        // PHASE2
    }
}
