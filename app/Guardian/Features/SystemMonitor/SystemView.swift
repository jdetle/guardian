import SwiftUI
import Charts

struct SystemView: View {
    @EnvironmentObject private var client: GuardianClient

    var body: some View {
        ScrollView {
            VStack(spacing: 20) {
                pressureBanner
                metricsGrid
                cpuChart
                processInfo
            }
            .padding()
        }
    }

    private var pressureBanner: some View {
        HStack {
            Image(systemName: pressureIcon)
                .font(.title)
                .foregroundStyle(pressureColor)
            VStack(alignment: .leading) {
                Text("System Pressure: \(client.state.pressure.rawValue.capitalized)")
                    .font(.title2.bold())
                Text("Sampled at \(client.state.sampledAt)")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            Spacer()
        }
        .padding()
        .background(pressureColor.opacity(0.1))
        .clipShape(RoundedRectangle(cornerRadius: 12))
    }

    private var metricsGrid: some View {
        LazyVGrid(columns: [
            GridItem(.flexible()),
            GridItem(.flexible()),
            GridItem(.flexible()),
            GridItem(.flexible()),
        ], spacing: 16) {
            GaugeCard(title: "CPU", value: client.state.cpuPercent, maxValue: 100, unit: "%", color: gaugeColor(client.state.cpuPercent / 100))
            GaugeCard(title: "Memory Free", value: client.state.memoryAvailableGb, maxValue: client.state.memoryTotalGb, unit: "GB", color: gaugeColor(1 - client.state.memoryAvailableGb / max(client.state.memoryTotalGb, 1)))
            GaugeCard(title: "Swap Used", value: client.state.swapUsedPercent, maxValue: 100, unit: "%", color: gaugeColor(client.state.swapUsedPercent / 100))
            GaugeCard(title: "Docker CPU", value: client.state.docker.totalCpuPercent, maxValue: 800, unit: "%", color: .blue)
        }
    }

    private var cpuChart: some View {
        VStack(alignment: .leading) {
            Text("CPU History (last 5 min)")
                .font(.headline)

            if client.history.count > 2 {
                Chart(Array(client.history.enumerated()), id: \.offset) { index, sample in
                    AreaMark(
                        x: .value("Time", index),
                        y: .value("CPU %", sample.cpuPercent)
                    )
                    .foregroundStyle(
                        .linearGradient(
                            colors: [.blue.opacity(0.3), .blue.opacity(0.05)],
                            startPoint: .top,
                            endPoint: .bottom
                        )
                    )
                    LineMark(
                        x: .value("Time", index),
                        y: .value("CPU %", sample.cpuPercent)
                    )
                    .foregroundStyle(.blue)
                }
                .chartYScale(domain: 0...100)
                .chartXAxis(.hidden)
                .frame(height: 150)
            } else {
                Text("Collecting data...")
                    .foregroundStyle(.secondary)
                    .frame(height: 150)
            }
        }
        .padding()
        .background(Color(nsColor: .controlBackgroundColor))
        .clipShape(RoundedRectangle(cornerRadius: 12))
    }

    private var processInfo: some View {
        HStack(spacing: 20) {
            InfoCard(title: "Processes", value: "\(client.state.processCount)", subtitle: "of \(client.state.maxProcPerUid) max")
            InfoCard(title: "Docker Containers", value: "\(client.state.docker.runningContainers)", subtitle: "\(client.state.docker.totalMemoryMb) MB used")
            InfoCard(title: "Cursor Processes", value: "\(client.state.cursor.processCount)", subtitle: "\(client.state.cursor.activeSessions) sessions")
            InfoCard(title: "Thermal", value: client.state.thermalState.capitalized, subtitle: "")
        }
    }

    private var pressureIcon: String {
        switch client.state.pressure {
        case .clear: return "checkmark.shield"
        case .strained: return "exclamationmark.shield"
        case .critical: return "xmark.shield"
        }
    }

    private var pressureColor: Color {
        switch client.state.pressure {
        case .clear: return .green
        case .strained: return .yellow
        case .critical: return .red
        }
    }

    private func gaugeColor(_ ratio: Double) -> Color {
        if ratio > 0.9 { return .red }
        if ratio > 0.7 { return .orange }
        return .green
    }
}

struct GaugeCard: View {
    let title: String
    let value: Double
    let maxValue: Double
    let unit: String
    let color: Color

    var body: some View {
        VStack(spacing: 8) {
            Gauge(value: value, in: 0...maxValue) {
                Text(title)
                    .font(.caption2)
            } currentValueLabel: {
                Text(String(format: "%.1f%@", value, unit))
                    .font(.caption.monospacedDigit().bold())
            }
            .gaugeStyle(.accessoryCircular)
            .tint(color)

            Text(title)
                .font(.caption)
                .foregroundStyle(.secondary)
        }
        .padding()
        .background(Color(nsColor: .controlBackgroundColor))
        .clipShape(RoundedRectangle(cornerRadius: 12))
    }
}

struct InfoCard: View {
    let title: String
    let value: String
    let subtitle: String

    var body: some View {
        VStack(spacing: 4) {
            Text(value)
                .font(.title2.monospacedDigit().bold())
            Text(title)
                .font(.caption)
            if !subtitle.isEmpty {
                Text(subtitle)
                    .font(.caption2)
                    .foregroundStyle(.secondary)
            }
        }
        .frame(maxWidth: .infinity)
        .padding()
        .background(Color(nsColor: .controlBackgroundColor))
        .clipShape(RoundedRectangle(cornerRadius: 12))
    }
}
