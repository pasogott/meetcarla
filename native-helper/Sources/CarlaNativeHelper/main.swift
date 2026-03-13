import AppKit
import AVFoundation
import CoreGraphics
import CoreMedia
import Foundation
@preconcurrency import ScreenCaptureKit

struct Envelope: Codable {
    let command: String
}

struct Response: Codable {
    let status: String
    let message: String?
    let microphone: Bool?
    let screenRecording: Bool?
}

struct AudioDeviceInfo: Codable {
    let id: String
    let name: String
    let isDefault: Bool
    let isInput: Bool
}

struct AudioDevicesResponse: Codable {
    let status: String
    let devices: [AudioDeviceInfo]
}

final class PermissionBox: @unchecked Sendable {
    var granted = false
}

final class DisplayBox: @unchecked Sendable {
    var result: Result<SCDisplay, Error>?
}

final class AppBox: @unchecked Sendable {
    var app: SCRunningApplication?
}

final class ErrorBox: @unchecked Sendable {
    var error: Error?
}

final class MicrophoneChunkWriter: @unchecked Sendable {
    let chunkDirectoryURL: URL
    let chunkDurationSeconds: Double
    var baseSeconds: Double?
    var currentChunkIndex: Int?
    var currentFile: AVAudioFile?

    init(chunkDirectoryURL: URL, chunkDurationSeconds: Double = 5.0) {
        self.chunkDirectoryURL = chunkDirectoryURL
        self.chunkDurationSeconds = chunkDurationSeconds
    }

    func append(_ sampleBuffer: CMSampleBuffer) throws {
        guard CMSampleBufferIsValid(sampleBuffer) else {
            return
        }
        let time = CMSampleBufferGetPresentationTimeStamp(sampleBuffer)
        let seconds = CMTimeGetSeconds(time)
        if baseSeconds == nil {
            baseSeconds = seconds
        }
        let relativeSeconds = max(seconds - (baseSeconds ?? 0), 0)
        let chunkIndex = max(Int(relativeSeconds / chunkDurationSeconds), 0)
        let pcmBuffer = try makePCMBuffer(from: sampleBuffer)

        if currentChunkIndex != chunkIndex {
            try rotate(to: chunkIndex, format: pcmBuffer.format)
        }

        try currentFile?.write(from: pcmBuffer)
    }

    func finalize() {
        if let currentChunkIndex {
            currentFile = nil
            writeDoneMarker(for: currentChunkIndex)
            self.currentChunkIndex = nil
        }
    }

    private func rotate(to chunkIndex: Int, format: AVAudioFormat) throws {
        if let currentChunkIndex {
            currentFile = nil
            writeDoneMarker(for: currentChunkIndex)
        }

        let chunkURL = chunkDirectoryURL.appendingPathComponent(
            String(format: "chunk-%06d.mic.caf", chunkIndex)
        )
        try FileManager.default.createDirectory(
            at: chunkDirectoryURL,
            withIntermediateDirectories: true
        )
        currentFile = try AVAudioFile(
            forWriting: chunkURL,
            settings: format.settings,
            commonFormat: format.commonFormat,
            interleaved: format.isInterleaved
        )
        currentChunkIndex = chunkIndex
    }

    private func writeDoneMarker(for chunkIndex: Int) {
        let doneURL = chunkDirectoryURL.appendingPathComponent(
            String(format: "chunk-%06d.done", chunkIndex)
        )
        FileManager.default.createFile(atPath: doneURL.path, contents: Data())
    }
}

let encoder = JSONEncoder()
encoder.outputFormatting = [.withoutEscapingSlashes]

func cliArgument(named name: String) -> String? {
    guard let index = CommandLine.arguments.firstIndex(of: name), index + 1 < CommandLine.arguments.count else {
        return nil
    }
    return CommandLine.arguments[index + 1]
}

if CommandLine.arguments.contains("--ping") {
    print("pong")
    exit(0)
}

@available(macOS 15.0, *)
final class MeetingRecordingDelegate: NSObject, SCStreamDelegate, SCRecordingOutputDelegate, SCStreamOutput {
    let stopURL: URL
    let microphoneChunkWriter: MicrophoneChunkWriter
    var streamError: Error?
    var recordingError: Error?

