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
/// An answered call is CallKit's until the call's voice I/O goes (`ducktape_phone_call_voice`, the host's `av::voice`
/// made with the first call piece and gone with the last: Leave, or the call over): then it is reported ended. The
/// audio session is the host's own (it activates it for the voice and lets it go after); CallKit's activation of the
/// same session (`didActivate`) is only logged, and a voice opened before CallKit's activation lands is retried by the
/// host. A call with no voice (another program's video-only call) never ends by this route: a CallKit ring is a voice
/// call. A withdraw (the call answered, declined or ended elsewhere, or over) is a background push to the regular
/// remote-notification token: the launcher asks UIKit for it beside PushKit's, and gpui_ios's app delegate hands Rust
/// the token (kind `apns`, to the device registry as PushKit's goes) and the push, which comes back here as JSON
/// (`ducktape_phone_remote_notification`): `{"kind": "withdraw", "tag"}` stops the ring under `tag`, and only a ring
/// (a call answered here is CallKit's until its voice goes). An app the person force-quit gets no background push:
/// `until` stays the ring's end there. Hanging up from CallKit's own screen opens the call's `end` link (the program's
/// `end_link`): what that hang-up means is the program's, as Answer's link is; a decline opens nothing. A call that
/// comes while one answered here is CallKit's is reported and ended at once, missed (busy).
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
  /// The calls ringing, by CallKit's id: the link each one's Answer opens, and the link ending the answered call
  /// from CallKit's own screen opens (`end`, empty for none).
  private var ringing: [UUID: (link: String, end: String)] = [:]
  /// The call answered here, CallKit's until the call's voice goes (`voice`), and what ending it from CallKit's
  /// screen opens: the program's `end_link`, never for a decline nor for an end the app makes.
  private var answered: (id: UUID, end: String)?
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
    // the regular token, which a withdraw is pushed to; silent pushes need no permission prompt
    UIApplication.shared.registerForRemoteNotifications()
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
    let end = payload["end"] as? String ?? ""
    let until = (payload["until"] as? NSNumber)?.doubleValue ?? Date().timeIntervalSince1970 + Ring.ringSeconds
    let update = CXCallUpdate()
    update.remoteHandle = CXHandle(type: .generic, value: title)
    update.localizedCallerName = title
    update.hasVideo = true
    // kept before CallKit hears of it: an Answer may come before the report's answer does
    let fresh = ringing[id] == nil
    ringing[id] = (link, end)
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
        // one call at a time here, as the provider says: a second call taken with End & Accept would end the first
        // from CallKit's screen (its end link, then its voice gone), and the voice's end names no call, so it would
        // end the new one in CallKit too; it is missed instead (busy)
        if self.answered != nil {
          self.unanswered(id)
          return
        }
        NSLog("ducktape ring: \(id) rings until \(until)")
        DispatchQueue.main.asyncAfter(deadline: .now() + max(until - Date().timeIntervalSince1970, 0)) {
          self.unanswered(id)
        }
      }
    }
  }

  /// A background push: a withdrawn call's ring under `tag` stops, if it still rings. Whether the push was taken.
  func remote(_ payload: [String: Any]) -> Bool {
    guard payload["kind"] as? String == "withdraw", let tag = payload["tag"] as? String else { return false }
    let id = Ring.id(tag)
    guard ringing.removeValue(forKey: id) != nil else {
      NSLog("ducktape ring: withdraw of \(tag), which does not ring here")
      return true
    }
    NSLog("ducktape ring: \(tag) withdrawn")
    provider.reportCall(with: id, endedAt: nil, reason: .remoteEnded)
    return true
  }

  /// The call under `id` stops ringing unanswered, if it still rings.
  private func unanswered(_ id: UUID) {
    guard ringing.removeValue(forKey: id) != nil else { return }
    NSLog("ducktape ring: \(id) unanswered")
    provider.reportCall(with: id, endedAt: nil, reason: .unanswered)
  }

  /// The call's voice I/O made (`alive`) or gone: with it goes the call answered here, if one is CallKit's. An end
  /// the app makes is asked of CallKit as the person's own hang-up would be (`CXEndCallAction` through the
  /// controller), with `answered` cleared first, so the action's handler takes it as one made here; a call CallKit
  /// no longer has refuses the action, and is reported ended instead.
  func voice(_ alive: Bool) {
    guard !alive, let id = answered?.id else { return }
    answered = nil
    NSLog("ducktape ring: \(id) over, its voice gone")
    calls.request(CXTransaction(action: CXEndCallAction(call: id))) { error in
      guard error != nil else { return }
      DispatchQueue.main.async { self.provider.reportCall(with: id, endedAt: nil, reason: .remoteEnded) }
    }
  }

  func providerDidReset(_ provider: CXProvider) {
    NSLog("ducktape ring: CallKit reset, \(ringing.count) ringing calls dropped")
    ringing.removeAll()
    answered = nil
  }

  /// Answered: the call's link opens, and it rings no more.
  func provider(_ provider: CXProvider, perform action: CXAnswerCallAction) {
    let links = ringing.removeValue(forKey: action.callUUID)
    if let link = links?.link, !link.isEmpty {
      ducktape_phone_link(link)
    }
    answered = (action.callUUID, links?.end ?? "")
    action.fulfill()
  }

  /// Declined while ringing: this phone's alone (SPEC D11), nothing opens. Hung up in CallKit's own screen while
  /// answered: the call's end link opens, so the program hears the end (an end the app made itself cleared
  /// `answered` first, `voice`).
  func provider(_ provider: CXProvider, perform action: CXEndCallAction) {
    NSLog("ducktape ring: \(action.callUUID) ended here")
    ringing.removeValue(forKey: action.callUUID)
    if let answered, answered.id == action.callUUID {
      self.answered = nil
      if !answered.end.isEmpty { ducktape_phone_link(answered.end) }
    }
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
      "link": "duck://test/huddle/call/ring-test", "end": "duck://test/huddle/call/ring-test/leave",
      "until": Date().timeIntervalSince1970 + 30,
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

/// The call's voice I/O made (1) or gone (0), from Rust (`crates/av`'s voice, any thread): the ring's answered call
/// follows it.
@_cdecl("ducktape_phone_call_voice")
public func ducktapePhoneCallVoice(_ active: UInt8) {
  DispatchQueue.main.async { Ring.shared.voice(active != 0) }
}

/// A background push from Rust (gpui_ios's app delegate), its payload as JSON; 1 when the ring took it. Main thread.
@_cdecl("ducktape_phone_remote_notification")
public func ducktapePhoneRemoteNotification(_ json: UnsafePointer<CChar>) -> UInt8 {
  let data = Data(bytes: json, count: strlen(json))
  guard let payload = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else {
    NSLog("ducktape ring: a push's payload was not a JSON object")
    return 0
  }
  return Ring.shared.remote(payload) ? 1 : 0
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
