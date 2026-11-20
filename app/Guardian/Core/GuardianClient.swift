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
