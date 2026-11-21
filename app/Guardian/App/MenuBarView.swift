import AppKit
import SwiftUI

/// The panel that opens from the menu bar item: what the level is and why, what is switched
/// off, how today has gone, and where disk space can be found.
struct MenuBarView: View {
    @EnvironmentObject private var client: GuardianClient
    @State private var copiedPath: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            header

            if client.isStale {
                notice(
                    "guardiand is not reporting. The numbers below are old and the gates are off.",
                    detail: "Restart it: bash scripts/install.sh",
                    color: .orange
                )
            }
            if let problem = client.state.configProblems.first, !client.isStale {
                notice(
                    "Config problem: \(problem)",
                    detail: "Details: ~/.guardian/guardian config check",
                    color: .orange
                )
            }

            Divider()
            promptControls

            Divider()
            metrics

            if let today = client.today, today.totalSecs > 0 {
                Divider()
                todayRow(today)
            }

            Divider()
            diskSection

            if !client.queueEntries.isEmpty {
                Divider()
                queueSection
            }

            Divider()
            HStack {
                Button("Open dashboard") { DashboardWindow.shared.show(client: client) }
                Spacer()
                Button("Quit") { NSApplication.shared.terminate(nil) }
            }
        }
        .padding()
        .frame(width: 360)
        .onAppear {
            client.loadPromptControls()
            client.loadStats()
        }
    }

    // MARK: - Header

    private var header: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack {
                Circle()
                    .fill(pressureColor)
                    .frame(width: 8, height: 8)
                Text("Guardian")
                    .font(.headline)
                Spacer()
                Text(client.isStale ? "No data" : client.state.pressure.rawValue.capitalized)
                    .font(.caption)
                    .padding(.horizontal, 6)
                    .padding(.vertical, 2)
                    .background(pressureColor.opacity(0.2))
                    .foregroundStyle(pressureColor)
                    .clipShape(Capsule())
            }
            if !client.isStale {
                Text(whyText)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
    }

    private var whyText: String {
        let reasons = client.state.pressureReasons
        if reasons.isEmpty {
            return client.state.pressure == .clear ? "Nothing is under pressure." : "No cause reported."
        }
        return reasons.prefix(3).joined(separator: " · ")
    }

    private func notice(_ text: String, detail: String, color: Color) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(text)
                .font(.caption)
                .fixedSize(horizontal: false, vertical: true)
            Text(detail)
                .font(.caption2.monospaced())
                .foregroundStyle(.secondary)
                .textSelection(.enabled)
        }
        .padding(8)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(RoundedRectangle(cornerRadius: 6).fill(color.opacity(0.15)))
    }

    // MARK: - Overrides

    private var promptControls: some View {
        VStack(alignment: .leading, spacing: 6) {
            if let expiry = client.snoozeUntil {
                HStack {
                    VStack(alignment: .leading, spacing: 1) {
                        Text("Snoozed until \(expiry.formatted(date: .omitted, time: .shortened))")
                            .font(.caption)
                        Text("Prompt gates and subagent limits are off.")
                            .font(.caption2)
                            .foregroundStyle(.secondary)
                    }
                    Spacer()
                    Button("Resume now") { client.runPromptControl(["clear-snooze"]) }
                }
            } else {
                HStack {
                    Button("Snooze 15 min") { client.runPromptControl(["snooze", "15"]) }
                    Menu {
                        Button("5 minutes") { client.runPromptControl(["snooze", "5"]) }
                        Button("30 minutes") { client.runPromptControl(["snooze", "30"]) }
                        Button("60 minutes") { client.runPromptControl(["snooze", "60"]) }
                        Button("2 hours (the most allowed)") { client.runPromptControl(["snooze", "120"]) }
                    } label: {
                        Image(systemName: "ellipsis.circle")
                            .accessibilityLabel("More snooze durations")
                    }
                    .menuStyle(.borderlessButton)
                    .fixedSize()
                    Spacer()
                    Button(client.allowOncePending ? "Allow once armed" : "Allow once") {
                        client.runPromptControl(["once"])
                    }
                    .disabled(client.allowOncePending)
                    .help("Lets the next paused prompt through. Expires after 10 minutes.")
                }
            }

            if let error = client.promptControlError {
                Text(error)
                    .font(.caption)
                    .foregroundStyle(.red)
            }
        }
    }

    // MARK: - Numbers

    private var metrics: some View {
        let s = client.state
        return VStack(alignment: .leading, spacing: 6) {
            metricRow(
                label: "Memory free",
                value: String(format: "%.1f of %.0f GB", s.memoryAvailableGb, s.memoryTotalGb),
                progress: 1.0 - (s.memoryAvailableGb / max(s.memoryTotalGb, 1))
            )
            metricRow(label: "Swap used", value: String(format: "%.0f%%", s.swapUsedPercent), progress: s.swapUsedPercent / 100)
            metricRow(label: "CPU", value: String(format: "%.0f%%", s.cpuPercent), progress: s.cpuPercent / 100)
            metricRow(
                label: "Disk",
                value: String(format: "%.1f GB free", s.disk.availableGb),
                progress: s.disk.usedPercent / 100,
                tint: diskColor
            )
            HStack(spacing: 12) {
                tag("macOS memory", s.memoryPressureLevel)
                tag("Thermal", s.thermalState)
                Spacer()
            }
            if s.agents.totalMb > 0 {
                Text(agentsText)
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            if s.docker.runningContainers > 0 {
                Text("Docker: \(s.docker.runningContainers) containers, \(String(format: "%.0f%%", s.docker.totalCpuPercent)) CPU")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
        }
    }

    private var agentsText: String {
        let a = client.state.agents
        var parts: [String] = []
        if a.claudeMb > 0 { parts.append("Claude \(a.claudeMb)") }
        if a.codexMb > 0 { parts.append("Codex \(a.codexMb)") }
        if a.cursorMb > 0 { parts.append("Cursor \(a.cursorMb)") }
        return String(format: "Agents use %.1f GB", Double(a.totalMb) / 1024)
            + (parts.isEmpty ? "" : " (\(parts.joined(separator: ", ")) MB)")
    }

    private func tag(_ label: String, _ value: String) -> some View {
        HStack(spacing: 4) {
            Text(label)
                .foregroundStyle(.secondary)
            Text(value)
        }
        .font(.caption)
    }

    private func metricRow(label: String, value: String, progress: Double, tint: Color? = nil) -> some View {
        HStack {
            Text(label)
                .font(.caption)
                .frame(width: 80, alignment: .leading)
            ProgressView(value: min(max(progress, 0), 1))
                .tint(tint ?? progressColor(progress))
            Text(value)
                .font(.caption.monospacedDigit())
                .foregroundStyle(.secondary)
                .frame(width: 104, alignment: .trailing)
        }
    }

    private func todayRow(_ today: DayStats) -> some View {
        let changes = today.transitions == 1 ? "1 change" : "\(today.transitions) changes"
        return VStack(alignment: .leading, spacing: 2) {
            Text("Today")
                .font(.caption.weight(.semibold))
            Text(
                "\(today.percent(today.clearSecs))% clear · \(today.percent(today.strainedSecs))% strained · "
                    + "\(today.percent(today.criticalSecs))% critical · \(changes)"
            )
            .font(.caption.monospacedDigit())
            .foregroundStyle(.secondary)
        }
    }

    // MARK: - Disk

    private var diskSection: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack {
                Text("Free up disk space")
                    .font(.caption.weight(.semibold))
                Spacer()
                if client.diskScanning {
                    ProgressView()
                        .controlSize(.small)
                    Text("Scanning…")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                } else {
                    Button(client.diskReport == nil ? "Find space to free" : "Scan again") {
                        client.scanDisk()
                    }
                }
            }

            if let error = client.diskScanError {
                Text(error)
                    .font(.caption)
                    .foregroundStyle(.red)
                    .fixedSize(horizontal: false, vertical: true)
            }

            if let message = client.deleteMessage {
                Text(message)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }

            if let report = client.diskReport {
                if report.targets.isEmpty {
                    Text("Nothing large and easy to remove was found.")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                } else {
                    Text(summary(report))
                        .font(.caption2)
                        .foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                    ScrollView {
                        VStack(alignment: .leading, spacing: 4) {
                            ForEach(report.targets) { target in
                                targetRow(target)
                            }
                        }
                    }
                    .frame(maxHeight: 260)
                    Text("The bin deletes after asking, and only what is safe: nothing git tracks, no worktree with uncommitted work. Other rows copy a command to run yourself.")
                        .font(.caption2)
                        .foregroundStyle(.tertiary)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
        }
    }

    private func summary(_ report: DiskReport) -> String {
        var text = "\(report.reposScanned) git repositories scanned. "
            + "Deleting would free \(DiskTarget.sizeText(report.regeneratesBytes)) that comes back on its own, "
            + "\(DiskTarget.sizeText(report.reviewBytes)) that needs a look."
        if report.fromCache, let scannedAt = report.scannedAt {
            let formatter = RelativeDateTimeFormatter()
            formatter.unitsStyle = .full
            text = "Saved scan from \(formatter.localizedString(for: scannedAt, relativeTo: Date())). " + text
        }
        if report.unmeasured > 0 {
            text += " \(report.unmeasured) recently used places were not measured."
        }
        return text
    }

    private func targetRow(_ target: DiskTarget) -> some View {
        HStack(alignment: .top, spacing: 8) {
            Text(target.sizeText)
                .font(.caption.monospacedDigit().weight(.medium))
                .frame(width: 62, alignment: .trailing)
            VStack(alignment: .leading, spacing: 1) {
                Text(target.what)
                    .font(.caption)
                    .lineLimit(1)
                Text(target.displayPath)
                    .font(.caption2)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .truncationMode(.middle)
                Text(target.detailText)
                    .font(.caption2)
                    .foregroundStyle(target.regenerates ? Color.secondary : Color.orange)
                    .lineLimit(3)
                    .fixedSize(horizontal: false, vertical: true)
            }
            Spacer(minLength: 0)
            Button {
                NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: target.path)])
            } label: {
                Image(systemName: "folder")
            }
            .buttonStyle(.borderless)
            .help("Show in Finder")
            if client.deletingPath == target.path {
                ProgressView()
                    .controlSize(.small)
            } else if target.deletable {
                Button {
                    confirmDelete(target)
                } label: {
                    Image(systemName: "trash")
                }
                .buttonStyle(.borderless)
                .disabled(client.deletingPath != nil)
                .help("Delete (asks first)")
            } else {
                Button {
                    NSPasteboard.general.clearContents()
                    NSPasteboard.general.setString(target.remove, forType: .string)
                    copiedPath = target.path
                } label: {
                    Image(systemName: copiedPath == target.path ? "checkmark" : "doc.on.doc")
                }
                .buttonStyle(.borderless)
                .help("Copy: \(target.remove)")
            }
        }
        .padding(6)
        .background(RoundedRectangle(cornerRadius: 6).fill(Color.primary.opacity(0.05)))
    }

    /// Ask before deleting. Cancel is the default button; the delete button is marked
    /// destructive, so Return never deletes.
    private func confirmDelete(_ target: DiskTarget) {
        let alert = NSAlert()
        alert.alertStyle = target.regenerates ? .informational : .warning
        alert.messageText = "Delete \(target.what.lowercased()), \(target.sizeText)?"
        var lines = [target.displayPath, target.detailText]
        if target.kind == "worktree" {
            lines.append("Removed with git worktree remove. Ignored files in it, such as .env files and installed dependencies, are deleted with it.")
        } else {
            lines.append("It comes back: \(target.cost).")
        }
        lines.append("This cannot be undone.")
        alert.informativeText = lines.joined(separator: "\n\n")
        alert.addButton(withTitle: "Cancel")
        let delete = alert.addButton(withTitle: "Delete")
        delete.hasDestructiveAction = true
        NSApp.activate(ignoringOtherApps: true)
        if alert.runModal() == .alertSecondButtonReturn {
            client.deleteTarget(target)
        }
    }

    // MARK: - Queue

    private var queueSection: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text("Queued jobs (\(client.queueEntries.count))")
                .font(.caption.weight(.semibold))
            ScrollView {
                VStack(alignment: .leading, spacing: 4) {
                    ForEach(client.queueEntries) { entry in
                        Button {
                            CursorWorkspaceOpener.open(workspacePath: entry.workspacePath)
                        } label: {
                            VStack(alignment: .leading, spacing: 1) {
                                Text(entry.title)
                                    .font(.caption)
                                    .lineLimit(2)
                                    .multilineTextAlignment(.leading)
                                if let workspace = entry.workspacePath, !workspace.isEmpty {
                                    Text(workspace)
                                        .font(.caption2)
                                        .foregroundStyle(.secondary)
                                        .lineLimit(1)
                                }
                            }
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .padding(6)
                            .background(RoundedRectangle(cornerRadius: 6).fill(Color.primary.opacity(0.05)))
                        }
                        .buttonStyle(.plain)
                    }
                }
            }
            .frame(maxHeight: 120)
        }
    }

    // MARK: - Colors

    private var pressureColor: Color {
        if client.isStale { return .gray }
        switch client.state.pressure {
        case .clear: return .green
        case .strained: return .yellow
        case .critical: return .red
        }
    }

    private var diskColor: Color {
        switch client.state.disk.level {
        case "critical": return .red
        case "warn": return .yellow
        default: return .green
        }
    }

    private func progressColor(_ value: Double) -> Color {
        if value > 0.9 { return .red }
        if value > 0.7 { return .yellow }
        return .green
    }
}
