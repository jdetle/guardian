import SwiftUI

struct SettingsView: View {
    @State private var configText: String = ""
    @State private var isSaving = false
    @State private var statusMessage: String?

    private let configPath: String = {
        FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent(".guardian/config.toml")
            .path
    }()

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            HStack {
                Text("Guardian Configuration")
                    .font(.title2.bold())
                Spacer()
                if let msg = statusMessage {
                    Text(msg)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
                Button("Save") {
                    saveConfig()
                }
                .disabled(isSaving)
                Button("Reload") {
                    loadConfig()
                }
            }
            .padding(.horizontal)
            .padding(.top)

            Text(configPath)
                .font(.caption)
                .foregroundStyle(.secondary)
                .padding(.horizontal)

            TextEditor(text: $configText)
                .font(.system(.body, design: .monospaced))
                .padding(4)
                .background(.background.secondary)
                .clipShape(RoundedRectangle(cornerRadius: 8))
                .padding(.horizontal)
                .padding(.bottom)

            VStack(alignment: .leading, spacing: 8) {
                Text("Service Management")
                    .font(.headline)
                    .padding(.horizontal)

                HStack(spacing: 12) {
                    Button("Restart Daemon") {
                        restartDaemon()
                    }
                    Button("Stop Daemon") {
                        stopDaemon()
                    }
                    Button("Open Logs") {
                        openLogs()
                    }
                }
                .padding(.horizontal)
                .padding(.bottom)
            }
        }
        .onAppear { loadConfig() }
    }

    private func loadConfig() {
        do {
            configText = try String(contentsOfFile: configPath, encoding: .utf8)
            statusMessage = "Loaded"
        } catch {
            configText = """
            # Guardian config not found.
            # Run the install script to create a default config.
            """
            statusMessage = "Config not found"
        }
    }

    private func saveConfig() {
        isSaving = true
        do {
            try configText.write(toFile: configPath, atomically: true, encoding: .utf8)
            statusMessage = "Saved"
            restartDaemon()
        } catch {
            statusMessage = "Save failed: \(error.localizedDescription)"
        }
        isSaving = false
    }

    private func restartDaemon() {
        shell("launchctl unload ~/Library/LaunchAgents/com.guardian.guardiand.plist 2>/dev/null; launchctl load ~/Library/LaunchAgents/com.guardian.guardiand.plist")
        statusMessage = "Daemon restarted"
    }

    private func stopDaemon() {
        shell("launchctl unload ~/Library/LaunchAgents/com.guardian.guardiand.plist 2>/dev/null")
        statusMessage = "Daemon stopped"
    }

    private func openLogs() {
        let logPath = FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent(".guardian/guardiand.stderr.log")
        NSWorkspace.shared.open(logPath)
    }

    @discardableResult
    private func shell(_ command: String) -> String {
        let process = Process()
        let pipe = Pipe()
        process.executableURL = URL(fileURLWithPath: "/bin/bash")
        process.arguments = ["-c", command]
        process.standardOutput = pipe
        process.standardError = FileHandle.nullDevice
        do {
            try process.run()
            process.waitUntilExit()
        } catch {
            return ""
        }
        return String(data: pipe.fileHandleForReading.readDataToEndOfFile(), encoding: .utf8) ?? ""
    }
}
