import AppKit
import SwiftUI

/// `Guardian --render-panel <file.png> [--with-disk]` draws the menu bar panel to a PNG from
/// the live ~/.guardian state and exits. It exists so the panel can be checked (and
/// screenshotted for docs) without clicking through the menu bar.
@MainActor
enum PanelSnapshot {
    /// The output path when the process was started in snapshot mode.
    static var requestedPath: String? {
        let args = CommandLine.arguments
        guard let flag = args.firstIndex(of: "--render-panel"), args.count > flag + 1 else { return nil }
        return args[flag + 1]
    }

    static func render(to path: String, client: GuardianClient) -> Bool {
        client.refreshNow()
        if CommandLine.arguments.contains("--with-disk") {
            client.scanDisk()
            while client.diskScanning {
                RunLoop.main.run(until: Date().addingTimeInterval(0.1))
            }
        }

        let panel = MenuBarView()
            .environmentObject(client)
            .background(Color(nsColor: .windowBackgroundColor))
        let view = NSHostingView(rootView: panel)
        view.frame = NSRect(origin: .zero, size: view.fittingSize)
        let window = NSWindow(contentRect: view.frame, styleMask: [.borderless], backing: .buffered, defer: false)
        window.contentView = view
        view.layoutSubtreeIfNeeded()
        // Give SwiftUI a moment to finish its first layout pass.
        RunLoop.main.run(until: Date().addingTimeInterval(0.4))
        view.frame = NSRect(origin: .zero, size: view.fittingSize)
        view.layoutSubtreeIfNeeded()

        guard let bitmap = view.bitmapImageRepForCachingDisplay(in: view.bounds) else { return false }
        view.cacheDisplay(in: view.bounds, to: bitmap)
        guard let png = bitmap.representation(using: .png, properties: [:]) else { return false }
        do {
            try png.write(to: URL(fileURLWithPath: path))
            return true
        } catch {
            return false
        }
    }
}
