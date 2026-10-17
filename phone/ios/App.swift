import AVFoundation
import CallKit
import CryptoKit
import Foundation
import PushKit
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
    Ring.shared.follow()
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

/// The ring (huddle call SPEC §5): a call's VoIP push becomes CallKit's incoming call, reported before anything else
/// (iOS ends an app that takes a VoIP push and reports no call). The payload is the carrier's (pushd): `tag` names the
/// call (CallKit's id is derived from it), `title` is who calls, `link` is what Answer opens through Rust's link path
/// (`ducktape_phone_link`, as an OS link), `until` (Unix seconds) is when it stops ringing unanswered. Decline is this
/// phone's alone (SPEC D11): nothing is sent. PushKit's token goes to Rust (`ducktape_phone_push_token`) for the device
/// registry. Made when UIKit finishes launching, which is also when a push that launched the app is delivered.
/// Not yet: a withdraw (the call seen or answered elsewhere) takes no ring down here. It comes as a regular background
/// push, and gpui_ios's app delegate hands the app neither that token nor that push (a gpui-pre hook to come), so a
/// call rings on until its `until`. Ending an answered call when the person leaves it is the call host's.
final class Ring: NSObject, PKPushRegistryDelegate, CXProviderDelegate {
  static let shared = Ring()
  /// The huddle program's ring, for a payload that carries no `until`: a call always carries one (SPEC §5).
  private static let ringSeconds: TimeInterval = 45
  private var observer: NSObjectProtocol?
  private var registry: PKPushRegistry?
  /// Made on first use, which is `start`, after launch.
  private lazy var provider: CXProvider = {
    let configuration = CXProviderConfiguration()
    configuration.supportsVideo = true
    configuration.maximumCallGroups = 1
    configuration.maximumCallsPerCallGroup = 1
    configuration.supportedHandleTypes = [.generic]
    configuration.includesCallsInRecents = false
    return CXProvider(configuration: configuration)
  }()
  /// The calls ringing, by CallKit's id: the link each one's Answer opens.
  private var ringing: [UUID: String] = [:]
  /// The debug entry's taps (`test`), made with the provider.
  private lazy var calls = CXCallController()

  func follow() {
    observer = NotificationCenter.default.addObserver(
      forName: UIApplication.didFinishLaunchingNotification, object: nil, queue: .main
    ) { [weak self] _ in self?.start() }
  }

  private func start() {
    provider.setDelegate(self, queue: nil)
    // the controller before any call, so its observer sees them all
    _ = calls
    // after the provider: a push waiting for the app is delivered as soon as the registry wants it
    let registry = PKPushRegistry(queue: .main)
    registry.delegate = self
    registry.desiredPushTypes = [.voIP]
    self.registry = registry
    if let mode = ProcessInfo.processInfo.environment["DUCKTAPE_RING_TEST"] { test(mode) }
  }

  /// CallKit's id for the call under `tag`: the same tag, the same call, whichever push names it.
  static func id(_ tag: String) -> UUID {
    NSUUID(uuidBytes: Array(Insecure.MD5.hash(data: Data(tag.utf8)))) as UUID
  }

  func pushRegistry(_ registry: PKPushRegistry, didUpdate credentials: PKPushCredentials, for type: PKPushType) {
    let token = credentials.token.map { String(format: "%02x", $0) }.joined()
    ducktape_phone_push_token("voip", token)
  }

  func pushRegistry(_ registry: PKPushRegistry, didInvalidatePushTokenFor type: PKPushType) {
    ducktape_phone_push_token("voip", "")
  }

  func pushRegistry(
    _ registry: PKPushRegistry, didReceiveIncomingPushWith payload: PKPushPayload, for type: PKPushType,
    completion: @escaping () -> Void
  ) {
    ring(payload.dictionaryPayload, completion: completion)
  }

