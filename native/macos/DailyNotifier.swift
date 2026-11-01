import AppKit
import UserNotifications

let bundleID = "com.ctdoteam1110.ctdozlite.notifications"
struct Payload: Decodable {
    let title: String
    let body: String
    let id: String
    let sound: Bool
    func validate() -> Bool {
        // Never accept chat content, names, or other caller-controlled notification text.
        title == "Zalo" && body == "You have a new message." && id.count == 64 && id.utf8.allSatisfy { (48...57).contains($0) || (97...102).contains($0) }
    }
}
func statusName(_ status: UNAuthorizationStatus) -> String {
    switch status {
    case .notDetermined: return "notDetermined"
    case .denied: return "denied"
    case .authorized: return "authorized"
    case .provisional: return "provisional"
    @unknown default: return "unavailable"
    }
}
func authorized(_ status: UNAuthorizationStatus) -> Bool { status == .authorized || status == .provisional }

final class NotificationDelegate: NSObject, UNUserNotificationCenterDelegate {
    func userNotificationCenter(_ center: UNUserNotificationCenter, didReceive response: UNNotificationResponse, withCompletionHandler completionHandler: @escaping () -> Void) {
        guard response.actionIdentifier == UNNotificationDefaultActionIdentifier else { completionHandler(); return }
        // A notification click may start the parent, but must not steal foreground focus.
        let helper = Bundle.main.bundleURL
        let parent = helper.deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
        guard parent.pathExtension == "app", parent != helper else { completionHandler(); exit(0) }
        let configuration = NSWorkspace.OpenConfiguration()
        configuration.activates = false
        configuration.addsToRecentItems = false
        NSWorkspace.shared.openApplication(at: parent, configuration: configuration) { _, _ in
            completionHandler()
            exit(0)
        }
    }
}
let args = Array(CommandLine.arguments.dropFirst())
if args == ["--self-test"] {
    guard Bundle.main.bundleIdentifier == bundleID, Bundle.main.bundleURL.pathExtension == "app" else { exit(1) }
    let valid = Payload(title: "Zalo", body: "You have a new message.", id: String(repeating: "a", count: 64), sound: false)
    let invalid = Payload(title: "Private Name", body: "Chat content", id: "raw-message", sound: true)
    guard valid.validate(), !invalid.validate(), authorized(.authorized), !authorized(.denied), statusName(.notDetermined) == "notDetermined" else { exit(1) }
    print("SELF_TEST: bundle identity, generic-only payload, opaque identity, and authorization gates PASS")
    exit(0)
}
guard Bundle.main.bundleIdentifier == bundleID, Bundle.main.bundleURL.pathExtension == "app" else {
    fputs("Notification helper bundle identity required\n", stderr); exit(2)
}
guard args.isEmpty || args == ["--status"] || args == ["--request"] || args == ["--notify"] else { exit(2) }
let app = NSApplication.shared
app.setActivationPolicy(.prohibited)
let center = UNUserNotificationCenter.current()
let delegate = NotificationDelegate()
center.delegate = delegate
var finished = false
var resultCode: Int32 = 0
func finish(_ output: String, code: Int32 = 0) {
    DispatchQueue.main.async { print(output); resultCode = code; finished = true }
}
if args == ["--status"] {
    center.getNotificationSettings { settings in finish(statusName(settings.authorizationStatus)) }
} else if args == ["--request"] {
    // Only the explicit Rust user-action API launches this mode.
    center.requestAuthorization(options: [.alert, .sound]) { _, error in
        if error != nil { finish("unavailable", code: 3); return }
        center.getNotificationSettings { settings in finish(statusName(settings.authorizationStatus)) }
    }
} else if args == ["--notify"] {
    let data = FileHandle.standardInput.readData(ofLength: 4097)
    guard data.count <= 4096, let payload = try? JSONDecoder().decode(Payload.self, from: data), payload.validate() else { exit(2) }
    center.getNotificationSettings { settings in
        guard authorized(settings.authorizationStatus) else { finish("notAuthorized"); return }
        let content = UNMutableNotificationContent()
        content.title = payload.title
        content.body = payload.body
        if payload.sound { content.sound = .default }
        let request = UNNotificationRequest(identifier: payload.id, content: content, trigger: nil)
        center.add(request) { error in finish(error == nil ? "submitted" : "failed", code: error == nil ? 0 : 3) }
    }
} else {
    // LaunchServices starts this bundle on notification click. No window or activation occurs.
    DispatchQueue.main.asyncAfter(deadline: .now() + 15) { NSApp.terminate(nil) }
    app.run()
    exit(0)
}
let timeout = Date().addingTimeInterval(args == ["--request"] ? 115 : 20)
while !finished && Date() < timeout { RunLoop.main.run(until: Date().addingTimeInterval(0.05)) }
exit(finished ? resultCode : 4)
