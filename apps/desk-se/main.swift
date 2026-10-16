// desk-se: the Tameion owner key, inside this Mac's Secure Enclave.
//
// The account's owner (contracts/src/PasskeyPrincipal.sol) verifies a
// WebAuthn assertion over P-256, the curve the Secure Enclave signs with. This
// tool wraps an Enclave signature in the same assertion a browser passkey
// produces, so the chip can own the account with no contract change. The
// private key never leaves the Enclave; the file on disk is a blob only this
// Mac's Enclave can unwrap.
//
//   desk-se create [--force] [--no-presence]   new key, Touch ID on every signature unless --no-presence; prints qx qy
//   desk-se pub                             prints qx qy
//   desk-se sign <challenge-hex> [--reason "<what is signed>"]   prints the assertion JSON; with a Touch ID
//                                           key the system prompt says the reason (clear-signing, docs/91 R18)
//   desk-se selftest                        signs 16 random challenges, checks each
//
// Blob path: $SE_BLOB, default ~/.tameion/se-owner.blob (mode 600).
// --presence binds the key to Touch ID / the login password: every signature
// then asks the person at the keyboard.
//
// Build: swiftc -O apps/desk-se/main.swift -o target/desk-se

import CryptoKit
import Foundation
import LocalAuthentication
import Security

let blobPath = ProcessInfo.processInfo.environment["SE_BLOB"]
    ?? NSHomeDirectory() + "/.tameion/se-owner.blob"

// The WebAuthn shape soft_passkey.py and the console page produce. The
// contract checks type, challenge, the UP and UV flags and the signature; it
// does not read the origin or the rpId hash, which are kept identical to the
// browser's so the service's /api/level1/execute accepts this too.
let origin = "http://localhost:7880"
let rpId = "localhost"
let flags: UInt8 = 0x05  // UP | UV: OpenZeppelin's WebAuthn.verify(..., requireUV = true)

// P-256 group order. OpenZeppelin's P256 refuses s > n/2 (malleability), and
// the Enclave returns either half, so high s is folded to n - s.
let n: [UInt8] = hex("FFFFFFFF00000000FFFFFFFFFFFFFFFFBCE6FAADA7179E84F3B9CAC2FC632551")!
let halfN: [UInt8] = hex("7fffffff800000007fffffffffffffffde737d56d38bcf4279dce5617e3192a8")!

func hex(_ s: String) -> [UInt8]? {
    let t = s.hasPrefix("0x") ? String(s.dropFirst(2)) : s
    guard t.count % 2 == 0 else { return nil }
    var out = [UInt8]()
    var i = t.startIndex
    while i < t.endIndex {
        let j = t.index(i, offsetBy: 2)
        guard let b = UInt8(t[i..<j], radix: 16) else { return nil }
        out.append(b)
        i = j
    }
    return out
}

func hexs<D: Sequence>(_ d: D) -> String where D.Element == UInt8 {
    "0x" + d.map { String(format: "%02x", $0) }.joined()
}

func die(_ msg: String) -> Never {
    FileHandle.standardError.write((msg + "\n").data(using: .utf8)!)
    exit(1)
}

func loadKey(reason: String? = nil) -> SecureEnclave.P256.Signing.PrivateKey {
    guard let blob = FileManager.default.contents(atPath: blobPath) else {
        die("no key at \(blobPath): run 'desk-se create'")
    }
    // The Touch ID prompt says what is being signed when a reason is given:
    // the context travels with the key into the Enclave's user check.
    let ctx = LAContext()
    if let reason { ctx.localizedReason = reason }
    do { return try SecureEnclave.P256.Signing.PrivateKey(dataRepresentation: blob, authenticationContext: ctx) }
    catch { die("this Mac's Secure Enclave cannot open \(blobPath): \(error)") }
}

func printPub(_ k: SecureEnclave.P256.Signing.PrivateKey) {
    let raw = [UInt8](k.publicKey.rawRepresentation)  // x || y, 32 bytes each
    print("qx \(hexs(raw[0..<32]))")
    print("qy \(hexs(raw[32..<64]))")
}

/// (r, low-s, folded?) for `message`, checked against the public key before it leaves.
func signLowS(_ k: SecureEnclave.P256.Signing.PrivateKey, _ message: Data) -> ([UInt8], [UInt8], Bool) {
    let sig: P256.Signing.ECDSASignature
    do { sig = try k.signature(for: message) } catch { die("Secure Enclave refused to sign: \(error)") }
    let raw = [UInt8](sig.rawRepresentation)
    let r = Array(raw[0..<32])
    var s = Array(raw[32..<64])
    let high = s.lexicographicallyPrecedes(halfN) == false && s != halfN
    if high {  // s = n - s, big-endian with borrow
        var borrow = 0
        for i in stride(from: 31, through: 0, by: -1) {
            let d = Int(n[i]) - Int(s[i]) - borrow
            s[i] = UInt8((d + 256) % 256)
            borrow = d < 0 ? 1 : 0
        }
    }
    guard s.lexicographicallyPrecedes(halfN) || s == halfN,
          let folded = try? P256.Signing.ECDSASignature(rawRepresentation: Data(r + s)),
          k.publicKey.isValidSignature(folded, for: message)
    else { die("signature failed its own check after low-s folding") }
    return (r, s, high)
}

