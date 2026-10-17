// Find or make the Developer ID provisioning profile (MAC_APP_DIRECT) that
// lets one Developer ID Application certificate sign one macOS bundle ID,
// through the App Store Connect API, and write it out. release.yml's
// macos-sign embeds it as Contents/embedded.provisionprofile: macOS lets an
// app use the data protection keychain, where a Touch ID-locked key lives,
// only with an application identifier and a keychain access group that an
// embedded profile authorizes (TN3137).
//
//   swift ops/apple-profile.swift <bundle id> <certificate serial> <out.provisionprofile>
//
// The serial is the signing certificate's, in hex (`openssl x509 -serial`).
// The API key is the notary key: DUCKTAPE_NOTARY_KEY (the .p8's path),
// DUCKTAPE_NOTARY_KEY_ID, DUCKTAPE_NOTARY_ISSUER. A team without the bundle
// ID gets it (platform MAC_OS); an ACTIVE profile of it that holds the
// certificate and has 30 days left is reused, else one is made. Every API
// failure exits non-zero with the HTTP status and Apple's own words.
import CryptoKit
import Foundation

func fail(_ message: String) -> Never {
    FileHandle.standardError.write(Data("apple-profile: \(message)\n".utf8))
    exit(1)
}

func environment(_ name: String) -> String {
    guard let value = ProcessInfo.processInfo.environment[name], !value.isEmpty else {
        fail("\(name) is not set")
    }
    return value
}

// openssl and the API may write one serial in another case, with colons or
// leading zeros: compare the number
func normalizedSerial(_ serial: String) -> String {
    String(serial.uppercased().filter { $0 != ":" }.drop { $0 == "0" })
}

// Apple writes "2041-10-17T07:20:00.000+00:00", fractional seconds or not.
// An unreadable date fails the run: read as expired, it would make a new
// profile every run.
func expiration(_ profile: Resource) -> Date {
    let text = profile.attributes?.expirationDate ?? ""
    let fractional = ISO8601DateFormatter()
    fractional.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
    guard let date = fractional.date(from: text) ?? ISO8601DateFormatter().date(from: text) else {
        fail("profile \(profile.id) has an unreadable expirationDate \"\(text)\"")
    }
    return date
}

func base64URL(_ data: Data) -> String {
    data.base64EncodedString()
        .replacingOccurrences(of: "+", with: "-")
        .replacingOccurrences(of: "/", with: "_")
        .replacingOccurrences(of: "=", with: "")
}

// The API's bearer token, an ES256 JWT. Apple refuses one that lives over
// 20 minutes; 10 leave room for a runner clock ahead of Apple's.
func token(key: P256.Signing.PrivateKey, keyID: String, issuer: String, now: Date) throws -> String {
    let issued = Int(now.timeIntervalSince1970)
    let header: [String: Any] = ["alg": "ES256", "kid": keyID, "typ": "JWT"]
    let claims: [String: Any] = ["iss": issuer, "iat": issued, "exp": issued + 600, "aud": "appstoreconnect-v1"]
    let input = try [header, claims]
        .map { base64URL(try JSONSerialization.data(withJSONObject: $0)) }
        .joined(separator: ".")
    // r ‖ s, 64 bytes: JWS's form of an ECDSA signature (not DER)
    let signature = try key.signature(for: Data(input.utf8)).rawRepresentation
    return input + "." + base64URL(signature)
}

// The JSON:API resources read here, only the fields used
struct Resource: Decodable {
    struct Attributes: Decodable {
        let identifier: String?
        let platform: String?
        let serialNumber: String?
        let name: String?
        let profileContent: String?
        let expirationDate: String?
    }
    struct Link: Decodable { let id: String }
    struct ToOne: Decodable { let data: Link? }
    struct ToMany: Decodable { let data: [Link]? }
    struct Relationships: Decodable {
        let bundleId: ToOne?
        let certificates: ToMany?
    }
    let id: String
    let attributes: Attributes?
    let relationships: Relationships?
}

struct Document<Content: Decodable>: Decodable { let data: Content }

struct Failure: Decodable {
    struct Entry: Decodable { let code: String?; let detail: String? }
    let errors: [Entry]
}

struct API {
    let token: String
    let base = "https://api.appstoreconnect.apple.com/v1/"

    func get(_ path: String, _ query: [String: String]) async -> [Resource] {
        var url = URLComponents(string: base + path)!
        url.queryItems = query.map { URLQueryItem(name: $0.key, value: $0.value) }
        return await send(URLRequest(url: url.url!), as: Document<[Resource]>.self).data
    }

    func post(_ path: String, _ data: [String: Any]) async -> Resource {
        var request = URLRequest(url: URL(string: base + path)!)
        request.httpMethod = "POST"
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        do {
            request.httpBody = try JSONSerialization.data(withJSONObject: ["data": data])
        } catch {
            fail("POST /v1/\(path): \(error)")
        }
        return await send(request, as: Document<Resource>.self).data
    }

