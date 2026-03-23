import SwiftUI
import Charts

struct MenuBarView: View {
    @EnvironmentObject private var client: GuardianClient

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack {
                pressureIndicator
                Text("System Guardian")
                    .font(.headline)
                Spacer()
                Text(client.state.pressure.rawValue.capitalized)
                    .font(.caption)
                    .padding(.horizontal, 6)
                    .padding(.vertical, 2)
                    .background(pressureColor.opacity(0.2))
                    .foregroundStyle(pressureColor)
                    .clipShape(Capsule())
            }

            Divider()

            metricRow(label: "CPU", value: String(format: "%.1f%%", client.state.cpuPercent), progress: client.state.cpuPercent / 100)
            metricRow(label: "Memory Free", value: String(format: "%.1f GB", client.state.memoryAvailableGb), progress: 1.0 - (client.state.memoryAvailableGb / max(client.state.memoryTotalGb, 1)))
            metricRow(label: "Swap", value: String(format: "%.1f%%", client.state.swapUsedPercent), progress: client.state.swapUsedPercent / 100)
            metricRow(label: "Thermal", value: client.state.thermalState.capitalized, progress: nil)

            if client.state.docker.runningContainers > 0 {
                Divider()
                HStack {
                    Image(systemName: "shippingbox")
                    Text("\(client.state.docker.runningContainers) containers")
                    Spacer()
                    Text(String(format: "CPU: %.1f%%", client.state.docker.totalCpuPercent))
                        .foregroundStyle(.secondary)
                }
                .font(.caption)
            }

            if client.state.cursor.processCount > 0 {
                Divider()
                HStack {
                    Image(systemName: "cursorarrow.rays")
                    Text("\(client.state.cursor.processCount) Cursor processes")
                    Spacer()
                }
                .font(.caption)
            }

            if client.history.count > 5 {
                Divider()
                cpuSparkline
                    .frame(height: 40)
            }

            Divider()

            Button("Quit Guardian") {
                NSApplication.shared.terminate(nil)
            }
        }
        .padding()
        .frame(width: 300)
    }

    @ViewBuilder
    private func metricRow(label: String, value: String, progress: Double?) -> some View {
        HStack {
            Text(label)
                .font(.caption)
                .frame(width: 80, alignment: .leading)
            if let progress {
                ProgressView(value: min(max(progress, 0), 1))
                    .tint(progressColor(progress))
            }
            Spacer()
            Text(value)
                .font(.caption.monospacedDigit())
                .foregroundStyle(.secondary)
        }
    }

    private var cpuSparkline: some View {
        Chart(Array(client.history.enumerated()), id: \.offset) { index, sample in
            LineMark(
                x: .value("Time", index),
                y: .value("CPU", sample.cpuPercent)
            )
            .foregroundStyle(pressureColor.gradient)
        }
        .chartYScale(domain: 0...100)
        .chartXAxis(.hidden)
        .chartYAxis(.hidden)
    }

    private var pressureIndicator: some View {
        Circle()
            .fill(pressureColor)
            .frame(width: 8, height: 8)
    }

    private var pressureColor: Color {
        switch client.state.pressure {
        case .clear: return .green
        case .strained: return .yellow
        case .critical: return .red
        }
    }

    private func progressColor(_ value: Double) -> Color {
        if value > 0.9 { return .red }
        if value > 0.7 { return .yellow }
        return .green
    }
}
