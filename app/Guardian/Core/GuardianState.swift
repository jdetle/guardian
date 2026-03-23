import Foundation

struct DockerState: Codable, Sendable {
    var runningContainers: Int = 0
    var totalCpuPercent: Double = 0
    var totalMemoryMb: Int = 0

    enum CodingKeys: String, CodingKey {
        case runningContainers = "running_containers"
        case totalCpuPercent = "total_cpu_percent"
        case totalMemoryMb = "total_memory_mb"
    }
}

struct CursorState: Codable, Sendable {
    var activeSessions: Int = 0
    var processCount: Int = 0

    enum CodingKeys: String, CodingKey {
        case activeSessions = "active_sessions"
        case processCount = "process_count"
    }
}

enum PressureLevel: String, Codable, Sendable {
    case clear
    case strained
    case critical
}

struct GuardianStateData: Codable, Sendable {
    var pressure: PressureLevel = .clear
    var cpuPercent: Double = 0
    var memoryAvailableGb: Double = 0
    var memoryTotalGb: Double = 0
    var swapUsedPercent: Double = 0
    var thermalState: String = "unknown"
    var docker: DockerState = DockerState()
    var cursor: CursorState = CursorState()
    var processCount: Int = 0
    var maxProcPerUid: Int = 0
    var sampledAt: String = ""

    enum CodingKeys: String, CodingKey {
        case pressure
        case cpuPercent = "cpu_percent"
        case memoryAvailableGb = "memory_available_gb"
        case memoryTotalGb = "memory_total_gb"
        case swapUsedPercent = "swap_used_percent"
        case thermalState = "thermal_state"
        case docker
        case cursor
        case processCount = "process_count"
        case maxProcPerUid = "max_proc_per_uid"
        case sampledAt = "sampled_at"
    }
}