/// The assertion for a 32-byte challenge, in the JSON PasskeyPolicy.s.sol `submit` reads.
func assertion(_ k: SecureEnclave.P256.Signing.PrivateKey, _ challenge: [UInt8]) -> ([String: Any], Bool) {
    let b64 = Data(challenge).base64EncodedString()
        .replacingOccurrences(of: "+", with: "-").replacingOccurrences(of: "/", with: "_")
        .replacingOccurrences(of: "=", with: "")
    let client = "{\"type\":\"webauthn.get\",\"challenge\":\"\(b64)\",\"origin\":\"\(origin)\",\"crossOrigin\":false}"
    let authData = [UInt8](SHA256.hash(data: Data(rpId.utf8))) + [flags, 0, 0, 0, 0]  // rpIdHash | flags | signCount 0
    let message = Data(authData) + Data(SHA256.hash(data: Data(client.utf8)))
    let (r, s, folded) = signLowS(k, message)
    let utf8 = Array(client.utf8)
    func index(_ needle: String) -> Int {
        let nd = Array(needle.utf8)
        return (0...(utf8.count - nd.count)).first { Array(utf8[$0..<$0 + nd.count]) == nd }!
    }
    return ([
        "r": hexs(r), "s": hexs(s),
        "challengeIndex": index("\"challenge\":\""), "typeIndex": index("\"type\":\"webauthn.get\""),
        "authenticatorData": hexs(authData), "clientDataJSON": client,
    ], folded)
}

var args = Array(CommandLine.arguments.dropFirst())
let cmd = args.isEmpty ? "" : args.removeFirst()
guard SecureEnclave.isAvailable else { die("no Secure Enclave on this machine") }

switch cmd {
case "create":
    let fm = FileManager.default
    if fm.fileExists(atPath: blobPath) {
        guard args.contains("--force") else {
            die("\(blobPath) exists and may own an account; pass --force to replace it (the old blob is kept as .old)")
        }
        let old = blobPath + ".\(Int(Date().timeIntervalSince1970)).old"
        do { try fm.moveItem(atPath: blobPath, toPath: old) } catch { die("could not keep the old blob: \(error)") }
        FileHandle.standardError.write("previous key kept at \(old)\n".data(using: .utf8)!)
    }
    let k: SecureEnclave.P256.Signing.PrivateKey
    do {
        // Touch ID / the login password on every signature by default (docs/89 M4):
        // the assertion claims user verification (UV), so the key must ask for it.
        // `--no-presence` is for unattended proofs only, and is recorded beside the blob.
        if !args.contains("--no-presence") {
            var err: Unmanaged<CFError>?
            guard let ac = SecAccessControlCreateWithFlags(
                nil, kSecAttrAccessibleWhenUnlockedThisDeviceOnly, [.privateKeyUsage, .userPresence], &err)
            else { die("access control: \(err!.takeRetainedValue())") }
            k = try SecureEnclave.P256.Signing.PrivateKey(accessControl: ac)
        } else {
            k = try SecureEnclave.P256.Signing.PrivateKey()
        }
    } catch { die("Secure Enclave refused to create a key: \(error)") }
    try? fm.createDirectory(atPath: (blobPath as NSString).deletingLastPathComponent, withIntermediateDirectories: true,
                            attributes: [.posixPermissions: 0o700])
    guard fm.createFile(atPath: blobPath, contents: k.dataRepresentation, attributes: [.posixPermissions: 0o600])
    else { die("could not write \(blobPath)") }
    let presence = args.contains("--no-presence") ? "0" : "1"
    _ = fm.createFile(atPath: blobPath + ".presence", contents: presence.data(using: .utf8), attributes: [.posixPermissions: 0o600])
    print("blob \(blobPath) (\(k.dataRepresentation.count) bytes, wrapped by this Mac's Secure Enclave)")
    printPub(k)
case "pub":
    printPub(loadKey())
case "sign":
    let reason = args.firstIndex(of: "--reason").flatMap { i in i + 1 < args.count ? args[i + 1] : nil }
    guard let c = args.first(where: { !$0.hasPrefix("--") && $0 != reason }).flatMap(hex), c.count == 32 else {
        die("usage: desk-se sign <32-byte challenge hex> [--unattended] [--reason \"<what is signed>\"]")
    }
    // A key made without Touch ID still sets the UV flag the contract requires; that
    // claim is false, so signing with one needs an explicit `--unattended` (docs/89 M4).
    // A blob from before the marker existed counts as made without presence.
    let presence = (try? String(contentsOfFile: blobPath + ".presence", encoding: .utf8))?.trimmingCharacters(in: .whitespacesAndNewlines) ?? "0"
    if presence != "1" {
        guard args.contains("--unattended") else {
            die("this key was made without Touch ID, so its 'user verified' flag would be false; pass --unattended to sign anyway (proofs only)")
        }
        FileHandle.standardError.write("warning: signing with a key that asks for no person; the UV flag claims a check that did not happen\n".data(using: .utf8)!)
    }
    let (json, _) = assertion(loadKey(reason: reason), c)
    let out = try! JSONSerialization.data(withJSONObject: json, options: [.sortedKeys, .withoutEscapingSlashes])
    print(String(data: out, encoding: .utf8)!)
case "selftest":
    // Every signature is verified inside signLowS; this shows the folding
    // branch ran and the folded signatures still verify.
    let k = loadKey()
    var folded = 0
    let t0 = Date()
    for _ in 0..<16 {
        let c = (0..<32).map { _ in UInt8.random(in: 0...255) }
        if assertion(k, c).1 { folded += 1 }
    }
    print("16 signatures verified against the public key; \(folded) had high s and were folded to n - s")
    print(String(format: "%.0f ms per signature", Date().timeIntervalSince(t0) * 1000 / 16))
    if folded == 0 { die("no high-s case in 16 tries (p = 2^-16): run again") }
default:
    die("usage: desk-se create [--force] [--no-presence] | pub | sign <challenge-hex> [--unattended] [--reason <text>] | selftest")
}
