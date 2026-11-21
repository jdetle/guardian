import Foundation

// Models for the files guardiand and the guardian CLI write under ~/.guardian.
// Decoding is tolerant: a field that is missing or has an unexpected type falls back to its
// default, so the app keeps working across daemon versions.

struct DockerState: Codable, Sendable {
    var runningContainers: Int = 0
    var totalCpuPercent: Double = 0
    var totalMemoryMb: Int = 0

    enum CodingKeys: String, CodingKey {
        case runningContainers = "running_containers"
        case totalCpuPercent = "total_cpu_percent"
        case totalMemoryMb = "total_memory_mb"
    }

    init() {}

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        runningContainers = (try? c.decodeIfPresent(Int.self, forKey: .runningContainers)) ?? 0
        totalCpuPercent = (try? c.decodeIfPresent(Double.self, forKey: .totalCpuPercent)) ?? 0
        totalMemoryMb = (try? c.decodeIfPresent(Int.self, forKey: .totalMemoryMb)) ?? 0
    }
}

struct CursorState: Codable, Sendable {
    var activeSessions: Int = 0
    var processCount: Int = 0

    enum CodingKeys: String, CodingKey {
        case activeSessions = "active_sessions"
        case processCount = "process_count"
    }

    init() {}

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        activeSessions = (try? c.decodeIfPresent(Int.self, forKey: .activeSessions)) ?? 0
        processCount = (try? c.decodeIfPresent(Int.self, forKey: .processCount)) ?? 0
    }
}

enum PressureLevel: String, Codable, Sendable {
    case clear
    case strained
    case critical
}

/// Home-volume disk usage (`state.json` → `disk`).
struct DiskInfo: Codable, Sendable {
    var availableGb: Double = 0
    var usedPercent: Double = 0
    /// clear | warn | critical
    var level: String = "clear"

    enum CodingKeys: String, CodingKey {
        case availableGb = "available_gb"
        case usedPercent = "used_percent"
        case level
    }

    init() {}

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        availableGb = (try? c.decodeIfPresent(Double.self, forKey: .availableGb)) ?? 0
        usedPercent = (try? c.decodeIfPresent(Double.self, forKey: .usedPercent)) ?? 0
        level = (try? c.decodeIfPresent(String.self, forKey: .level)) ?? "clear"
    }
}

/// Resident memory per coding agent, in MB (`state.json` → `agents`).
struct AgentsInfo: Codable, Sendable {
    var cursorMb: Int = 0
    var claudeMb: Int = 0
    var codexMb: Int = 0
    var totalMb: Int = 0

    enum CodingKeys: String, CodingKey {
        case cursorMb = "cursor_mb"
        case claudeMb = "claude_mb"
        case codexMb = "codex_mb"
        case totalMb = "total_mb"
    }

    init() {}

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        cursorMb = (try? c.decodeIfPresent(Int.self, forKey: .cursorMb)) ?? 0
        claudeMb = (try? c.decodeIfPresent(Int.self, forKey: .claudeMb)) ?? 0
        codexMb = (try? c.decodeIfPresent(Int.self, forKey: .codexMb)) ?? 0
        totalMb = (try? c.decodeIfPresent(Int.self, forKey: .totalMb)) ?? 0
    }
}

struct GuardianStateData: Codable, Sendable {
    var pressure: PressureLevel = .clear
    /// What is driving the level, in plain words. Empty when clear.
    var pressureReasons: [String] = []
    /// Problems guardiand found in config.toml. Empty when the file is clean.
    var configProblems: [String] = []
    var cpuPercent: Double = 0
    var memoryAvailableGb: Double = 0
    var memoryTotalGb: Double = 0
    var swapUsedPercent: Double = 0
    /// The kernel's verdict: normal | warn | critical | unknown.
    var memoryPressureLevel: String = "unknown"
    var thermalState: String = "unknown"
    var docker: DockerState = DockerState()
    var cursor: CursorState = CursorState()
    var agents: AgentsInfo = AgentsInfo()
    var disk: DiskInfo = DiskInfo()
    var processCount: Int = 0
    var maxProcPerUid: Int = 0
    var sampledAt: String = ""

