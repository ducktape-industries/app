import AVFoundation
import CallKit
import CryptoKit
import Foundation
import PushKit
import UIKit
import UserNotifications

/// The phone's entry. gpui (zed's `gpui_ios`) runs UIKit itself with its own app and scene delegates, so this only
/// finds the app container's two folders and hands them to Rust (`crates/phone/src/os/ios/ffi.rs`), which never
/// returns. Documents is where the Expo build kept its keyrings (`remotes`): the same container, the same keys.
@main
enum App {
  static func main() {
    let files = FileManager.default
    let documents = files.urls(for: .documentDirectory, in: .userDomainMask)[0].path
    let caches = files.urls(for: .cachesDirectory, in: .userDomainMask)[0].path
    Swipe.shared.follow()
    Ring.shared.follow()
    ducktape_phone_main(documents, caches)
  }
}

/// Back on iOS (SPEC §7.6): a swipe to the right, as iOS 26's own stacks take it: in from the left edge, over anything,
/// and from anywhere else on the screen when Rust says it begins there (`ducktape_phone_swipe_begins`: something to go
/// back to, nothing over the screen, nothing under the finger that takes a sideways drag itself, the move clearly
/// rightward). At a tab root the same swipe, either way, pages the tabs (Rust says so too). gpui_ios hands the app raw
/// touches and no gesture of the system's, so the launcher puts both pans on gpui's window when it becomes key, the
/// edge's first: the other waits for it to fail, so the edge always wins. Either tells Rust where the finger is (its
/// way across, negative to the left) and how fast it moves while it moves (`ducktape_phone_swipe_drag`, the screen on
/// top follows it), then that it let go (`ducktape_phone_swipe_let_go`: Rust judges Back, or the tab it lands on) or
/// ended (`ducktape_phone_swipe_cancel`). The touches a swipe takes are cancelled for the view under it.
final class Swipe: NSObject, UIGestureRecognizerDelegate {
  static let shared = Swipe()
  private var observer: NSObjectProtocol?
  /// When the finger the swipe from anywhere would follow came down, on the touches' clock.
  private var down: TimeInterval = 0

