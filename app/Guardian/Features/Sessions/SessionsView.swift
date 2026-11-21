import SwiftUI

struct SessionsView: View {
    @EnvironmentObject private var client: GuardianClient
    /// "all", or one agent: cursor | claude | codex.
    @State private var agent = "all"

    private var shown: [SessionRecord] {
        agent == "all" ? client.sessions : client.sessions.filter { $0.client == agent }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            HStack {
                Text("Agent Sessions")
                    .font(.title2.bold())
                Spacer()
                Picker("Agent", selection: $agent) {
                    Text("All agents").tag("all")
                    Text("Claude Code").tag("claude")
                    Text("Codex").tag("codex")
                    Text("Cursor").tag("cursor")
                }
                .pickerStyle(.segmented)
                .labelsHidden()
                .fixedSize()
                Button("Refresh") {
                    client.loadSessions()
                }
            }
            .padding(.horizontal)
            .padding(.top)

            if shown.isEmpty {
                VStack(spacing: 8) {
                    Image(systemName: "tray")
                        .font(.largeTitle)
                        .foregroundStyle(.secondary)
                    Text("No Sessions Recorded")
                        .font(.headline)
                    Text("Sessions appear here as Cursor, Claude Code and Codex run with Guardian's hooks installed. Check them with: ~/.guardian/guardian hooks status")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .multilineTextAlignment(.center)
                }
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                // In two groups: a table takes at most ten columns in one.
                Table(shown) {
                    Group {
                    TableColumn("Agent") { (session: SessionRecord) in
                        Text(session.clientLabel)
                            .font(.caption)
                    }
                    .width(min: 70, ideal: 85)

                    TableColumn("Project") { (session: SessionRecord) in
                        Text(session.project)
                            .font(.caption)
                            .lineLimit(1)
                            .truncationMode(.middle)
                            .help(session.cwd ?? "")
                    }
                    .width(min: 80, ideal: 130)

                    TableColumn("Now") { (session: SessionRecord) in
                        activityBadge(session.activity())
                    }
                    .width(min: 60, ideal: 75)

                    TableColumn("Last active") { (session: SessionRecord) in
                        Text(session.lastActiveText())
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                    .width(min: 70, ideal: 90)

                    TableColumn("Model") { (session: SessionRecord) in
                        Text(session.model)
                            .font(.caption)
                            .lineLimit(1)
                    }
                    .width(min: 80, ideal: 110)

                    TableColumn("Duration") { (session: SessionRecord) in
                        Text(session.durationFormatted)
                            .font(.caption.monospacedDigit())
                    }
                    .width(min: 60, ideal: 70)
                    }

                    Group {
                    TableColumn("Turns") { (session: SessionRecord) in
                        Text("\(session.turnCount ?? 0)")
                            .font(.caption.monospacedDigit())
                    }
                    .width(min: 40, ideal: 45)

                    TableColumn("Tools") { (session: SessionRecord) in
                        Text("\(session.toolCallCount ?? 0)")
                            .font(.caption.monospacedDigit())
                    }
                    .width(min: 40, ideal: 50)

                    TableColumn("Subagents") { (session: SessionRecord) in
                        Text("\(session.subagentCount ?? 0)")
                            .font(.caption.monospacedDigit())
                    }
                    .width(min: 50, ideal: 65)

                    TableColumn("Last turn") { (session: SessionRecord) in
                        statusBadge(session.status)
                    }
                    .width(min: 60, ideal: 80)

                    TableColumn("Pressure") { (session: SessionRecord) in
                        pressureBadge(session.pressureAvg ?? session.pressureAtStart)
                    }
                    .width(min: 60, ideal: 80)

                    TableColumn("Cost") { (session: SessionRecord) in
                        if let cost = session.estimatedCostUsd {
                            Text(String(format: "$%.3f", cost))
                                .font(.caption.monospacedDigit())
                        } else {
                            Text("--")
                                .font(.caption)
                        }
                    }
                    .width(min: 50, ideal: 60)

                    TableColumn("ROI") { (session: SessionRecord) in
                        roiBar(session.roiScore)
                    }
                    .width(min: 80, ideal: 100)
                    }
                }
            }
        }
        .onAppear {
            client.loadSessions()
        }
    }

    @ViewBuilder
    private func activityBadge(_ activity: SessionRecord.Activity) -> some View {
        let color: Color = {
            switch activity {
            case .working: return .blue
            case .idle: return .green
            case .dormant, .ended: return .gray
            }
        }()
        Text(activity.rawValue.capitalized)
            .font(.caption)
            .padding(.horizontal, 6)
            .padding(.vertical, 2)
            .background(color.opacity(0.15))
            .foregroundStyle(color)
            .clipShape(Capsule())
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