    private func send<Body: Decodable>(_ request: URLRequest, as _: Body.Type) async -> Body {
        var request = request
        request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        let call = "\(request.httpMethod ?? "GET") \(request.url!.path)"
        let body: Data
        let response: URLResponse
        do {
            (body, response) = try await URLSession.shared.data(for: request)
        } catch {
            fail("\(call): \(error.localizedDescription)")
        }
        let status = (response as! HTTPURLResponse).statusCode
        guard (200..<300).contains(status) else {
            let errors = (try? JSONDecoder().decode(Failure.self, from: body))?.errors ?? []
            let words = errors.map { [$0.code, $0.detail].compactMap { $0 }.joined(separator: ": ") }
            fail("\(call): HTTP \(status): "
                + (words.isEmpty ? String(decoding: body, as: UTF8.self) : words.joined(separator: "; ")))
        }
        do {
            return try JSONDecoder().decode(Body.self, from: body)
        } catch {
            fail("\(call): unreadable answer: \(error)")
        }
    }
}

let arguments = CommandLine.arguments
guard arguments.count == 4 else {
    fail("usage: apple-profile.swift <bundle id> <certificate serial> <out.provisionprofile>")
}
let (bundleIdentifier, serial, out) = (arguments[1], normalizedSerial(arguments[2]), arguments[3])
let key: P256.Signing.PrivateKey
do {
    let path = environment("DUCKTAPE_NOTARY_KEY")
    key = try P256.Signing.PrivateKey(pemRepresentation: String(contentsOfFile: path, encoding: .utf8))
} catch {
    fail("DUCKTAPE_NOTARY_KEY: \(error)")
}
let api: API
do {
    api = API(token: try token(key: key, keyID: environment("DUCKTAPE_NOTARY_KEY_ID"),
        issuer: environment("DUCKTAPE_NOTARY_ISSUER"), now: Date()))
} catch {
    fail("the API token: \(error)")
}

// ponytail: one page of 200, the API's most; follow links.next if a team
// ever has more Developer ID certificates or Developer ID profiles
let certificates = await api.get("certificates", [
    "filter[certificateType]": "DEVELOPER_ID_APPLICATION,DEVELOPER_ID_APPLICATION_G2", "limit": "200",
])
guard let certificate = certificates.first(where: { normalizedSerial($0.attributes?.serialNumber ?? "") == serial })
else {
    let listed = certificates.map { $0.attributes?.serialNumber ?? $0.id }.joined(separator: ", ")
    fail("no Developer ID Application certificate on the team has serial \(serial) (the team's: \(listed))")
}

// the filter is not an exact match: the identifier is compared here
let bundle: Resource
if let found = await api.get("bundleIds", ["filter[identifier]": bundleIdentifier, "limit": "200"])
    .first(where: { $0.attributes?.identifier == bundleIdentifier }) {
    let platform = found.attributes?.platform ?? "no platform"
    guard platform == "MAC_OS" || platform == "UNIVERSAL" else {
        fail("\(bundleIdentifier) is registered on the team for \(platform); a Developer ID profile needs MAC_OS or UNIVERSAL")
    }
    bundle = found
} else {
    // an App ID's name takes no dots
    let name = bundleIdentifier.replacingOccurrences(of: ".", with: " ")
    bundle = await api.post("bundleIds", [
        "type": "bundleIds",
        "attributes": ["identifier": bundleIdentifier, "name": name, "platform": "MAC_OS"],
    ])
}

let active = await api.get("profiles", [
    "filter[profileType]": "MAC_APP_DIRECT", "filter[profileState]": "ACTIVE",
    "include": "bundleId,certificates", "limit": "200", "limit[certificates]": "50",
])
// A Developer ID app stops launching once its embedded profile expires, and
// a release stays installed long after this run: a profile is reused only
// with 30 days left, else a new one is made.
let reusableUntil = Date().addingTimeInterval(30 * 24 * 60 * 60)
let reused = active.first { profile in
    profile.relationships?.bundleId?.data?.id == bundle.id
        && profile.relationships?.certificates?.data?.contains { $0.id == certificate.id } == true
        && expiration(profile) > reusableUntil
}
let profile: Resource
if let reused {
    profile = reused
} else {
    // profile names are unique within the team, and an INVALID profile keeps
    // its name: the Unix time keeps a new one from colliding with it
    let name = "\(bundleIdentifier) Developer ID \(Int(Date().timeIntervalSince1970))"
    profile = await api.post("profiles", [
        "type": "profiles",
        "attributes": ["name": name, "profileType": "MAC_APP_DIRECT"],
        "relationships": [
            "bundleId": ["data": ["type": "bundleIds", "id": bundle.id]],
            "certificates": ["data": [["type": "certificates", "id": certificate.id]]],
        ],
    ])
}
guard let content = profile.attributes?.profileContent, let bytes = Data(base64Encoded: content) else {
    fail("profile \(profile.id) came without its content")
}
do {
    try bytes.write(to: URL(fileURLWithPath: out))
} catch {
    fail("\(out): \(error.localizedDescription)")
}
print("apple-profile: \(reused == nil ? "made" : "reused") profile \"\(profile.attributes?.name ?? profile.id)\" for \(bundleIdentifier) and certificate \(serial): \(out)")