    enum CodingKeys: String, CodingKey {
        case pressure
        case pressureReasons = "pressure_reasons"
        case configProblems = "config_problems"
        case cpuPercent = "cpu_percent"
        case memoryAvailableGb = "memory_available_gb"
        case memoryTotalGb = "memory_total_gb"
        case swapUsedPercent = "swap_used_percent"
        case memoryPressureLevel = "memory_pressure_level"
        case thermalState = "thermal_state"
        case docker
        case cursor
        case agents
        case disk
        case processCount = "process_count"
        case maxProcPerUid = "max_proc_per_uid"
        case sampledAt = "sampled_at"
    }

    init() {}

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        pressure = (try? c.decodeIfPresent(PressureLevel.self, forKey: .pressure)) ?? .clear
        pressureReasons = (try? c.decodeIfPresent([String].self, forKey: .pressureReasons)) ?? []
        configProblems = (try? c.decodeIfPresent([String].self, forKey: .configProblems)) ?? []
        cpuPercent = (try? c.decodeIfPresent(Double.self, forKey: .cpuPercent)) ?? 0
        memoryAvailableGb = (try? c.decodeIfPresent(Double.self, forKey: .memoryAvailableGb)) ?? 0
        memoryTotalGb = (try? c.decodeIfPresent(Double.self, forKey: .memoryTotalGb)) ?? 0
        swapUsedPercent = (try? c.decodeIfPresent(Double.self, forKey: .swapUsedPercent)) ?? 0
        memoryPressureLevel = (try? c.decodeIfPresent(String.self, forKey: .memoryPressureLevel)) ?? "unknown"
        thermalState = (try? c.decodeIfPresent(String.self, forKey: .thermalState)) ?? "unknown"
        docker = (try? c.decodeIfPresent(DockerState.self, forKey: .docker)) ?? DockerState()
        cursor = (try? c.decodeIfPresent(CursorState.self, forKey: .cursor)) ?? CursorState()
        agents = (try? c.decodeIfPresent(AgentsInfo.self, forKey: .agents)) ?? AgentsInfo()
        disk = (try? c.decodeIfPresent(DiskInfo.self, forKey: .disk)) ?? DiskInfo()
        processCount = (try? c.decodeIfPresent(Int.self, forKey: .processCount)) ?? 0
        maxProcPerUid = (try? c.decodeIfPresent(Int.self, forKey: .maxProcPerUid)) ?? 0
        sampledAt = (try? c.decodeIfPresent(String.self, forKey: .sampledAt)) ?? ""
    }
}

/// One day of `~/.guardian/pressure_stats.json` (written by guardiand 0.8+).
struct DayStats: Codable, Sendable {
    var clearSecs: Double = 0
    var strainedSecs: Double = 0
    var criticalSecs: Double = 0
    var transitions: Int = 0
    var criticalEntries: Int = 0

    enum CodingKeys: String, CodingKey {
        case clearSecs = "clear_secs"
        case strainedSecs = "strained_secs"
        case criticalSecs = "critical_secs"
        case transitions
        case criticalEntries = "critical_entries"
    }

    init() {}

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        clearSecs = (try? c.decodeIfPresent(Double.self, forKey: .clearSecs)) ?? 0
        strainedSecs = (try? c.decodeIfPresent(Double.self, forKey: .strainedSecs)) ?? 0
        criticalSecs = (try? c.decodeIfPresent(Double.self, forKey: .criticalSecs)) ?? 0
        transitions = (try? c.decodeIfPresent(Int.self, forKey: .transitions)) ?? 0
        criticalEntries = (try? c.decodeIfPresent(Int.self, forKey: .criticalEntries)) ?? 0
    }

    var totalSecs: Double { clearSecs + strainedSecs + criticalSecs }

    func percent(_ secs: Double) -> Int {
        totalSecs > 0 ? Int((secs / totalSecs * 100).rounded()) : 0
    }
}

struct PressureStatsFile: Codable, Sendable {
    var days: [String: DayStats] = [:]
}

