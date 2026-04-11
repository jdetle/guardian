import Foundation

/// Opens Cursor like `open -a Cursor [path]` so the user lands in the repo for a queued job.
enum CursorWorkspaceOpener {
    static func open(workspacePath: String?) {
        let task = Process()
        task.executableURL = URL(fileURLWithPath: "/usr/bin/open")
        if let raw = workspacePath?.trimmingCharacters(in: .whitespacesAndNewlines), !raw.isEmpty {
            let path = (raw as NSString).expandingTildeInPath
            if FileManager.default.fileExists(atPath: path) {
                task.arguments = ["-a", "Cursor", path]
            } else {
                task.arguments = ["-a", "Cursor"]
            }
        } else {
            task.arguments = ["-a", "Cursor"]
        }
        task.standardOutput = FileHandle.nullDevice
        task.standardError = FileHandle.nullDevice
        try? task.run()
    }
}