  /// `payload` reported as an incoming call, then rung until its `until`; one already past it is reported and ended
  /// at once, since every VoIP push must report a call.
  func ring(_ payload: [AnyHashable: Any], completion: @escaping () -> Void = {}) {
    let id = Ring.id(payload["tag"] as? String ?? "")
    let title = payload["title"] as? String ?? ""
    let link = payload["link"] as? String ?? ""
    let until = (payload["until"] as? NSNumber)?.doubleValue ?? Date().timeIntervalSince1970 + Ring.ringSeconds
    let update = CXCallUpdate()
    update.remoteHandle = CXHandle(type: .generic, value: title)
    update.localizedCallerName = title
    update.hasVideo = true
    // kept before CallKit hears of it: an Answer may come before the report's answer does
    let fresh = ringing[id] == nil
    ringing[id] = link
    provider.reportNewIncomingCall(with: id, update: update) { error in
      // CallKit answers on a queue of its own; the calls are kept on the main one
      DispatchQueue.main.async {
        defer { completion() }
        if let error {
          // a call under this id rings already (its link stays), or the person's Do Not Disturb refused it
          NSLog("ducktape ring: CallKit did not take the call: \(error)")
          if fresh { self.ringing.removeValue(forKey: id) }
          return
        }
        NSLog("ducktape ring: \(id) rings until \(until)")
        DispatchQueue.main.asyncAfter(deadline: .now() + max(until - Date().timeIntervalSince1970, 0)) {
          self.unanswered(id)
        }
      }
    }
  }

  /// The call under `id` stops ringing unanswered, if it still rings.
  private func unanswered(_ id: UUID) {
    guard ringing.removeValue(forKey: id) != nil else { return }
    NSLog("ducktape ring: \(id) unanswered")
    provider.reportCall(with: id, endedAt: nil, reason: .unanswered)
  }

  func providerDidReset(_ provider: CXProvider) {
    NSLog("ducktape ring: CallKit reset, \(ringing.count) ringing calls dropped")
    ringing.removeAll()
  }

  /// Answered: the call's link opens, and it rings no more.
  func provider(_ provider: CXProvider, perform action: CXAnswerCallAction) {
    if let link = ringing.removeValue(forKey: action.callUUID), !link.isEmpty {
      ducktape_phone_link(link)
    }
    action.fulfill()
  }

  /// Declined while ringing, or hung up in CallKit's screen: this phone's alone (SPEC D11).
  func provider(_ provider: CXProvider, perform action: CXEndCallAction) {
    NSLog("ducktape ring: \(action.callUUID) ended here")
    ringing.removeValue(forKey: action.callUUID)
    action.fulfill()
  }

  /// CallKit's audio session, up once a call is answered and down when it ends: the voice the host plays and records
  /// opens on it.
  func provider(_ provider: CXProvider, didActivate audioSession: AVAudioSession) {
    ducktape_phone_call_audio(1)
  }

  func provider(_ provider: CXProvider, didDeactivate audioSession: AVAudioSession) {
    ducktape_phone_call_audio(0)
  }

  /// A debug launch's `DUCKTAPE_RING_TEST` (`SIMCTL_CHILD_DUCKTAPE_RING_TEST=answer xcrun simctl launch ...`, or the
  /// scheme's environment): a sample call 2 s after launch through the push's own path, ringing 30 s; `answer` or
  /// `decline` then act on it 3 s later through CallKit's own actions, as the person's tap does, so the flow runs
  /// without APNs and without a finger. Any other value only rings.
  private func test(_ mode: String) {
    let tag = "call/ring-test"
    let payload: [AnyHashable: Any] = [
      "kind": "call", "chain": "test", "program": "huddle", "tag": tag, "title": "Ring test", "body": "",
      "link": "duck://test/huddle/call/ring-test", "until": Date().timeIntervalSince1970 + 30,
    ]
    let id = Ring.id(tag)
    let action: CXAction? =
      switch mode {
      case "answer": CXAnswerCallAction(call: id)
      case "decline": CXEndCallAction(call: id)
      default: nil
      }
    DispatchQueue.main.asyncAfter(deadline: .now() + 2) {
      self.ring(payload)
      guard let action else { return }
      DispatchQueue.main.asyncAfter(deadline: .now() + 3) {
        let known = self.calls.callObserver.calls.map { "\($0.uuid) answered \($0.hasConnected)" }
        NSLog("ducktape ring test: \(mode) \(id), CallKit has \(known)")
        self.calls.request(CXTransaction(action: action)) { error in
          NSLog("ducktape ring test: \(mode): \(error.map { "\($0)" } ?? "requested")")
          // the simulator's CallKit takes the call and ends it at once (it has no call screen), so it refuses the
          // tap: the link goes the way Answer sends it
          if error != nil && mode == "answer", let link = payload["link"] as? String {
            DispatchQueue.main.async { ducktape_phone_link(link) }
          }
        }
      }
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
