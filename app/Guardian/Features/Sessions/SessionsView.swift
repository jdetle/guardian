import SwiftUI

struct SessionsView: View {
    @EnvironmentObject private var client: GuardianClient

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            HStack {
                Text("Agent Sessions")
                    .font(.title2.bold())
                Spacer()
                Button("Refresh") {
                    client.loadSessions()
                }
            }
            .padding(.horizontal)
            .padding(.top)

            if client.sessions.isEmpty {
                VStack(spacing: 8) {
                    Image(systemName: "tray")
                        .font(.largeTitle)
                        .foregroundStyle(.secondary)
                    Text("No Sessions Recorded")
                        .font(.headline)
                    Text("Sessions will appear here as Cursor agents run with Guardian hooks installed.")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                Table(client.sessions) {
                    TableColumn("Model") { session in
                        Text(session.model)
                            .font(.caption)
                    }
                    .width(min: 80, ideal: 100)

                    TableColumn("Duration") { session in
                        Text(session.durationFormatted)
                            .font(.caption.monospacedDigit())
                    }
                    .width(min: 60, ideal: 70)

                    TableColumn("Tools") { session in
                        Text("\(session.toolCallCount ?? 0)")
                            .font(.caption.monospacedDigit())
                    }
                    .width(min: 40, ideal: 50)

                    TableColumn("Status") { session in
                        statusBadge(session.status)
                    }
                    .width(min: 60, ideal: 80)

                    TableColumn("Pressure") { session in
                        pressureBadge(session.pressureAvg ?? session.pressureAtStart)
                    }
                    .width(min: 60, ideal: 80)

                    TableColumn("Cost") { session in
                        if let cost = session.estimatedCostUsd {
                            Text(String(format: "$%.3f", cost))
                                .font(.caption.monospacedDigit())
                        } else {
                            Text("--")
                                .font(.caption)
                        }
                    }
                    .width(min: 50, ideal: 60)

                    TableColumn("ROI") { session in
                        roiBar(session.roiScore)
                    }
                    .width(min: 80, ideal: 100)

                    TableColumn("Started") { session in
                        Text(session.startedAt)
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                    .width(min: 120, ideal: 150)
                }
            }
        }
    }

    @ViewBuilder
    private func statusBadge(_ status: String?) -> some View {
        let text = status ?? "unknown"
        let color: Color = {
            switch text {
            case "completed": return .green
            case "error": return .red
            case "aborted": return .orange
            default: return .gray
            }
        }()
        Text(text.capitalized)
            .font(.caption)
            .padding(.horizontal, 6)
            .padding(.vertical, 2)
            .background(color.opacity(0.15))
            .foregroundStyle(color)
            .clipShape(Capsule())
    }

    @ViewBuilder
    private func pressureBadge(_ pressure: String?) -> some View {
        let text = pressure ?? "unknown"
        let color: Color = {
            switch text {
            case "clear": return .green
            case "strained": return .yellow
            case "critical": return .red
            default: return .gray
            }
        }()
        Text(text.capitalized)
            .font(.caption)
            .padding(.horizontal, 6)
            .padding(.vertical, 2)
            .background(color.opacity(0.15))
            .foregroundStyle(color)
            .clipShape(Capsule())
    }

    @ViewBuilder
    private func roiBar(_ score: Double?) -> some View {
        if let score {
            HStack(spacing: 4) {
                ProgressView(value: min(max(score, 0), 100), total: 100)
                    .tint(roiColor(score))
                Text("\(Int(score))")
                    .font(.caption.monospacedDigit())
                    .foregroundStyle(.secondary)
            }
        } else {
            Text("--")
                .font(.caption)
                .foregroundStyle(.secondary)
        }
    }

    private func roiColor(_ score: Double) -> Color {
        if score >= 70 { return .green }
        if score >= 40 { return .yellow }
        return .red
    }
}
