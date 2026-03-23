import SwiftUI

struct ContentView: View {
    @EnvironmentObject private var client: GuardianClient

    var body: some View {
        TabView {
            SystemView()
                .tabItem {
                    Label("System", systemImage: "gauge.with.dots.needle.33percent")
                }
            SessionsView()
                .tabItem {
                    Label("Sessions", systemImage: "list.bullet.rectangle")
                }
            DockerView()
                .tabItem {
                    Label("Docker", systemImage: "shippingbox")
                }
            SettingsView()
                .tabItem {
                    Label("Settings", systemImage: "gear")
                }
        }
        .frame(minWidth: 700, minHeight: 500)
    }
}
