import Foundation

/// The phone's entry. gpui (zed's `gpui_ios`) runs UIKit itself with its own app and scene delegates, so this only
/// finds the app container's two folders and hands them to Rust (`crates/phone/src/os/ios/ffi.rs`), which never
/// returns. Documents is where the Expo build kept its keyrings (`remotes`): the same container, the same keys.
@main
enum App {
  static func main() {
    let files = FileManager.default
    let documents = files.urls(for: .documentDirectory, in: .userDomainMask)[0].path
    let caches = files.urls(for: .cachesDirectory, in: .userDomainMask)[0].path
    ducktape_phone_main(documents, caches)
  }
}

/// Where the key under `alias` is kept, for Rust (`crates/phone/src/key/ios.rs`): `sealed` set to 1 or 0, `level`
/// to the store's word as a C string from `strdup` the caller frees. Null when done; else what went wrong, from
/// `strdup`, which the caller frees. The Expo build answered this to JS (`DeviceKeyModule.storage`); the store
/// itself is its file as it is.
@_cdecl("ducktape_device_key_storage")
public func ducktapeDeviceKeyStorage(
  _ alias: UnsafePointer<CChar>, _ sealed: UnsafeMutablePointer<UInt8>,
  _ level: UnsafeMutablePointer<UnsafeMutablePointer<CChar>?>
) -> UnsafeMutablePointer<CChar>? {
  do {
    let kept = try DeviceKeyStore.storage(String(cString: alias))
    guard let isSealed = kept["sealed"] as? Bool, let word = kept["level"] as? String
    else { return strdup("The key store answered no storage for \(String(cString: alias)).") }
    sealed.pointee = isSealed ? 1 : 0
    level.pointee = strdup(word)
    return nil
  } catch {
    return strdup(error.localizedDescription)
  }
}
