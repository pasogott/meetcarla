// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "CarlaNativeHelper",
    platforms: [.macOS(.v14)],
    products: [
        .executable(name: "CarlaNativeHelper", targets: ["CarlaNativeHelper"])
    ],
    targets: [
        .executableTarget(
            name: "CarlaNativeHelper",
            path: "Sources/CarlaNativeHelper"
        )
    ]
)
