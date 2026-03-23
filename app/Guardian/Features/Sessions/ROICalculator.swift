import Foundation

/// Estimates the Return on Investment of a Cursor agent session.
///
/// Five weighted signals:
/// - Session outcome (completed vs error/aborted): 30%
/// - Tool call count (proxy for work done): 25%
/// - Efficiency (tool calls per minute): 25%
/// - Resource cost (inverse of avg CPU during session): 20%
struct ROICalculator {
    struct Input {
        let status: String?
        let toolCallCount: Int
        let durationMs: Int
        let avgCpuPercent: Double
        let filesModified: Int
        let linesChanged: Int
    }

    struct Score {
        let overall: Double
        let statusScore: Double
        let workScore: Double
        let efficiencyScore: Double
        let resourceScore: Double
    }

    static func compute(_ input: Input) -> Score {
        let statusScore = statusComponent(input.status)
        let workScore = workComponent(input.toolCallCount, files: input.filesModified, lines: input.linesChanged)
        let efficiencyScore = efficiencyComponent(toolCalls: input.toolCallCount, durationMs: input.durationMs)
        let resourceScore = resourceComponent(avgCpu: input.avgCpuPercent)

        let overall = statusScore * 0.30
            + workScore * 0.25
            + efficiencyScore * 0.25
            + resourceScore * 0.20

        return Score(
            overall: min(max(overall, 0), 100),
            statusScore: statusScore,
            workScore: workScore,
            efficiencyScore: efficiencyScore,
            resourceScore: resourceScore
        )
    }

    /// 100 for completed, 30 for aborted, 10 for error
    private static func statusComponent(_ status: String?) -> Double {
        switch status {
        case "completed": return 100
        case "aborted": return 30
        case "error": return 10
        default: return 0
        }
    }

    /// Higher tool calls + file changes = more value delivered.
    /// Capped at 50 tool calls = 100 or 20+ files = 100.
    private static func workComponent(_ toolCalls: Int, files: Int, lines: Int) -> Double {
        let toolScore = min(Double(toolCalls) * 2.0, 100)
        let fileScore = min(Double(files) * 5.0, 100)
        let lineScore = min(Double(lines) / 10.0, 100)
        return max(toolScore, max(fileScore, lineScore))
    }

    /// Tool calls per minute. 10 calls/min = perfect efficiency.
    private static func efficiencyComponent(toolCalls: Int, durationMs: Int) -> Double {
        guard durationMs > 0 else { return 50 }
        let minutes = Double(durationMs) / 60_000.0
        guard minutes > 0 else { return 50 }
        let callsPerMin = Double(toolCalls) / minutes
        return min(callsPerMin * 10.0, 100)
    }

    /// Lower average CPU = higher score (session was efficient with resources).
    private static func resourceComponent(avgCpu: Double) -> Double {
        max(100.0 - avgCpu, 0)
    }
}
