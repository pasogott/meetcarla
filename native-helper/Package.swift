// swift-tools-version: 5.10
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