  func follow() {
    observer = NotificationCenter.default.addObserver(
      forName: UIWindow.didBecomeKeyNotification, object: nil, queue: .main
    ) { [weak self] note in
      guard let self, let window = note.object as? UIWindow,
        !(window.gestureRecognizers ?? []).contains(where: { $0 is UIScreenEdgePanGestureRecognizer })
      else { return }
      let edge = UIScreenEdgePanGestureRecognizer(target: self, action: #selector(self.swiped(_:)))
      edge.edges = .left
      window.addGestureRecognizer(edge)
      let anywhere = UIPanGestureRecognizer(target: self, action: #selector(self.swiped(_:)))
      anywhere.maximumNumberOfTouches = 1
      anywhere.delegate = self
      anywhere.require(toFail: edge)
      window.addGestureRecognizer(anywhere)
    }
  }

  func gestureRecognizer(_ recognizer: UIGestureRecognizer, shouldReceive touch: UITouch) -> Bool {
    down = touch.timestamp
    return true
  }

  /// The swipe from anywhere, as UIKit is about to start it (at the finger's first move of a point or two): where its
  /// finger came down, how fast it moves, how long it rested first, for Rust to judge.
  func gestureRecognizerShouldBegin(_ recognizer: UIGestureRecognizer) -> Bool {
    guard let pan = recognizer as? UIPanGestureRecognizer, let view = pan.view, !Swipe.presented(view) else {
      return false
    }
    let at = pan.location(in: view)
    let moved = pan.translation(in: view)
    let moving = pan.velocity(in: view)
    let held = ProcessInfo.processInfo.systemUptime - down
    return ducktape_phone_swipe_begins(
      Double(at.x - moved.x), Double(at.y - moved.y), Double(moving.x), Double(moving.y), held) != 0
  }

  /// A presented controller (a link's Safari view) takes its own swipe; the stack behind stays.
  private static func presented(_ view: UIView) -> Bool {
    view.window?.rootViewController?.presentedViewController != nil
  }

  @objc private func swiped(_ pan: UIPanGestureRecognizer) {
    guard let view = pan.view else { return }
    let presented = Swipe.presented(view)
    let x = Double(pan.translation(in: view).x)
    let speed = Double(pan.velocity(in: view).x)
    switch pan.state {
    case .began, .changed:
      if !presented { ducktape_phone_swipe_drag(x, speed) }
    case .ended where !presented:
      ducktape_phone_swipe_drag(x, speed)
      ducktape_phone_swipe_let_go()
    case .ended, .cancelled, .failed:
      ducktape_phone_swipe_cancel()
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
/// comes while one answered here is CallKit's is reported and ended at once, missed (busy). A tap on a program's
/// notice (an alert to the regular token) opens its `link`, as Answer does.
///
/// Every program's notice the OS shows goes to Rust for the Notifications tab (`ducktape_phone_notice`, its payload
/// as JSON and when the OS showed it): those still in the OS's list at launch and each time the app comes to the
/// front, one opened from the list (before its link), and one that comes while the app is in front. One the person
/// swiped away before the app came to the front is not in the list: what the program's notifier here posts when it
/// runs again is what the tab gets of it.
///
/// While the app is in front a notice is the OS's own banner all the same, once: this phone runs the notifiers of the
/// chain in hand, and what they post Rust raises as a banner of this phone's own (`ducktape_phone_banner`, the push's
/// payload under the notice's id); a push for that chain then shows nothing, and one for another chain shows (Rust
/// answers which). The notifiers leave alone what the person is looking at, which the push server cannot see.
final class Ring: NSObject, PKPushRegistryDelegate, CXProviderDelegate, UNUserNotificationCenterDelegate {
  static let shared = Ring()
  /// The huddle program's ring, for a payload that carries no `until`: a call always carries one (SPEC §5).
  private static let ringSeconds: TimeInterval = 45
  private var observer: NSObjectProtocol?
  private var foreground: NSObjectProtocol?
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
    // before launch ends: a tap that launched the app is handed to the delegate set by then
    UNUserNotificationCenter.current().delegate = self
    observer = NotificationCenter.default.addObserver(
      forName: UIApplication.didFinishLaunchingNotification, object: nil, queue: .main
    ) { [weak self] _ in self?.start() }
    foreground = NotificationCenter.default.addObserver(
      forName: UIApplication.willEnterForegroundNotification, object: nil, queue: .main
    ) { [weak self] _ in self?.delivered() }
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
    // the regular token, which a withdraw and a program's notices are pushed to; a withdraw (silent) needs no
    // permission, a notice shows only once the person allows alerts (iOS asks once)
    UIApplication.shared.registerForRemoteNotifications()
    UNUserNotificationCenter.current().requestAuthorization(options: [.alert, .sound]) { allowed, error in
      NSLog("ducktape ring: alerts \(allowed ? "allowed" : "not allowed")\(error.map { ": \($0)" } ?? "")")
    }
    if let mode = ProcessInfo.processInfo.environment["DUCKTAPE_RING_TEST"] { test(mode) }
    delivered()
  }

  /// The notices still in the OS's list, each to Rust as the OS showed it, oldest first.
  private func delivered() {
    UNUserNotificationCenter.current().getDeliveredNotifications { notices in
      let oldest = notices.sorted { $0.date < $1.date }
      DispatchQueue.main.async { oldest.forEach { Ring.hand($0, front: false, opened: false) } }
    }
  }

  /// Every notice under pushd's `tag` (each its own notification, the tag in its payload) out of the OS's list,
  /// then `done`. Any thread.
  static func takeDown(_ tag: String, done: @escaping () -> Void = {}) {
    let center = UNUserNotificationCenter.current()
    center.getDeliveredNotifications { delivered in
      center.removeDeliveredNotifications(
        withIdentifiers: delivered.filter { $0.request.content.userInfo["tag"] as? String == tag }
          .map { $0.request.identifier })
      done()
    }
  }

  /// `notice` to Rust (`ducktape_phone_notice`), its payload as JSON; one that is no program's notice (no carrier's
  /// `kind: notice`) is let go there. Whether the OS shows one that came in front.
  @discardableResult
  static func hand(_ notice: UNNotification, front: Bool, opened: Bool) -> Bool {
    let payload = notice.request.content.userInfo
    guard JSONSerialization.isValidJSONObject(payload),
      let data = try? JSONSerialization.data(withJSONObject: payload),
      let json = String(data: data, encoding: .utf8)
    else { return false }
    return ducktape_phone_notice(json, notice.date.timeIntervalSince1970, front ? 1 : 0, opened ? 1 : 0) != 0
  }

  /// This phone's own banner of a program's notice (Rust's, the app in front): `payload` as its `userInfo`, so a tap on
  /// it is a pushed one's (`didReceive`) and a withdraw of its tag takes it down (`takeDown`); stacked with its tag's
  /// and under its id, the push's own for the same notice (`Notice::os_id`), which replaces it, never stands beside it.
  static func banner(_ payload: [String: Any]) {
    let content = UNMutableNotificationContent()
    content.title = payload["title"] as? String ?? ""
    content.body = payload["body"] as? String ?? ""
    content.threadIdentifier = payload["tag"] as? String ?? ""
    // the quack (`SOUND`, crates/host/src/runtime/notify.rs), bundled from assets/sounds (project.yml)
    content.sound = UNNotificationSound(named: UNNotificationSoundName(rawValue: "quack.wav"))
    content.userInfo = payload
    let id = payload["id"] as? String ?? UUID().uuidString
    UNUserNotificationCenter.current().add(UNNotificationRequest(identifier: id, content: content, trigger: nil)) {
      if let error = $0 { NSLog("ducktape ring: a banner did not go up: \(error)") }
    }
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

  /// A background push: a withdrawn call's ring under `tag` stops, if it still rings; else every notice under
  /// `tag` (each its own notification, pushd's `tag` in its payload) leaves the notification list. Whether the push
  /// was taken.
  func remote(_ payload: [String: Any]) -> Bool {
    guard payload["kind"] as? String == "withdraw", let tag = payload["tag"] as? String else { return false }
    let id = Ring.id(tag)
    guard ringing.removeValue(forKey: id) != nil else {
      NSLog("ducktape ring: withdraw of \(tag), which does not ring here")
      // the push is answered as this returns and iOS may suspend the app then: held awake until the list is read
      var awake = UIBackgroundTaskIdentifier.invalid
      awake = UIApplication.shared.beginBackgroundTask(withName: "ducktape withdraw") {
        UIApplication.shared.endBackgroundTask(awake)
      }
      Ring.takeDown(tag) { UIApplication.shared.endBackgroundTask(awake) }
      return true
    }
    NSLog("ducktape ring: \(tag) withdrawn")
    provider.reportCall(with: id, endedAt: nil, reason: .remoteEnded)
    return true
  }

  /// A notice tapped: its row read, then its link opens (a dismissal opens nothing).
  func userNotificationCenter(
    _ center: UNUserNotificationCenter, didReceive response: UNNotificationResponse,
    withCompletionHandler completionHandler: @escaping () -> Void
  ) {
    let link = response.notification.request.content.userInfo["link"] as? String ?? ""
    if response.actionIdentifier == UNNotificationDefaultActionIdentifier {
      Ring.hand(response.notification, front: false, opened: true)
      if !link.isEmpty { ducktape_phone_link(link) }
    }
    completionHandler()
  }

  /// A notice while the app is in front. This phone's own banner (no push's): shown. A push: to Rust, and shown unless
  /// this phone banners it itself (the app in front by Rust's lifecycle, on the push's chain: the notifiers it runs).
  func userNotificationCenter(
    _ center: UNUserNotificationCenter, willPresent notification: UNNotification,
    withCompletionHandler completionHandler: @escaping (UNNotificationPresentationOptions) -> Void
  ) {
    let shown: UNNotificationPresentationOptions = [.banner, .list, .sound]
    guard notification.request.trigger is UNPushNotificationTrigger else { return completionHandler(shown) }
    completionHandler(Ring.hand(notification, front: true, opened: false) ? shown : [])
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

/// The OS's notice under the push server's `tag` out of its list (Rust: its row read or cleared, or its program saw
/// it): every notice under it (`Ring.takeDown`), this phone's own banners with them. Any thread.
@_cdecl("ducktape_phone_withdraw_notice")
public func ducktapePhoneWithdrawNotice(_ tag: UnsafePointer<CChar>) {
  Ring.takeDown(String(cString: tag))
}

/// This phone's own banner from Rust (`Banners::post`), its payload as JSON; 1 when it was handed to the OS. Any
/// thread.
@_cdecl("ducktape_phone_banner")
public func ducktapePhoneBanner(_ json: UnsafePointer<CChar>) -> UInt8 {
  let data = Data(bytes: json, count: strlen(json))
  guard let payload = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else {
    NSLog("ducktape ring: a banner's payload was not a JSON object")
    return 0
  }
  Ring.banner(payload)
  return 1
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
