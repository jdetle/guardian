import Foundation
import UserNotifications

/// Tells the person, not just the agent, when something needs them.
///
/// Only two events notify: the machine entering `critical` (rare by design since 0.8) and the
/// disk crossing into `critical`. Strained and recovery are visible in the menu bar and do
/// not interrupt.
final class NotificationManager {
    private var lastPressure: PressureLevel?
    private var lastDiskLevel: String?

    /// Notifications need a real app bundle; a bare `swift run` binary has none.
    private var available: Bool { Bundle.main.bundleIdentifier != nil }

    func setup() {
        guard available else { return }
        UNUserNotificationCenter.current().requestAuthorization(options: [.alert, .sound]) { _, _ in }
    }

    func observe(_ state: GuardianStateData) {
        // No sample yet: nothing to compare against.
        guard !state.sampledAt.isEmpty else { return }
        defer {
            lastPressure = state.pressure
            lastDiskLevel = state.disk.level
        }
        // The first sample after launch sets the baseline without notifying.
        guard let lastPressure, let lastDiskLevel else { return }

        if state.pressure == .critical && lastPressure != .critical {
            let why = state.pressureReasons.prefix(2).joined(separator: "; ")
            post(
                id: "guardian-pressure-critical",
                title: "Guardian: load is critical",
                body: why.isEmpty ? "Hold off on heavy work until it clears." : "\(why). Hold off on heavy work until it clears."
            )
        }
        if state.disk.level == "critical" && lastDiskLevel != "critical" {
            post(
                id: "guardian-disk-critical",
                title: "Guardian: disk is nearly full",
                body: String(
                    format: "%.1f GB free (%.0f%% used). Open Guardian in the menu bar and choose Find space to free.",
                    state.disk.availableGb, state.disk.usedPercent
                )
            )
        }
    }

    private func post(id: String, title: String, body: String) {
        guard available else { return }
        let content = UNMutableNotificationContent()
        content.title = title
        content.body = body
        content.sound = .default
        UNUserNotificationCenter.current().add(
            UNNotificationRequest(identifier: id, content: content, trigger: nil)
        )
    }
}
