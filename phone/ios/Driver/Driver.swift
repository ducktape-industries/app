// The XCUITest driver the mac service (core ops/mac/mac.py) taps system UI with: the photo picker, permission alerts, anything the app's
// own AX door can't see. It asks the Mac for each command at DUCKTAPE_DRIVER (xcodebuild hands the runner TEST_RUNNER_DUCKTAPE_DRIVER without its
// prefix) and answers it. Built for testing only (`make -C phone ios-driver`), never archived.
import XCTest

final class Driver: XCTestCase {
    func testDrive() throws {
        guard let given = ProcessInfo.processInfo.environment["DUCKTAPE_DRIVER"], let base = URL(string: given) else {
            XCTFail("DUCKTAPE_DRIVER names no URL to ask for commands")
            return
        }
        var after = 0
        while true {
            let asked = try next(base, after: after)
            guard let seq = asked["seq"] as? Int, let command = asked["command"] as? [String: Any] else {
                XCTFail("the Mac sent no command: \(asked)")
                return
            }
            if command["end"] as? Bool == true {
                return
            }
            try done(base, answer(seq, command))
            after = seq
        }
    }

    private func next(_ base: URL, after: Int) throws -> [String: Any] {
        var components = URLComponents(url: base.appendingPathComponent("next"), resolvingAgainstBaseURL: false)
        components?.queryItems = [URLQueryItem(name: "after", value: String(after))]
        guard let url = components?.url else {
            throw URLError(.badURL)
        }
        while true {
            do {
                let data = try fetched(URLRequest(url: url))
                return try JSONSerialization.jsonObject(with: data) as? [String: Any] ?? [:]
            } catch let error as URLError where error.code == .timedOut {
                continue
            }
        }
    }

    private func answer(_ seq: Int, _ command: [String: Any]) -> [String: Any] {
        guard let label = command["tap"] as? String, let bundle = command["app"] as? String else {
            return ["seq": seq, "ok": false, "error": "the driver takes tap, with app"]
        }
        let milliseconds = (command["deadline_ms"] as? NSNumber)?.doubleValue ?? 0
        let app = XCUIApplication(bundleIdentifier: bundle)
        let named = app.descendants(matching: .any).matching(NSPredicate(format: "label CONTAINS %@", label))
        let control = named.matching(NSPredicate(format: "elementType != %d", XCUIElement.ElementType.other.rawValue)).firstMatch
        guard named.firstMatch.waitForExistence(timeout: milliseconds / 1000) else {
            let seen = labels(app)
            print("driver: no \(label) in \(bundle); its labels: \(seen)")
            return ["seq": seq, "ok": false, "error": "no element of \(bundle) labelled \(label) appeared within \(Int(milliseconds)) ms; its labels: \(seen)"]
        }
        let element = control.exists ? control : named.firstMatch
        print("driver: tapping \(element.elementType.rawValue) \"\(element.label)\" in \(bundle)")
        guard element.isHittable || !element.frame.isEmpty else {
            return ["seq": seq, "ok": false, "error": "the element of \(bundle) labelled \(label) can't be tapped; the app: \(labels(app))"]
        }
        element.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.5)).tap()
        return ["seq": seq, "ok": true]
    }

    private func labels(_ app: XCUIApplication) -> String {
        app.debugDescription
    }

    private func done(_ base: URL, _ result: [String: Any]) throws {
        var request = URLRequest(url: base.appendingPathComponent("done"))
        request.httpMethod = "POST"
        request.setValue("application/json", forHTTPHeaderField: "content-type")
        request.httpBody = try JSONSerialization.data(withJSONObject: result)
        _ = try fetched(request)
    }

    private func fetched(_ request: URLRequest) throws -> Data {
        let answered = DispatchSemaphore(value: 0)
        var result: Result<Data, Error> = .failure(URLError(.unknown))
        URLSession.shared.dataTask(with: request) { data, _, error in
            result = error.map { .failure($0) } ?? .success(data ?? Data())
            answered.signal()
        }.resume()
        answered.wait()
        return try result.get()
    }
}
