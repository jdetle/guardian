import AppKit
import SwiftUI

/// The full dashboard (System / Sessions / Docker / Settings), opened from the menu.
/// It is an ordinary AppKit window rather than a SwiftUI `WindowGroup` scene, because a
/// scene would open at every launch and the app is meant to start silently in the menu bar.
@MainActor
final class DashboardWindow {
    static let shared = DashboardWindow()

    private var window: NSWindow?

    func show(client: GuardianClient) {
        if window == nil {
            let hosting = NSHostingController(rootView: ContentView().environmentObject(client))
            let created = NSWindow(contentViewController: hosting)
            created.title = "Guardian"
            created.styleMask = [.titled, .closable, .miniaturizable, .resizable]
            created.setContentSize(NSSize(width: 800, height: 600))
            created.isReleasedWhenClosed = false
            created.center()
            window = created
        }
        NSApp.activate(ignoringOtherApps: true)
        window?.makeKeyAndOrderFront(nil)
    }
}
