import AppKit
import Combine
import SwiftUI

/// Guardian lives in the menu bar. There is no Dock icon and no window at launch; the
/// dashboard window opens on request from the menu.
@main
struct GuardianApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var appDelegate
    @StateObject private var client = GuardianClient.shared

    var body: some Scene {
        MenuBarExtra {
            MenuBarView()
                .environmentObject(client)
        } label: {
            MenuBarLabel(client: client)
        }
        .menuBarExtraStyle(.window)
    }
}

/// The menu bar item. The symbol's shape changes with the level, not just its color, so it
/// still reads when the system draws menu bar items in a single tint.
struct MenuBarLabel: View {
    @ObservedObject var client: GuardianClient

    var body: some View {
        HStack(spacing: 3) {
            Image(systemName: symbol)
            if client.state.disk.level == "critical" && !client.isStale {
                Text("Disk \(Int(client.state.disk.usedPercent.rounded()))%")
            }
        }
        .accessibilityLabel(accessibilityText)
    }

    private var symbol: String {
        if client.isStale { return "shield.slash" }
        switch client.state.pressure {
        case .clear: return "shield"
        case .strained: return "shield.lefthalf.filled"
        case .critical: return "exclamationmark.shield.fill"
        }
    }

    private var accessibilityText: String {
        client.isStale ? "Guardian: daemon not reporting" : "Guardian: \(client.state.pressure.rawValue)"
    }
}

final class AppDelegate: NSObject, NSApplicationDelegate {
    private let notifications = NotificationManager()
    private var observers = Set<AnyCancellable>()

    func applicationDidFinishLaunching(_ notification: Notification) {
        let client = GuardianClient.shared
        if let path = PanelSnapshot.requestedPath {
            let ok = PanelSnapshot.render(to: path, client: client)
            exit(ok ? 0 : 1)
        }
        notifications.setup()
        client.startPolling()
        client.$state
            .sink { [weak self] state in self?.notifications.observe(state) }
            .store(in: &observers)
    }
}
