// swift-tools-version: 5.9
import PackageDescription
import Foundation

let repoRoot = URL(fileURLWithPath: #filePath).deletingLastPathComponent().path
let swiftSDKRelativePath = "sdk/swift"
let ffiXCFrameworkRelativePath = "\(swiftSDKRelativePath)/Generated/MeshLLMFFI.xcframework"
let ffiXCFrameworkPath = "\(repoRoot)/\(ffiXCFrameworkRelativePath)"
let remoteFFIXCFrameworkURL = "https://github.com/Mesh-LLM/mesh-llm/releases/download/v0.65.0/MeshLLMFFI.xcframework.zip"
let remoteFFIXCFrameworkChecksum = "b336ff65204fc2a9fadf1a237301f71fc5744a085b21da1245401e972381a2d7"
let forceStubFFI = ProcessInfo.processInfo.environment["MESH_SWIFT_FORCE_STUB"] == "1"
let hasLocalFFIXCFramework = FileManager.default.fileExists(atPath: ffiXCFrameworkPath)
let hasRemoteFFIXCFramework = !forceStubFFI
    && !remoteFFIXCFrameworkURL.contains("__MESH_SWIFT_RELEASE_TAG__")
    && !remoteFFIXCFrameworkChecksum.contains("__MESH_SWIFT_RELEASE_CHECKSUM__")

var meshLLMDependencies: [Target.Dependency] = []
var packageTargets: [Target] = []

if hasLocalFFIXCFramework {
    meshLLMDependencies.append("MeshLLMFFI")
    packageTargets.append(
        .binaryTarget(
            name: "MeshLLMFFI",
            path: ffiXCFrameworkRelativePath
        )
    )
} else if hasRemoteFFIXCFramework {
    meshLLMDependencies.append("MeshLLMFFI")
    packageTargets.append(
        .binaryTarget(
            name: "MeshLLMFFI",
            url: remoteFFIXCFrameworkURL,
            checksum: remoteFFIXCFrameworkChecksum
        )
    )
}

let hasFFIBinaryTarget = hasLocalFFIXCFramework || hasRemoteFFIXCFramework

let package = Package(
    name: "MeshLLM",
    platforms: [
        .iOS(.v16),
        .macOS(.v13),
    ],
    products: [
        .library(
            name: "MeshLLM",
            targets: ["MeshLLM"]
        ),
    ],
    targets: [
        .target(
            name: "MeshLLM",
            dependencies: meshLLMDependencies,
            path: "sdk/swift/Sources/MeshLLM",
            exclude: hasFFIBinaryTarget ? [] : ["Generated"],
            linkerSettings: [
                .linkedFramework("SystemConfiguration"),
            ]
        ),
        .testTarget(
            name: "MeshLLMTests",
            dependencies: ["MeshLLM"],
            path: "sdk/swift/Tests/MeshLLMTests"
        ),
    ] + packageTargets
)
