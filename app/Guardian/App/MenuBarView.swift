import AppKit
import SwiftUI

/// The panel that opens from the menu bar item: what the level is and why, what is switched
/// off, how today has gone, and where disk space can be found.
struct MenuBarView: View {
    @EnvironmentObject private var client: GuardianClient
    @State private var copiedPath: String?
    /// Paths ticked for deletion in the disk list.
    @State private var selected: Set<String> = []
    /// The worktree whose save is being worked out before asking about it.
    @State private var planningPath: String?

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
        // A new scan can change what a row is (safe then, needing a look now), so ticks
        // made against the old one are dropped rather than carried over.
        .onChange(of: client.diskReport?.scannedAt) { _, _ in
            selected = []
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

    /// Height of the list of targets. Exact, not a maximum: a menu bar panel sizes itself to
    /// the least its content needs, which for a scroll view with only a maximum is nothing,
    /// and the rows vanished. Sized so the whole panel fits a 13-inch screen.
    private static let listHeight: CGFloat = 214

    private var diskSection: some View {
        VStack(alignment: .leading, spacing: 8) {
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
                if report.targets.isEmpty && report.bundles.isEmpty {
                    Text("Nothing large and easy to remove was found.")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                } else {
                    Text(summary(report))
                        .font(.caption2)
                        .foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                    targetList(report)
                }
            } else if !client.diskScanning {
                Text("Lists build output, dependency folders, unused worktrees and caches, with what deleting each would free. Finding them deletes nothing.")
                    .font(.caption2)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
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

    /// Ticked rows that are still in the list, without any that sit inside another.
    private func picked(from targets: [DiskTarget]) -> [DiskTarget] {
        DiskTarget.withoutNested(targets.filter { $0.deletable && selected.contains($0.path) })
    }

    /// Everything found, in one scrolling list: stale bundles first (the longest untouched
    /// at the top), then single folders Guardian deletes itself, then what needs its own
    /// tool or a decision (a command to copy).
    @ViewBuilder
    private func targetList(_ report: DiskReport) -> some View {
        let bundles = report.bundles
        let members = bundles.flatMap(\.members)
        // Stale folders that can be deleted but need a look are offered one at a time.
        let singles = members.filter { $0.deletable && !$0.regenerates } + report.targets.filter(\.deletable)
        // Worktrees held back only by unsaved work: one button commits it, pushes its
        // branch and then removes the worktree.
        let unsaved = (members + report.targets).filter(\.saveable)
        let manual = (members + report.targets).filter { !$0.deletable && !$0.saveable }
        let chosen = picked(from: members + report.targets)
        let old = Set(bundles.filter { $0.isOld && $0.isProject }.flatMap(\.deletePaths))
        let canDelete = !singles.isEmpty || bundles.contains { !$0.deletePaths.isEmpty }

        if canDelete {
            Text((bundles.isEmpty ? "" : "Stale things come first, bundled by project. ")
                + "Tick what to delete, then press Delete selected. Nothing is removed until you confirm.")
                .font(.caption)
                .fixedSize(horizontal: false, vertical: true)
        }
        Text(counts(bundles: bundles.count, unsaved: unsaved.count, singles: singles.count, manual: manual.count))
            .font(.caption2)
            .foregroundStyle(.secondary)
        ScrollView {
            VStack(alignment: .leading, spacing: 4) {
                ForEach(Array(bundles.enumerated()), id: \.element.id) { index, bundle in
                    if index == 0 || bundles[index - 1].heading != bundle.heading {
                        listLabel(bundle.heading, first: index == 0)
                    }
                    bundleRow(bundle)
                }
                if !unsaved.isEmpty {
                    listLabel(
                        "Unsaved work: Save & delete commits it, pushes its branch, then removes the worktree. It asks first.",
                        first: bundles.isEmpty
                    )
                    ForEach(unsaved) { target in
                        unsavedRow(target)
                    }
                }
                if !singles.isEmpty {
                    if !bundles.isEmpty || !unsaved.isEmpty {
                        listLabel("Single folders", first: false)
                    }
                    ForEach(singles) { target in
                        deletableRow(target)
                    }
                }
                if !manual.isEmpty {
                    listLabel(
                        "Needs a command: Guardian does not delete these itself. Copy the command and run it in a terminal.",
                        first: bundles.isEmpty && singles.isEmpty && unsaved.isEmpty
                    )
                    ForEach(manual) { target in
                        commandRow(target)
                    }
                }
            }
        }
        .frame(height: Self.listHeight)
        if canDelete {
            HStack {
                Button(selected.isEmpty ? "Tick old and safe" : "Untick all") {
                    selected = selected.isEmpty ? old : []
                }
                .disabled(selected.isEmpty && old.isEmpty)
                .help(old.isEmpty
                    ? "No project has anything that both comes back on its own and has gone untouched for \(DiskTarget.oldDays) days. Tick rows yourself."
                    : "Ticks every bundle untouched for \(DiskTarget.oldDays) days or more: only what comes back on its own")
                Spacer()
                Button(chosen.isEmpty
                    ? "Delete selected…"
                    : "Delete \(chosen.count) selected (\(DiskTarget.sizeText(chosen.map(\.freedBytes).reduce(0, +))))…") {
                    confirmDelete(chosen)
                }
                .disabled(chosen.isEmpty || !client.deletingPaths.isEmpty)
                .help("Asks first. Deletes only what is safe: nothing git tracks, no worktree with uncommitted work. Each one is checked again at that moment.")
            }
        }
    }

    private func listLabel(_ text: String, first: Bool) -> some View {
        Text(text)
            .font(.caption2.weight(.semibold))
            .foregroundStyle(.secondary)
            .fixedSize(horizontal: false, vertical: true)
            .padding(.top, first ? 0 : 6)
    }

    private func counts(bundles: Int, unsaved: Int, singles: Int, manual: Int) -> String {
        let plural = { (n: Int, one: String, many: String) in n == 1 ? "1 \(one)" : "\(n) \(many)" }
        var parts: [String] = []
        if bundles > 0 { parts.append(plural(bundles, "stale bundle", "stale bundles")) }
        if unsaved > 0 { parts.append("\(unsaved) with unsaved work") }
        if singles > 0 { parts.append(plural(singles, "single folder", "single folders")) }
        if manual > 0 { parts.append("\(manual) that need a command") }
        return parts.joined(separator: ", ") + ". Scroll for more."
    }

    /// A bundle as one row: ticking it ticks everything in it that can go without a look.
    private func bundleRow(_ bundle: DiskBundle) -> some View {
        let paths = Set(bundle.deletePaths)
        let isOn = Binding(
            get: { !paths.isEmpty && paths.isSubset(of: selected) },
            set: { on in
                if on { selected.formUnion(paths) } else { selected.subtract(paths) }
            }
        )
        let canTick = !paths.isEmpty && client.deletingPaths.isEmpty
        return HStack(alignment: .top, spacing: 8) {
            if !client.deletingPaths.isDisjoint(with: paths) {
                ProgressView()
                    .controlSize(.small)
                    .frame(width: 16)
            } else if paths.isEmpty {
                // Nothing in it can go without a look: shown, not tickable.
                Color.clear.frame(width: 16, height: 16)
            } else {
                Toggle("Delete the stale folders of \(bundle.name)", isOn: isOn)
                    .toggleStyle(.checkbox)
                    .labelsHidden()
                    .disabled(!client.deletingPaths.isEmpty)
            }
            Text(DiskTarget.sizeText(bundle.deletableFreedBytes))
                .font(.caption.monospacedDigit().weight(.medium))
                .frame(width: 56, alignment: .trailing)
            VStack(alignment: .leading, spacing: 1) {
                Text(bundle.name)
                    .font(.caption)
                    .lineLimit(1)
                    .truncationMode(.middle)
                Text(bundle.contents)
                    .font(.caption2)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                Text(bundle.detail)
                    .font(.caption2)
                    .foregroundStyle(.secondary)
                    .lineLimit(3)
                    .fixedSize(horizontal: false, vertical: true)
                    .help(bundle.members.prefix(12).map { "\($0.sizeText)  \($0.displayPath)" }.joined(separator: "\n"))
            }
            Spacer(minLength: 0)
        }
        .padding(6)
        .background(RoundedRectangle(cornerRadius: 6).fill(Color.primary.opacity(isOn.wrappedValue ? 0.12 : 0.05)))
        .contentShape(Rectangle())
        .onTapGesture {
            if canTick { isOn.wrappedValue.toggle() }
        }
    }

    private func deletableRow(_ target: DiskTarget) -> some View {
        let isOn = Binding(
            get: { selected.contains(target.path) },
            set: { on in
                if on { selected.insert(target.path) } else { selected.remove(target.path) }
            }
        )
        return HStack(alignment: .top, spacing: 8) {
            if client.deletingPaths.contains(target.path) {
                ProgressView()
                    .controlSize(.small)
                    .frame(width: 16)
            } else {
                Toggle("Delete \(target.displayPath)", isOn: isOn)
                    .toggleStyle(.checkbox)
                    .labelsHidden()
                    .disabled(!client.deletingPaths.isEmpty)
            }
            targetText(target)
            Spacer(minLength: 0)
            finderButton(target)
        }
        .padding(6)
        .background(RoundedRectangle(cornerRadius: 6).fill(Color.primary.opacity(isOn.wrappedValue ? 0.12 : 0.05)))
        .contentShape(Rectangle())
        .onTapGesture {
            if client.deletingPaths.isEmpty { isOn.wrappedValue.toggle() }
        }
    }

    /// A worktree kept only by unsaved work, with the button that saves it and removes it.
    private func unsavedRow(_ target: DiskTarget) -> some View {
        let busy = client.deletingPaths.contains(target.path) || planningPath == target.path
        return HStack(alignment: .top, spacing: 8) {
            targetText(target)
            Spacer(minLength: 0)
            finderButton(target)
            if busy {
                ProgressView()
                    .controlSize(.small)
            } else {
                Button("Save & delete…") {
                    beginSave(target)
                }
                .controlSize(.small)
                .disabled(!client.deletingPaths.isEmpty || planningPath != nil)
                .help("Commits its uncommitted files, pushes its branch, checks the remote has it, then removes the worktree")
            }
        }
        .padding(6)
        .background(RoundedRectangle(cornerRadius: 6).fill(Color.primary.opacity(0.05)))
    }

    /// Work out what a save would do, then ask. Nothing is changed before the answer.
    private func beginSave(_ target: DiskTarget) {
        planningPath = target.path
        client.planSave(target) { result in
            planningPath = nil
            guard let result, result.ok, let plan = result.plan else {
                let alert = NSAlert()
                alert.alertStyle = .informational
                alert.messageText = "Guardian will not save this one"
                alert.informativeText = "\(target.displayPath)\n\n"
                    + (result?.error ?? "The save command did not run. It needs guardian 0.12 or newer: bash scripts/install.sh")
                    + "\n\nNothing was changed."
                NSApp.activate(ignoringOtherApps: true)
                alert.runModal()
                return
            }
            confirmSave(target, plan)
        }
    }

    /// Say exactly what will be committed and pushed, and ask. Cancel is the default.
    private func confirmSave(_ target: DiskTarget, _ plan: SavePlan) {
        let alert = NSAlert()
        alert.alertStyle = .warning
        alert.messageText = "Commit, push, then remove this worktree?"
        var lines = [target.displayPath]
        let commit: String
        switch plan.files.count {
        case 0: commit = "Nothing is uncommitted"
        case 1: commit = "Commits 1 file"
        default: commit = "Commits \(plan.files.count) files"
        }
        let branch: String
        if let shared = plan.fromBranch {
            branch = "; \(shared) is a shared branch, so the work goes on a new one, \(plan.branch),"
        } else if plan.newBranch {
            branch = "; its detached HEAD goes on a new branch, \(plan.branch),"
        } else {
            branch = plan.files.isEmpty ? " on branch \(plan.branch);" : " to branch \(plan.branch)"
        }
        let push: String
        if plan.upToDate {
            push = " \(plan.remote)/\(plan.remoteBranch) already has it, so nothing is pushed."
        } else if plan.beside {
            push = " and pushes it to \(plan.remote) as a new branch, \(plan.remoteBranch). \(plan.remote)/\(plan.branch) is left as it is."
        } else {
            push = " and pushes it to \(plan.remote)/\(plan.remoteBranch), which is not on the remote yet."
        }
        lines.append("\(commit)\(branch)\(push) Then removes the worktree, freeing about \(target.sizeText).")
        if !plan.files.isEmpty {
            var files = plan.files.prefix(8).joined(separator: "\n")
            if plan.files.count > 8 { files += "\n…and \(plan.files.count - 8) more" }
            lines.append(files)
        }
        if !plan.ignored.isEmpty {
            lines.append("NOT saved, because git ignores \(plan.ignored.count == 1 ? "it" : "them"), and deleted with the worktree: "
                + plan.ignored.prefix(6).joined(separator: ", ")
                + (plan.ignored.count > 6 ? " and \(plan.ignored.count - 6) more" : ""))
        }
        lines.append("Your commit and push hooks run as usual, and nothing is force-pushed. If a hook refuses, the push does not go through, or anything here changes first, the worktree is kept.")
        alert.informativeText = lines.joined(separator: "\n\n")
        alert.addButton(withTitle: "Cancel")
        let save = alert.addButton(withTitle: "Save and delete")
        save.hasDestructiveAction = true
        NSApp.activate(ignoringOtherApps: true)
        if alert.runModal() == .alertSecondButtonReturn {
            client.saveAndDelete(target, plan: plan)
        }
    }

    private func commandRow(_ target: DiskTarget) -> some View {
        HStack(alignment: .top, spacing: 8) {
            targetText(target)
            Spacer(minLength: 0)
            finderButton(target)
            Button(copiedPath == target.path ? "Copied" : "Copy") {
                NSPasteboard.general.clearContents()
                NSPasteboard.general.setString(target.remove, forType: .string)
                copiedPath = target.path
            }
            .controlSize(.small)
            .help("Copy: \(target.remove)")
        }
        .padding(6)
        .background(RoundedRectangle(cornerRadius: 6).fill(Color.primary.opacity(0.05)))
    }

    private func targetText(_ target: DiskTarget) -> some View {
        HStack(alignment: .top, spacing: 8) {
            Text(target.sizeText)
                .font(.caption.monospacedDigit().weight(.medium))
                .frame(width: 56, alignment: .trailing)
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
                    .lineLimit(2)
                    .fixedSize(horizontal: false, vertical: true)
                    .help(target.detailText)
            }
        }
    }

    private func finderButton(_ target: DiskTarget) -> some View {
        Button {
            NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: target.path)])
        } label: {
            Image(systemName: "folder")
        }
        .buttonStyle(.borderless)
        .help("Show in Finder")
    }

    /// Ask before deleting. Cancel is the default button; the delete button is marked
    /// destructive, so Return never deletes.
    private func confirmDelete(_ targets: [DiskTarget]) {
        guard !targets.isEmpty else { return }
        // What needs a look is listed first, so it is never among the ones left unnamed.
        let targets = targets.filter { !$0.regenerates } + targets.filter(\.regenerates)
        let total = DiskTarget.sizeText(targets.map(\.freedBytes).reduce(0, +))
        let alert = NSAlert()
        alert.alertStyle = targets.allSatisfy(\.regenerates) ? .informational : .warning
        alert.messageText = targets.count == 1
            ? "Delete \(targets[0].what.lowercased()), freeing about \(total)?"
            : "Delete these \(targets.count) items, freeing about \(total)?"
        var lines = targets.prefix(8).map { target in
            "\(target.sizeText)  \(target.displayPath)" + (target.regenerates ? "" : "\n    Look first: \(target.detailText)")
        }
        if targets.count > 8 {
            lines.append("…and \(targets.count - 8) more")
        }
        var notes: [String] = []
        if targets.contains(where: { $0.kind == "worktree" }) {
            notes.append("Worktrees are removed with git worktree remove. Ignored files in them, such as .env files and installed dependencies, go with them; their branches stay.")
        }
        notes.append("Each one is checked again first, and anything with unsaved work is kept. This cannot be undone.")
        alert.informativeText = lines.joined(separator: "\n") + "\n\n" + notes.joined(separator: "\n\n")
        alert.addButton(withTitle: "Cancel")
        let delete = alert.addButton(withTitle: "Delete")
        delete.hasDestructiveAction = true
        NSApp.activate(ignoringOtherApps: true)
        if alert.runModal() == .alertSecondButtonReturn {
            selected.subtract(targets.map(\.path))
            client.deleteTargets(targets)
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