    init(stopURL: URL, chunkDirectoryURL: URL) {
        self.stopURL = stopURL
        self.microphoneChunkWriter = MicrophoneChunkWriter(chunkDirectoryURL: chunkDirectoryURL)
    }

    func stream(_ stream: SCStream, didStopWithError error: Error) {
        streamError = error
    }

    func recordingOutput(_ recordingOutput: SCRecordingOutput, didFailWithError error: Error) {
        recordingError = error
    }

    func stream(
        _ stream: SCStream,
        didOutputSampleBuffer sampleBuffer: CMSampleBuffer,
        of outputType: SCStreamOutputType
    ) {
        guard outputType == .microphone else {
            return
        }
        do {
            try microphoneChunkWriter.append(sampleBuffer)
        } catch {
            streamError = error
        }
    }
}

func makePCMBuffer(from sampleBuffer: CMSampleBuffer) throws -> AVAudioPCMBuffer {
    guard let formatDescription = CMSampleBufferGetFormatDescription(sampleBuffer),
          let streamDescription = CMAudioFormatDescriptionGetStreamBasicDescription(formatDescription)
    else {
        throw NSError(domain: "CarlaNativeHelper", code: 2, userInfo: [
            NSLocalizedDescriptionKey: "Could not read audio stream description."
        ])
    }

    let format = AVAudioFormat(streamDescription: streamDescription)!
    let frameCount = AVAudioFrameCount(CMSampleBufferGetNumSamples(sampleBuffer))
    guard let pcmBuffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: frameCount) else {
        throw NSError(domain: "CarlaNativeHelper", code: 3, userInfo: [
            NSLocalizedDescriptionKey: "Could not allocate audio buffer."
        ])
    }
    pcmBuffer.frameLength = frameCount

    let status = CMSampleBufferCopyPCMDataIntoAudioBufferList(
        sampleBuffer,
        at: 0,
        frameCount: Int32(frameCount),
        into: pcmBuffer.mutableAudioBufferList
    )
    guard status == noErr else {
        throw NSError(domain: NSOSStatusErrorDomain, code: Int(status), userInfo: [
            NSLocalizedDescriptionKey: "Could not copy microphone samples into a PCM buffer."
        ])
    }

    return pcmBuffer
}

func currentProcessApplication(from content: SCShareableContent) -> SCRunningApplication? {
    content.applications.first { $0.processID == ProcessInfo.processInfo.processIdentifier }
}

func firstDisplay() throws -> SCDisplay {
    let semaphore = DispatchSemaphore(value: 0)
    let box = DisplayBox()
    SCShareableContent.getExcludingDesktopWindows(false, onScreenWindowsOnly: true) { content, error in
        if let error {
            box.result = .failure(error)
        } else if let display = content?.displays.first {
            box.result = .success(display)
        } else {
            box.result = .failure(NSError(domain: "CarlaNativeHelper", code: 1, userInfo: [
                NSLocalizedDescriptionKey: "No shareable display is available for ScreenCaptureKit."
            ]))
        }
        semaphore.signal()
    }
    semaphore.wait()
    return try box.result!.get()
}

func currentProcessExclusionApplication() -> SCRunningApplication? {
    let semaphore = DispatchSemaphore(value: 0)
    let box = AppBox()
    SCShareableContent.getExcludingDesktopWindows(false, onScreenWindowsOnly: true) { content, _ in
        if let content {
            box.app = currentProcessApplication(from: content)
        }
        semaphore.signal()
    }
    semaphore.wait()
    return box.app
}