/// One row of `guardian disk --json`.
struct DiskTarget: Codable, Sendable, Identifiable {
    var id: String { path }
    var path: String = ""
    var displayPath: String = ""
    /// cache | build | worktree
    var kind: String = "cache"
    var what: String = ""
    var cost: String = ""
    var remove: String = ""
    /// regenerates | review
    var safety: String = "review"
    /// What it occupies on disk.
    var bytes: UInt64 = 0
    /// What deleting it would free: less than `bytes` when its files are clones of files
    /// elsewhere (a bun or pnpm install). Equal to `bytes` from a guardian before 0.12.
    var freedBytes: UInt64 = 0
    var complete: Bool = true
    /// Whole days since the project or worktree was last touched; nil for tool caches.
    var ageDays: Int?
    var repo: String?
    var branch: String?
    /// What the scan found, e.g. "3 uncommitted or untracked files".
    var note: String?
    /// True when `guardian disk delete` will remove it (it re-checks first).
    var deletable: Bool = false
    /// True for a worktree kept from deletion only by unsaved work that
    /// `guardian disk save --delete` can commit and push first.
    var saveable: Bool = false

    enum CodingKeys: String, CodingKey {
        case path
        case displayPath = "display_path"
        case kind, what, cost, remove, safety, bytes, complete
        case freedBytes = "freed_bytes"
        case ageDays = "age_days"
        case repo, branch, note, deletable, saveable
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        path = try c.decode(String.self, forKey: .path)
        displayPath = (try? c.decodeIfPresent(String.self, forKey: .displayPath)) ?? path
        kind = (try? c.decodeIfPresent(String.self, forKey: .kind)) ?? "cache"
        what = (try? c.decodeIfPresent(String.self, forKey: .what)) ?? ""
        cost = (try? c.decodeIfPresent(String.self, forKey: .cost)) ?? ""
        remove = (try? c.decodeIfPresent(String.self, forKey: .remove)) ?? ""
        safety = (try? c.decodeIfPresent(String.self, forKey: .safety)) ?? "review"
        bytes = (try? c.decodeIfPresent(UInt64.self, forKey: .bytes)) ?? 0
        freedBytes = (try? c.decodeIfPresent(UInt64.self, forKey: .freedBytes)) ?? bytes
        complete = (try? c.decodeIfPresent(Bool.self, forKey: .complete)) ?? true
        ageDays = try? c.decodeIfPresent(Int.self, forKey: .ageDays)
        repo = try? c.decodeIfPresent(String.self, forKey: .repo)
        branch = try? c.decodeIfPresent(String.self, forKey: .branch)
        note = try? c.decodeIfPresent(String.self, forKey: .note)
        deletable = (try? c.decodeIfPresent(Bool.self, forKey: .deletable)) ?? false
        saveable = (try? c.decodeIfPresent(Bool.self, forKey: .saveable)) ?? false
    }

    var regenerates: Bool { safety == "regenerates" }

    /// Untouched for this long counts as old (OLD_DAYS in src/disk_targets.rs).
    static let oldDays = 14

    /// Something Guardian can delete that comes back on its own and has gone untouched for
    /// `oldDays`: what `guardian disk clean` selects with `old`.
    var isOldAndSafe: Bool {
        deletable && regenerates && kind != "cache" && (ageDays ?? 0) >= Self.oldDays
    }

    /// `targets` without those inside another of them: deleting a worktree takes the build
    /// folders in it along, so they are not deleted, or counted, a second time.
    static func withoutNested(_ targets: [DiskTarget]) -> [DiskTarget] {
        targets.filter { inner in
            !targets.contains { $0.path != inner.path && inner.path.hasPrefix($0.path + "/") }
        }
    }

    /// What deleting it would free; `+` marks a lower bound.
    var sizeText: String {
        let text = Self.sizeText(freedBytes)
        return complete ? text : text + "+"
    }

    /// Said when a good part of what it occupies would not come back (same rule as the CLI).
    var sharedText: String? {
        let shared = bytes > freedBytes ? bytes - freedBytes : 0
        guard shared >= max(bytes / 10, 10 * 1_048_576) else { return nil }
        return "\(Self.sizeText(bytes)) in size, of which \(Self.sizeText(shared)) is shared with other copies and freed only when they go too"
    }

    static func sizeText(_ bytes: UInt64) -> String {
        let gb = Double(bytes) / 1_073_741_824
        return gb >= 1 ? String(format: "%.1f GB", gb) : String(format: "%.0f MB", Double(bytes) / 1_048_576)
    }

