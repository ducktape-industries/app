import Foundation
import UIKit

/// The phone's entry. gpui (zed's `gpui_ios`) runs UIKit itself with its own app and scene delegates, so this only
/// finds the app container's two folders and hands them to Rust (`crates/phone/src/os/ios/ffi.rs`), which never
/// returns. Documents is where the Expo build kept its keyrings (`remotes`): the same container, the same keys.
@main
enum App {
  static func main() {
    let files = FileManager.default
    let documents = files.urls(for: .documentDirectory, in: .userDomainMask)[0].path
    let caches = files.urls(for: .cachesDirectory, in: .userDomainMask)[0].path
    EdgeBack.shared.follow()
    ducktape_phone_main(documents, caches)
  }
}

/// Back on iOS (SPEC §7.6): a swipe in from the left edge, as UIKit's own stacks take it. gpui_ios hands the app raw
/// touches and no edge gesture, so the launcher puts one on gpui's window when it becomes key and tells Rust: where the
/// finger is while it moves (`ducktape_phone_back_drag`, the screen on top follows it), then Back
/// (`ducktape_phone_back`) or not (`ducktape_phone_back_cancel`). The touches it takes are cancelled for the view under
/// it.
final class EdgeBack: NSObject {
  static let shared = EdgeBack()
  private var observer: NSObjectProtocol?

  func follow() {
    observer = NotificationCenter.default.addObserver(
      forName: UIWindow.didBecomeKeyNotification, object: nil, queue: .main
    ) { [weak self] note in
      guard let self, let window = note.object as? UIWindow,
        !(window.gestureRecognizers ?? []).contains(where: { $0 is UIScreenEdgePanGestureRecognizer })
      else { return }
      let pan = UIScreenEdgePanGestureRecognizer(target: self, action: #selector(self.swiped(_:)))
      pan.edges = .left
      window.addGestureRecognizer(pan)
    }
  }

  /// A swipe let go past half the width, its speed counted for 0.3 s of travel, is Back; one short of it is not
  /// (react-native-screens' own rule, `RNSScreenStack.mm`).
  @objc private func swiped(_ pan: UIScreenEdgePanGestureRecognizer) {
    // a presented controller (a link's Safari view) takes its own edge swipe; the stack behind stays
    let presented = pan.view?.window?.rootViewController?.presentedViewController != nil
    let x = pan.translation(in: pan.view).x
    switch pan.state {
    case .began, .changed:
      if !presented { ducktape_phone_back_drag(Double(max(x, 0))) }
    case .ended:
      let width = pan.view?.bounds.width ?? 0
      if !presented && x + 0.3 * pan.velocity(in: pan.view).x > width / 2 {
        ducktape_phone_back()
      } else {
        ducktape_phone_back_cancel()
      }
    case .cancelled, .failed:
      ducktape_phone_back_cancel()
    default:
      break
    }
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
