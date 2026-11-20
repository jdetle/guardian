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