    var ageText: String? {
        guard let ageDays else { return nil }
        // A cache's date is when its top level last changed, not when it was last read.
        if kind == "cache" {
            switch ageDays {
            case 0: return "added to today"
            case 1: return "nothing new for 1 day"
            default: return "nothing new for \(ageDays) days"
            }
        }
        switch ageDays {
        case 0: return "used today"
        case 1: return "untouched 1 day"
        default: return "untouched \(ageDays) days"
        }
    }

    /// The line under the path: how stale it is and what was found, or what removal costs.
    var detailText: String {
        var parts: [String] = []
        if let ageText { parts.append(ageText) }
        if let note, !note.isEmpty {
            parts.append(note)
        } else if parts.isEmpty || kind == "cache" {
            parts.append(cost)
        }
        if let sharedText { parts.append(sharedText) }
        return parts.joined(separator: "; ")
    }
}

/// One bundle of `guardian disk --json`: the stale folders of one project (or the stale tool
/// caches), listed and deleted as one.
struct DiskBundle: Codable, Sendable, Identifiable {
    var id: String { staleness + "|" + name }
    /// The project as `~/…`, or "Tool caches".
    var name: String = ""
    var isProject: Bool = true
    /// quarter | month | fortnight | week
    var staleness: String = "week"
    /// The age band in words: "Untouched 30 to 89 days".
    var heading: String = ""
    /// How stale it is and how much of it can go without a look.
    var detail: String = ""
    /// "3 worktrees, 4 build folders"
    var contents: String = ""
    /// What deleting all of it would free.
    var freedBytes: UInt64 = 0
    /// What deleting `deletePaths` would free.
    var deletableFreedBytes: UInt64 = 0
    /// What can be deleted now without a look. Nothing here is inside anything else here.
    var deletePaths: [String] = []
    var heldBack: Int = 0
    var members: [DiskTarget] = []

    enum CodingKeys: String, CodingKey {
        case name, staleness, heading, detail, contents, members
        case isProject = "is_project"
        case freedBytes = "freed_bytes"
        case deletableFreedBytes = "deletable_freed_bytes"
        case deletePaths = "delete_paths"
        case heldBack = "held_back"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        name = try c.decode(String.self, forKey: .name)
        isProject = (try? c.decodeIfPresent(Bool.self, forKey: .isProject)) ?? true
        staleness = (try? c.decodeIfPresent(String.self, forKey: .staleness)) ?? "week"
        heading = (try? c.decodeIfPresent(String.self, forKey: .heading)) ?? ""
        detail = (try? c.decodeIfPresent(String.self, forKey: .detail)) ?? ""
        contents = (try? c.decodeIfPresent(String.self, forKey: .contents)) ?? ""
        freedBytes = (try? c.decodeIfPresent(UInt64.self, forKey: .freedBytes)) ?? 0
        deletableFreedBytes = (try? c.decodeIfPresent(UInt64.self, forKey: .deletableFreedBytes)) ?? 0
        deletePaths = (try? c.decodeIfPresent([String].self, forKey: .deletePaths)) ?? []
        heldBack = (try? c.decodeIfPresent(Int.self, forKey: .heldBack)) ?? 0
        members = (try? c.decodeIfPresent([DiskTarget].self, forKey: .members)) ?? []
    }

    /// Untouched for two weeks or more (OLD_DAYS in src/disk_targets.rs): what
    /// `guardian disk clean` selects with `old`, for projects.
    var isOld: Bool { ["quarter", "month", "fortnight"].contains(staleness) }
}

/// Output of `guardian disk --json`.
struct DiskReport: Codable, Sendable {
    /// Stale things grouped by project and age, the longest untouched first. Empty from a
    /// guardian that does not bundle.
    var bundles: [DiskBundle]
    /// Everything not in a bundle.
    var targets: [DiskTarget]
    let unmeasured: Int
    let reposScanned: Int
    /// What deleting every target of each kind would free.
    let regeneratesBytes: UInt64
    let reviewBytes: UInt64
    /// When the scan behind this report finished; nil from a guardian before 0.12.
    let scannedAt: Date?
    /// True when this is the saved scan read back rather than a new one.
    let fromCache: Bool

