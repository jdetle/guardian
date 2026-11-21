import Foundation
import Combine

final class GuardianClient: ObservableObject {
    /// One client for the whole app: the menu bar, the dashboard window and notifications
    /// all read the same polled state.
    static let shared = GuardianClient()

    /// Longest snooze the hooks honor (GUARDIAN_MAX_SNOOZE_SECS in hooks/lib.sh, plus slack).
    static let maxSnoozeSeconds: TimeInterval = 7200 + 120
    /// How long an allow-once token stays good (GUARDIAN_ONCE_TTL_MINUTES in hooks/lib.sh).
    static let allowOnceSeconds: TimeInterval = 600
    /// state.json older than this means guardiand is not reporting (hooks use the same 30 s).
    static let staleSeconds: TimeInterval = 30

    @Published var state = GuardianStateData()
    /// True when guardiand has stopped writing state.json: the numbers shown are old and
    /// the hooks have failed open.
    @Published private(set) var isStale = false
    @Published private(set) var today: DayStats?
    @Published private(set) var diskReport: DiskReport?
    @Published private(set) var diskScanning = false
    @Published private(set) var diskScanError: String?
    /// The paths being deleted right now.
    @Published private(set) var deletingPaths: Set<String> = []
    /// The outcome of the last delete, in a sentence.
    @Published private(set) var deleteMessage: String?
    @Published var history: [GuardianStateData] = []
    @Published var sessions: [SessionRecord] = []
    @Published var queueEntries: [AgentQueueEntry] = []

    @Published private(set) var snoozeUntil: Date?
    @Published private(set) var allowOncePending = false
    @Published private(set) var promptControlError: String?

    private let guardianDir: URL
    private let stateFileURL: URL
    private let queueFileURL: URL
    private let dbPath: String
    private let statsFileURL: URL
    private let promptControlQueue = DispatchQueue(label: "guardian.prompt-controls")
    private let scanQueue = DispatchQueue(label: "guardian.disk-scan", qos: .utility)
    private let savedScanQueue = DispatchQueue(label: "guardian.disk-saved-scan", qos: .utility)
    private let deleteQueue = DispatchQueue(label: "guardian.disk-delete", qos: .userInitiated)
    private var timer: Timer?

    static let maxHistoryCount = 150

    init() {
        let home = FileManager.default.homeDirectoryForCurrentUser
        let guardianDir = home.appendingPathComponent(".guardian")
        self.guardianDir = guardianDir
        self.stateFileURL = guardianDir.appendingPathComponent("state.json")
        self.queueFileURL = guardianDir.appendingPathComponent("agent_queue.jsonl")
        self.dbPath = guardianDir.appendingPathComponent("sessions.db").path
        self.statsFileURL = guardianDir.appendingPathComponent("pressure_stats.json")
    }

    /// Start reading ~/.guardian. Safe to call more than once.
    func startPolling() {
        guard timer == nil else { return }
        loadPromptControls()
        loadState()
        loadQueue()
        loadStats()
        loadSavedDiskScan()
        // guardiand samples every 2 s; the menu bar icon does not need to be that fresh.
        var ticks = 0
        let t = Timer(timeInterval: 4.0, repeats: true) { [weak self] _ in
            self?.loadPromptControls()
            self?.loadState()
            self?.loadQueue()
            // guardiand saves the day's totals about once a minute; keep "Today" in step,
            // including across midnight.
            ticks += 1
            if ticks % 15 == 0 { self?.loadStats() }
        }
        t.tolerance = 1.0
        RunLoop.main.add(t, forMode: .common)
        timer = t
    }

    func stopPolling() {
        timer?.invalidate()
        timer = nil
    }

    func loadPromptControls() {
        let marker = guardianDir.appendingPathComponent("snooze_until")
        let text = try? String(contentsOf: marker, encoding: .utf8)
        let now = Date()
        // Same rule as the hooks: a snooze further out than the cap is not honored.
        let expiry = text.flatMap {
            ISO8601DateFormatter().date(from: $0.trimmingCharacters(in: .whitespacesAndNewlines))
        }.flatMap { $0 > now && $0.timeIntervalSince(now) <= Self.maxSnoozeSeconds ? $0 : nil }
        if snoozeUntil != expiry { snoozeUntil = expiry }

        // An allow-once token expires; an old one is not "armed".
        let oncePath = guardianDir.appendingPathComponent("proceed_once").path
        let armedAt = (try? FileManager.default.attributesOfItem(atPath: oncePath))?[.modificationDate] as? Date
        let pending = armedAt.map { now.timeIntervalSince($0) < Self.allowOnceSeconds } ?? false
        if allowOncePending != pending { allowOncePending = pending }
    }

