import Foundation
import Combine

final class GuardianClient: ObservableObject {
    @Published var state = GuardianStateData()
    @Published var history: [GuardianStateData] = []
    @Published var sessions: [SessionRecord] = []
    @Published var queueEntries: [AgentQueueEntry] = []

    private let stateFileURL: URL
    private let queueFileURL: URL
    private let dbPath: String
    private var timer: Timer?
    private var sessionTimer: Timer?

    static let maxHistoryCount = 150

    init() {
        let home = FileManager.default.homeDirectoryForCurrentUser
        let guardianDir = home.appendingPathComponent(".guardian")
        self.stateFileURL = guardianDir.appendingPathComponent("state.json")
        self.queueFileURL = guardianDir.appendingPathComponent("agent_queue.jsonl")
        self.dbPath = guardianDir.appendingPathComponent("sessions.db").path
    }

    func startPolling() {
        loadState()
        loadQueue()
        loadSessions()
        timer = Timer.scheduledTimer(withTimeInterval: 2.0, repeats: true) { [weak self] _ in
            self?.loadState()
            self?.loadQueue()
        }
        sessionTimer = Timer.scheduledTimer(withTimeInterval: 10.0, repeats: true) { [weak self] _ in
            self?.loadSessions()
        }
    }

    func stopPolling() {
        timer?.invalidate()
        timer = nil
        sessionTimer?.invalidate()
        sessionTimer = nil
    }

    private func loadState() {
        guard let data = try? Data(contentsOf: stateFileURL) else { return }
        guard let decoded = try? JSONDecoder().decode(GuardianStateData.self, from: data) else { return }

        DispatchQueue.main.async {
            self.state = decoded
            self.history.append(decoded)
            if self.history.count > Self.maxHistoryCount {
                self.history.removeFirst(self.history.count - Self.maxHistoryCount)
            }
        }
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