    enum CodingKeys: String, CodingKey {
        case bundles, targets, unmeasured
        case reposScanned = "repos_scanned"
        case regeneratesBytes = "regenerates_bytes"
        case reviewBytes = "review_bytes"
        case regeneratesFreedBytes = "regenerates_freed_bytes"
        case reviewFreedBytes = "review_freed_bytes"
        case scannedAt = "scanned_at"
        case fromCache = "from_cache"
    }

    func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        try c.encode(bundles, forKey: .bundles)
        try c.encode(targets, forKey: .targets)
        try c.encode(unmeasured, forKey: .unmeasured)
        try c.encode(reposScanned, forKey: .reposScanned)
        try c.encode(regeneratesBytes, forKey: .regeneratesFreedBytes)
        try c.encode(reviewBytes, forKey: .reviewFreedBytes)
        try c.encodeIfPresent(scannedAt.map { UInt64($0.timeIntervalSince1970) }, forKey: .scannedAt)
        try c.encode(fromCache, forKey: .fromCache)
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        bundles = (try? c.decodeIfPresent([DiskBundle].self, forKey: .bundles)) ?? []
        targets = (try? c.decodeIfPresent([DiskTarget].self, forKey: .targets)) ?? []
        unmeasured = (try? c.decodeIfPresent(Int.self, forKey: .unmeasured)) ?? 0
        reposScanned = (try? c.decodeIfPresent(Int.self, forKey: .reposScanned)) ?? 0
        // The freed totals where the CLI gives them (0.12 on), else what is on disk.
        regeneratesBytes = (try? c.decodeIfPresent(UInt64.self, forKey: .regeneratesFreedBytes))
            ?? (try? c.decodeIfPresent(UInt64.self, forKey: .regeneratesBytes)) ?? 0
        reviewBytes = (try? c.decodeIfPresent(UInt64.self, forKey: .reviewFreedBytes))
            ?? (try? c.decodeIfPresent(UInt64.self, forKey: .reviewBytes)) ?? 0
        scannedAt = (try? c.decodeIfPresent(UInt64.self, forKey: .scannedAt))
            .map { Date(timeIntervalSince1970: TimeInterval($0)) }
        fromCache = (try? c.decodeIfPresent(Bool.self, forKey: .fromCache)) ?? false
    }
}

/// What `guardian disk save` would do to a worktree (`--plan --json`).
struct SavePlan: Codable, Sendable {
    let branch: String
    /// True when `branch` is created for the save: HEAD is detached, or it was on a shared
    /// branch (`fromBranch`) that work in progress is not pushed to.
    let newBranch: Bool
    let fromBranch: String?
    /// Files that will be committed.
    let files: [String]
    let remote: String
    let remoteBranch: String
    /// False when the branch is not on the remote yet.
    let hasUpstream: Bool
    /// Ignored files kept nowhere else: not saved, and deleted with the worktree.
    let ignored: [String]

    enum CodingKeys: String, CodingKey {
        case branch, files, remote, ignored
        case newBranch = "new_branch"
        case fromBranch = "from_branch"
        case remoteBranch = "remote_branch"
        case hasUpstream = "has_upstream"
    }
}

/// One element of `guardian disk save --plan --json`.
struct SavePlanResult: Codable, Sendable {
    let path: String
    let ok: Bool
    let plan: SavePlan?
    let error: String?
}

/// One element of `guardian disk save --delete --yes --json`.
struct SaveResult: Codable, Sendable {
    let path: String
    let ok: Bool
    let saved: Bool?
    let deleted: Bool?
    let branch: String?
    let committedFiles: Int?
    let pushedTo: String?
    let freedBytes: UInt64?
    let error: String?

    enum CodingKeys: String, CodingKey {
        case path, ok, saved, deleted, branch, error
        case committedFiles = "committed_files"
        case pushedTo = "pushed_to"
        case freedBytes = "freed_bytes"
    }
}

/// One element of `guardian disk delete --yes --json`.
struct DeleteResult: Codable, Sendable {
    let path: String
    let ok: Bool
    let freedBytes: UInt64?
    let error: String?

    enum CodingKeys: String, CodingKey {
        case path, ok, error
        case freedBytes = "freed_bytes"
    }
}
