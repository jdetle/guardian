import SwiftUI

struct DockerView: View {
    @EnvironmentObject private var client: GuardianClient
    @State private var containers: [ContainerInfo] = []
    @State private var isRefreshing = false

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            HStack {
                Text("Docker Containers")
                    .font(.title2.bold())
                Spacer()
                dockerSummary
                Button("Refresh") {
                    refreshContainers()
                }
                .disabled(isRefreshing)
            }
            .padding(.horizontal)
            .padding(.top)

            if containers.isEmpty {
                VStack(spacing: 8) {
                    Image(systemName: "shippingbox")
                        .font(.largeTitle)
                        .foregroundStyle(.secondary)
                    Text("No Running Containers")
                        .font(.headline)
                    Text("Start Docker Compose to see container metrics here.")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                Table(containers) {
                    TableColumn("Name") { container in
                        Text(container.name)
                            .font(.body.bold())
                    }
                    .width(min: 150, ideal: 200)

                    TableColumn("Status") { container in
                        HStack(spacing: 4) {
                            Circle()
                                .fill(container.state == "running" ? .green : .orange)
                                .frame(width: 6, height: 6)
                            Text(container.state.capitalized)
                                .font(.caption)
                        }
                    }
                    .width(min: 80, ideal: 100)

                    TableColumn("Actions") { container in
                        HStack(spacing: 4) {
                            if container.state == "running" {
                                Button(action: { pauseContainer(container.name) }) {
                                    Image(systemName: "pause.circle")
                                }
                                .buttonStyle(.borderless)
                                .help("Pause container")
                            } else if container.state == "paused" {
                                Button(action: { unpauseContainer(container.name) }) {
                                    Image(systemName: "play.circle")
                                }
                                .buttonStyle(.borderless)
                                .help("Unpause container")
                            }
                        }
                    }
                    .width(min: 50, ideal: 60)
                }
            }
        }
        .onAppear { refreshContainers() }
    }

    private var dockerSummary: some View {
        HStack(spacing: 12) {
            Label("\(client.state.docker.runningContainers) running", systemImage: "shippingbox")
            Label(String(format: "CPU: %.1f%%", client.state.docker.totalCpuPercent), systemImage: "cpu")
            Label("\(client.state.docker.totalMemoryMb) MB", systemImage: "memorychip")
        }
        .font(.caption)
        .foregroundStyle(.secondary)
    }

    private func refreshContainers() {
        isRefreshing = true
        DispatchQueue.global().async {
            let output = shellSync("docker ps --format '{{.Names}}\\t{{.State}}\\t{{.Status}}'")
            let parsed = output.split(separator: "\n").compactMap { line -> ContainerInfo? in
                let parts = line.split(separator: "\t", maxSplits: 2)
                guard parts.count >= 2 else { return nil }
                return ContainerInfo(
                    name: String(parts[0]),
                    state: String(parts[1])
                )
            }

            DispatchQueue.main.async {
                containers = parsed
                isRefreshing = false
            }
        }
    }

    private func pauseContainer(_ name: String) {
        DispatchQueue.global().async {
            _ = shellSync("docker pause \(name)")
            DispatchQueue.main.async { refreshContainers() }
        }
    }

    private func unpauseContainer(_ name: String) {
        DispatchQueue.global().async {
            _ = shellSync("docker unpause \(name)")
            DispatchQueue.main.async { refreshContainers() }
        }
    }

    private func shellSync(_ command: String) -> String {
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

struct ContainerInfo: Identifiable {
    var id: String { name }
    let name: String
    let state: String
}