@available(macOS 15.0, *)
func recordMeeting(output: String, stopFile: String, chunkDirectory: String, deviceId: String?) throws -> Never {
    guard microphoneGranted() else {
        FileHandle.standardError.write(Data("Microphone permission not granted\n".utf8))
        exit(1)
    }
    guard screenRecordingGranted() else {
        FileHandle.standardError.write(Data("Screen recording permission not granted\n".utf8))
        exit(1)
    }

    let outputURL = URL(fileURLWithPath: output)
    let stopURL = URL(fileURLWithPath: stopFile)
    let chunkDirectoryURL = URL(fileURLWithPath: chunkDirectory, isDirectory: true)
    try? FileManager.default.removeItem(at: stopURL)
    try FileManager.default.createDirectory(
        at: outputURL.deletingLastPathComponent(),
        withIntermediateDirectories: true
    )
    try FileManager.default.createDirectory(
        at: chunkDirectoryURL,
        withIntermediateDirectories: true
    )

    let display = try firstDisplay()
    let excludedApp = currentProcessExclusionApplication()
    let filter = SCContentFilter(
        display: display,
        excludingApplications: excludedApp.map { [$0] } ?? [],
        exceptingWindows: []
    )

    let configuration = SCStreamConfiguration()
    configuration.width = 2
    configuration.height = 2
    configuration.minimumFrameInterval = CMTime(value: 1, timescale: 1)
    configuration.showsCursor = false
    configuration.capturesAudio = true
    configuration.captureMicrophone = true
    configuration.excludesCurrentProcessAudio = true
    configuration.sampleRate = 48_000
    configuration.channelCount = 2
    configuration.queueDepth = 3
    if let deviceId = deviceId, let device = AVCaptureDevice(uniqueID: deviceId) {
        configuration.microphoneCaptureDeviceID = device.uniqueID
    }

    let delegate = MeetingRecordingDelegate(stopURL: stopURL, chunkDirectoryURL: chunkDirectoryURL)
    let stream = SCStream(filter: filter, configuration: configuration, delegate: delegate)
    let microphoneQueue = DispatchQueue(label: "CarlaNativeHelper.microphone")

    let recordingConfiguration = SCRecordingOutputConfiguration()
    recordingConfiguration.outputURL = outputURL
    recordingConfiguration.outputFileType = .mp4
    let recordingOutput = SCRecordingOutput(configuration: recordingConfiguration, delegate: delegate)

    try stream.addRecordingOutput(recordingOutput)
    try stream.addStreamOutput(delegate, type: .microphone, sampleHandlerQueue: microphoneQueue)

    let startSemaphore = DispatchSemaphore(value: 0)
    let startError = ErrorBox()
    stream.startCapture { error in
        startError.error = error
        startSemaphore.signal()
    }
    startSemaphore.wait()
    if let startError = startError.error {
        throw startError
    }

    while !FileManager.default.fileExists(atPath: stopURL.path) {
        RunLoop.current.run(mode: .default, before: Date(timeIntervalSinceNow: 0.2))
        if let error = delegate.streamError ?? delegate.recordingError {
            throw error
        }
    }

    let stopSemaphore = DispatchSemaphore(value: 0)
    let stopError = ErrorBox()
    stream.stopCapture { error in
        stopError.error = error
        stopSemaphore.signal()
    }
    stopSemaphore.wait()
    try? FileManager.default.removeItem(at: stopURL)

    if let stopError = stopError.error {
        throw stopError
    }
    delegate.microphoneChunkWriter.finalize()
    if let error = delegate.streamError ?? delegate.recordingError {
        throw error
    }
    exit(0)
}

if CommandLine.arguments.contains("record-meeting") {
    guard let output = cliArgument(named: "--output"),
          let stopFile = cliArgument(named: "--stop-file"),
          let chunkDirectory = cliArgument(named: "--chunk-dir")
    else {
        FileHandle.standardError.write(Data("Missing --output, --stop-file, or --chunk-dir\n".utf8))
        exit(1)
    }
    let deviceId = cliArgument(named: "--device-id")
    if #available(macOS 15.0, *) {
        do {
            try recordMeeting(output: output, stopFile: stopFile, chunkDirectory: chunkDirectory, deviceId: deviceId)
        } catch {
            FileHandle.standardError.write(Data("\(error.localizedDescription)\n".utf8))
            exit(1)
        }
    } else {
        FileHandle.standardError.write(Data("System audio capture requires macOS 15 or newer\n".utf8))
        exit(1)
    }
}

let handle = FileHandle.standardInput
let data = handle.readDataToEndOfFile()