    /// Today's share of time at each level, from guardiand's per-day accounting.
    func loadStats() {
        guard let data = try? Data(contentsOf: statsFileURL),
              let file = try? JSONDecoder().decode(PressureStatsFile.self, from: data) else {
            if today != nil { today = nil }
            return
        }
        let formatter = DateFormatter()
        formatter.locale = Locale(identifier: "en_US_POSIX")
        formatter.dateFormat = "yyyy-MM-dd"
        today = file.days[formatter.string(from: Date())]
    }

    /// Show the scan guardiand keeps up to date in the background, at once and without
    /// scanning (`guardian disk --cached`). Does nothing when there is no saved scan, when a
    /// newer report is already shown, or with a guardian too old to keep one.
    func loadSavedDiskScan() {
        guard diskReport == nil, !diskScanning else { return }
        let executable = guardianDir.appendingPathComponent("guardian")
        savedScanQueue.async {
            let process = Process()
            process.executableURL = executable
            process.arguments = ["disk", "--cached", "--json", "--top", "12"]
            let out = Pipe()
            process.standardOutput = out
            process.standardError = FileHandle.nullDevice
            var report: DiskReport?
            do {
                try process.run()
                let data = out.fileHandleForReading.readDataToEndOfFile()
                process.waitUntilExit()
                if process.terminationStatus == 0 {
                    report = try? JSONDecoder().decode(DiskReport.self, from: data)
                }
            } catch {
                report = nil
            }
            DispatchQueue.main.async {
                if let report, self.diskReport == nil, !self.diskScanning {
                    self.diskReport = report
                }
            }
        }
    }

    /// Run `guardian disk --json`: a read-only survey of what is cheap to delete. It measures
    /// only what has changed since the saved scan, so it is usually quick.
    func scanDisk() {
        guard !diskScanning else { return }
        diskScanning = true
        diskScanError = nil
        let executable = guardianDir.appendingPathComponent("guardian")
        scanQueue.async {
            let process = Process()
            process.executableURL = executable
            process.arguments = ["disk", "--json", "--top", "12", "--seconds", "45"]
            let out = Pipe()
            process.standardOutput = out
            process.standardError = FileHandle.nullDevice
            var report: DiskReport?
            var failure: String?
            do {
                try process.run()
                let data = out.fileHandleForReading.readDataToEndOfFile()
                process.waitUntilExit()
                if process.terminationStatus == 0 {
                    report = try? JSONDecoder().decode(DiskReport.self, from: data)
                }
                if report == nil {
                    failure = "The scan needs guardian 0.8 or newer. Reinstall: bash scripts/install.sh"
                }
            } catch {
                failure = error.localizedDescription
            }
            DispatchQueue.main.async {
                self.diskScanning = false
                self.diskScanError = failure
                if let report { self.diskReport = report }
            }
        }
    }

    func runPromptControl(_ arguments: [String]) {
        promptControlError = nil
        let executable = guardianDir.appendingPathComponent("guardian")
        promptControlQueue.async {
            let process = Process()
            process.executableURL = executable
            process.arguments = arguments
            process.standardOutput = FileHandle.nullDevice
            let errorPipe = Pipe()
            process.standardError = errorPipe
            let failure: String?
            do {
                try process.run()
                let errorData = errorPipe.fileHandleForReading.readDataToEndOfFile()
                process.waitUntilExit()
                let message = String(decoding: errorData, as: UTF8.self)
                    .trimmingCharacters(in: .whitespacesAndNewlines)
                failure = process.terminationStatus == 0 ? nil :
                    (message.isEmpty ? "Guardian could not update prompt controls." : message)
            } catch {
                failure = error.localizedDescription
            }
            DispatchQueue.main.async {
                self.promptControlError = failure
                self.loadPromptControls()
            }
        }
    }

