import Foundation
import UserNotifications

final class NotificationManager: ObservableObject {
    private var lastNotifiedPressure: PressureLevel = .clear

    func setup() {
        UNUserNotificationCenter.current().requestAuthorization(options: [.alert, .sound]) { _, _ in }
    }

    func checkTransition(from old: PressureLevel, to new: PressureLevel) {
        guard old != new, new != lastNotifiedPressure else { return }
        lastNotifiedPressure = new

        let content = UNMutableNotificationContent()
        switch new {
        case .critical:
            content.title = "System Guardian"
            content.body = "Resources critical — agent throttling active. Docker containers may be paused."
            content.sound = .default
        case .strained:
            content.title = "System Guardian"
            content.body = "Resources under moderate load. Parallel agent work limited."
            content.sound = nil
        case .clear:
            content.title = "System Guardian"
            content.body = "Resources recovered — throttling lifted."
            content.sound = nil
        }

        let request = UNNotificationRequest(
            identifier: "guardian-pressure-\(new.rawValue)",
            content: content,
            trigger: nil
        )
        UNUserNotificationCenter.current().add(request)
    }
}
