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
        // The current implementation uses a this-device-only Keychain item.
        // It is OS protected but not wrapped by an enclave-held key yet.
        false
    }

    static func loadIdentity() -> Data? {
        var query = baseQuery
        query[kSecReturnData as String] = true
        query[kSecMatchLimit as String] = kSecMatchLimitOne
        var result: CFTypeRef?
        let status = SecItemCopyMatching(query as CFDictionary, &result)
        if status == errSecItemNotFound { return nil }
        guard status == errSecSuccess else {
            NSLog("Anvil: Keychain load failed: \(status)")
            return nil
        }
        return result as? Data
    }

    static func storeIdentity(_ data: Data) {
        let update = [kSecValueData as String: data]
        let status = SecItemUpdate(baseQuery as CFDictionary, update as CFDictionary)
        if status == errSecSuccess { return }
        if status != errSecItemNotFound {
            NSLog("Anvil: Keychain update failed: \(status)")
            return
        }

        var item = baseQuery
        item[kSecValueData as String] = data
        item[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
        let addStatus = SecItemAdd(item as CFDictionary, nil)
        if addStatus != errSecSuccess {
            NSLog("Anvil: Keychain add failed: \(addStatus)")
        }
    }

    static func clearIdentity() {
        let status = SecItemDelete(baseQuery as CFDictionary)
        if status != errSecSuccess && status != errSecItemNotFound {
            NSLog("Anvil: Keychain delete failed: \(status)")
        }
    }

    private static var baseQuery: [String: Any] {
        [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: "dev.anvil.identity",
            kSecAttrAccount as String: "installation-v1",
            kSecAttrSynchronizable as String: false,
        ]
    }
}
