// swift-tools-version: 5.9
import PackageDescription

let package = Package(
    name: "Guardian",
    platforms: [.macOS(.v14)],
    targets: [
        .executableTarget(
            name: "Guardian",
            path: "Guardian"
        ),
    ]
)