    /// Delete scanned targets with `guardian disk delete`. The CLI re-checks each one first
    /// (nothing git tracks, no uncommitted work in a worktree) and keeps what fails.
    /// Call only after the person has confirmed.
    func deleteTargets(_ targets: [DiskTarget]) {
        let chosen = DiskTarget.withoutNested(targets)
        guard deletingPaths.isEmpty, !chosen.isEmpty else { return }
        deletingPaths = Set(chosen.map(\.path))
        deleteMessage = nil
        let executable = guardianDir.appendingPathComponent("guardian")
        // Not the scan queue: a delete must not wait behind a scan that is running.
        deleteQueue.async {
            let process = Process()
            process.executableURL = executable
            process.arguments = ["disk", "delete", "--yes", "--json"] + chosen.map(\.path)
            let out = Pipe()
            process.standardOutput = out
            process.standardError = FileHandle.nullDevice
            var results: [DeleteResult] = []
            do {
                try process.run()
                let data = out.fileHandleForReading.readDataToEndOfFile()
                process.waitUntilExit()
                results = (try? JSONDecoder().decode([DeleteResult].self, from: data)) ?? []
            } catch {
                results = []
            }
            DispatchQueue.main.async {
                self.deletingPaths = []
                let deleted = results.filter(\.ok)
                let kept = results.filter { !$0.ok }
                if var report = self.diskReport {
                    for done in deleted {
                        let gone = { (path: String) in path == done.path || path.hasPrefix(done.path + "/") }
                        report.targets.removeAll { gone($0.path) }
                        for i in report.bundles.indices {
                            report.bundles[i].members.removeAll { gone($0.path) }
                            report.bundles[i].deletePaths.removeAll(where: gone)
                        }
                    }
                    report.bundles.removeAll { $0.members.isEmpty }
                    self.diskReport = report
                }
                self.deleteMessage = Self.deleteSummary(chosen: chosen, deleted: deleted, kept: kept)
                self.loadState()
                // Bring the totals up to date; quick, since only what changed is measured.
                if !deleted.isEmpty { self.scanDisk() }
            }
        }
    }

    /// Ask what saving a worktree's unsaved work would do (`guardian disk save --plan`):
    /// which files would be committed, to which branch, pushed where. Changes nothing.
    /// `completion` runs on the main queue; nil when the command could not be run.
    func planSave(_ target: DiskTarget, completion: @escaping (SavePlanResult?) -> Void) {
        let executable = guardianDir.appendingPathComponent("guardian")
        deleteQueue.async {
            let data = Self.run(executable, ["disk", "save", "--plan", "--json", target.path])
            let result = data.flatMap { try? JSONDecoder().decode([SavePlanResult].self, from: $0) }?.first
            DispatchQueue.main.async { completion(result) }
        }
    }

    /// Commit and push a worktree's unsaved work, then remove the worktree
    /// (`guardian disk save --delete`). The worktree is kept if the push does not go
    /// through, or if it no longer matches `plan`, the one the person was shown. Call only
    /// after they have confirmed.
    func saveAndDelete(_ target: DiskTarget, plan: SavePlan) {
        guard deletingPaths.isEmpty else { return }
        deletingPaths = [target.path]
        deleteMessage = nil
        let executable = guardianDir.appendingPathComponent("guardian")
        deleteQueue.async {
            let data = Self.run(
                executable,
                ["disk", "save", "--delete", "--yes", "--json", "--plan-id", plan.id, target.path]
            )
            let result = data.flatMap { try? JSONDecoder().decode([SaveResult].self, from: $0) }?.first
            DispatchQueue.main.async {
                self.deletingPaths = []
                guard let result else {
                    self.deleteMessage = "Kept \(target.displayPath): the save command did not run. It needs guardian 0.12 or newer."
                    return
                }
                var parts: [String] = []
                if result.saved == true {
                    let files = result.committedFiles ?? 0
                    parts.append("Saved \(target.displayPath): "
                        + (files == 0 ? "nothing was uncommitted" : "\(files) file\(files == 1 ? "" : "s") committed")
                        + ", branch pushed to \(result.pushedTo ?? "its remote").")
                }
                if result.deleted == true {
                    parts.append("Removed it, freeing about \(DiskTarget.sizeText(result.freedBytes ?? 0)).")
                } else {
                    parts.append("Kept it: \(result.error ?? "it was not removed").")
                }
                self.deleteMessage = parts.joined(separator: " ")
                self.loadState()
                // What git says about it has changed either way: look again.
                self.scanDisk()
            }
        }
    }

    /// Run the guardian CLI and return what it printed; nil if it could not be started.
    private static func run(_ executable: URL, _ arguments: [String]) -> Data? {
        let process = Process()
        process.executableURL = executable
        process.arguments = arguments
        let out = Pipe()
        process.standardOutput = out
        process.standardError = FileHandle.nullDevice
        do {
            try process.run()
            let data = out.fileHandleForReading.readDataToEndOfFile()
            process.waitUntilExit()
            return data
        } catch {
            return nil
        }
    }

