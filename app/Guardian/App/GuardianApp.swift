import SwiftUI

@main
struct GuardianApp: App {
    @StateObject private var client = GuardianClient()
    @StateObject private var notifications = NotificationManager()
    @State private var previousPressure: PressureLevel = .clear

    var body: some Scene {
        WindowGroup {
            ContentView()
                .environmentObject(client)
                .onAppear {
                    notifications.setup()
                    client.startPolling()
                }
                .onReceive(client.$state) { newState in
                    if newState.pressure != previousPressure {
                        notifications.checkTransition(from: previousPressure, to: newState.pressure)
                        previousPressure = newState.pressure
                    }
                }
        }
        .defaultSize(width: 800, height: 600)

        MenuBarExtra {
            MenuBarView()
                .environmentObject(client)
        } label: {
            menuBarLabel
        }
    }

    @ViewBuilder
    private var menuBarLabel: some View {
        switch client.state.pressure {
        case .clear:
            Image(systemName: "shield.checkered")
                .symbolRenderingMode(.palette)
                .foregroundStyle(.green)
        case .strained:
            Image(systemName: "shield.checkered")
                .symbolRenderingMode(.palette)
                .foregroundStyle(.yellow)
        case .critical:
            Image(systemName: "shield.checkered")
                .symbolRenderingMode(.palette)
                .foregroundStyle(.red)
        }
    }
}
