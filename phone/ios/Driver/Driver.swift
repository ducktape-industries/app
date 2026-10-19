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

    // The Mac holds a command until it is done, so asking again after a lost or timed-out request gets the same one.
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
        let element = app.descendants(matching: .any).matching(NSPredicate(format: "label CONTAINS %@", label)).firstMatch
        guard element.waitForExistence(timeout: milliseconds / 1000) else {
            return ["seq": seq, "ok": false, "error": "no element of \(bundle) labelled \(label) appeared within \(Int(milliseconds)) ms"]
        }
        element.tap()
        return ["seq": seq, "ok": true]
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