if data.isEmpty {
    let response = Response(
        status: "ok",
        message: "CarlaNativeHelper ready",
        microphone: nil,
        screenRecording: nil
    )
    let payload = try encoder.encode(response)
    FileHandle.standardOutput.write(payload)
    exit(0)
}

func write(_ response: Response, exitCode: Int32 = 0) -> Never {
    do {
        let payload = try encoder.encode(response)
        FileHandle.standardOutput.write(payload)
        exit(exitCode)
    } catch {
        FileHandle.standardError.write(Data("Encoding failed\n".utf8))
        exit(1)
    }
}

func writeDevices(_ response: AudioDevicesResponse, exitCode: Int32 = 0) -> Never {
    do {
        let payload = try encoder.encode(response)
        FileHandle.standardOutput.write(payload)
        exit(exitCode)
    } catch {
        FileHandle.standardError.write(Data("Encoding failed\n".utf8))
        exit(1)
    }
}

func listAudioDevices() -> AudioDevicesResponse {
    let discoverySession = AVCaptureDevice.DiscoverySession(
        deviceTypes: [.microphone, .external],
        mediaType: .audio,
        position: .unspecified
    )
    let defaultDevice = AVCaptureDevice.default(for: .audio)
    let devices = discoverySession.devices.map { device in
        AudioDeviceInfo(
            id: device.uniqueID,
            name: device.localizedName,
            isDefault: device.uniqueID == defaultDevice?.uniqueID,
            isInput: true
        )
    }
    return AudioDevicesResponse(status: "ok", devices: devices)
}

func microphoneGranted() -> Bool {
    AVCaptureDevice.authorizationStatus(for: .audio) == .authorized
}

func screenRecordingGranted() -> Bool {
    CGPreflightScreenCaptureAccess()
}

func currentPermissions(message: String? = nil) -> Response {
    Response(
        status: "ok",
        message: message,
        microphone: microphoneGranted(),
        screenRecording: screenRecordingGranted()
    )
}

do {
    let envelope = try JSONDecoder().decode(Envelope.self, from: data)
    switch envelope.command {
    case "list_audio_devices":
        writeDevices(listAudioDevices())
    case "check_permissions":
        write(currentPermissions())
    case "request_microphone_permission":
        switch AVCaptureDevice.authorizationStatus(for: .audio) {
        case .authorized:
            write(currentPermissions(message: "Microphone access already granted."))
        case .denied, .restricted:
            write(currentPermissions(message: "Microphone access is blocked in macOS settings."))
        case .notDetermined:
            let semaphore = DispatchSemaphore(value: 0)
            let box = PermissionBox()
            AVCaptureDevice.requestAccess(for: .audio) { allowed in
                box.granted = allowed
                semaphore.signal()
            }
            _ = semaphore.wait(timeout: .now() + 30)
            write(
                currentPermissions(
                    message: box.granted
                        ? "Microphone access granted."
                        : "Microphone access was not granted."
                )
            )
        @unknown default:
            write(currentPermissions(message: "Unknown microphone authorization state."))
        }
    case "request_screen_recording_permission":
        let granted = CGRequestScreenCaptureAccess()
        write(
            currentPermissions(
                message: granted
                    ? "Screen recording access granted."
                    : "Screen recording access was not granted."
            )
        )
    case "open_system_settings":
        let urls = [
            "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture",
            "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone",
            "x-apple.systempreferences:com.apple.preference.security"
        ]
        let opened = urls
            .compactMap(URL.init(string:))
            .contains { NSWorkspace.shared.open($0) }
        write(
            Response(
                status: opened ? "ok" : "error",
                message: opened
                    ? "Opened macOS privacy settings."
                    : "Could not open macOS privacy settings.",
                microphone: nil,
                screenRecording: nil
            ),
            exitCode: opened ? 0 : 1
        )
    default:
        write(
            Response(
                status: "error",
                message: "Unsupported helper command: \(envelope.command)",
                microphone: nil,
                screenRecording: nil
            ),
            exitCode: 1
        )
    }
} catch {
    FileHandle.standardError.write(Data("Invalid request\n".utf8))
    exit(1)
}