    /// One or two sentences on what a delete did.
    static func deleteSummary(chosen: [DiskTarget], deleted: [DeleteResult], kept: [DeleteResult]) -> String {
        let shown = { (path: String) in chosen.first { $0.path == path }?.displayPath ?? path }
        var parts: [String] = []
        if deleted.count == 1 {
            parts.append("Deleted \(shown(deleted[0].path)), freeing about \(DiskTarget.sizeText(deleted[0].freedBytes ?? 0)).")
        } else if deleted.count > 1 {
            let freed = deleted.compactMap(\.freedBytes).reduce(0, +)
            parts.append("Deleted \(deleted.count) items, freeing about \(DiskTarget.sizeText(freed)).")
        }
        for result in kept {
            parts.append("Kept \(shown(result.path)): \(result.error ?? "it could not be deleted").")
        }
        if deleted.isEmpty && kept.isEmpty {
            parts.append("Nothing was deleted: the delete command did not run.")
        }
        return parts.joined(separator: " ")
    }

    private func loadState() {
        let written = (try? FileManager.default.attributesOfItem(atPath: stateFileURL.path))?[.modificationDate] as? Date
        let stale = written.map { Date().timeIntervalSince($0) > Self.staleSeconds } ?? true
        if isStale != stale { isStale = stale }

        guard let data = try? Data(contentsOf: stateFileURL) else { return }
        guard let decoded = try? JSONDecoder().decode(GuardianStateData.self, from: data) else { return }

        // Always called on the main thread (timer, launch, snapshot), so publish directly.
        state = decoded
        history.append(decoded)
        if history.count > Self.maxHistoryCount {
            history.removeFirst(history.count - Self.maxHistoryCount)
        }
    }

    /// Read everything once, synchronously (launch and `--render-panel`).
    func refreshNow() {
        loadPromptControls()
        loadState()
        loadStats()
    }

    private func loadQueue() {
        guard let text = try? String(contentsOf: queueFileURL, encoding: .utf8) else {
            DispatchQueue.main.async { self.queueEntries = [] }
            return
        }
        var parsed: [AgentQueueEntry] = []
        for line in text.split(separator: "\n", omittingEmptySubsequences: false) {
            let trimmed = line.trimmingCharacters(in: .whitespacesAndNewlines)
            if trimmed.isEmpty { continue }
            guard let data = trimmed.data(using: .utf8),
                  let entry = try? JSONDecoder().decode(AgentQueueEntry.self, from: data) else {
                continue
            }
            parsed.append(entry)
        }
        DispatchQueue.main.async {
            self.queueEntries = parsed
        }
    }

    func loadSessions() {
        DispatchQueue.global().async { [self] in
            let output = shellSync("sqlite3 -json '\(dbPath)' 'SELECT * FROM sessions ORDER BY started_at DESC LIMIT 50;'")
            guard let data = output.data(using: .utf8),
                  let decoded = try? JSONDecoder().decode([SessionRecord].self, from: data) else {
                return
            }
            DispatchQueue.main.async {
                self.sessions = decoded
            }
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

struct SessionRecord: Codable, Identifiable {
    var id: String { conversationId }
    let conversationId: String
    let model: String
    let startedAt: String
    let endedAt: String?
    let status: String?
    let loopCount: Int?
    let toolCallCount: Int?
    let pressureAtStart: String?
    let pressureAvg: String?
    let durationMs: Int?
    let filesModified: Int?
    let linesChanged: Int?
    let estimatedCostUsd: Double?
    let roiScore: Double?

    enum CodingKeys: String, CodingKey {
        case conversationId = "conversation_id"
        case model
        case startedAt = "started_at"
        case endedAt = "ended_at"
        case status
        case loopCount = "loop_count"
        case toolCallCount = "tool_call_count"
        case pressureAtStart = "pressure_at_start"
        case pressureAvg = "pressure_avg"
        case durationMs = "duration_ms"
        case filesModified = "files_modified"
        case linesChanged = "lines_changed"
        case estimatedCostUsd = "estimated_cost_usd"
        case roiScore = "roi_score"
    }

    var durationFormatted: String {
        guard let ms = durationMs else { return "--" }
        let seconds = ms / 1000
        if seconds < 60 { return "\(seconds)s" }
        let minutes = seconds / 60
        let secs = seconds % 60
        return "\(minutes)m \(secs)s"
    }
}
