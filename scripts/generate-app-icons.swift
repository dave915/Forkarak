#!/usr/bin/env swift
// macOS 13+ flattened fallbacks from Apple's real Icon Composer exports.
// Shipping assets change only with --write-production; originals stay in sources/.
import AppKit
import CoreGraphics
import Foundation
import ImageIO
import UniformTypeIdentifiers

let root = URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent()
let icons = root.appendingPathComponent("assets/icons")
let app = icons.appendingPathComponent("app")
let names = ["charcoal", "deep-blue", "deep-green", "plum", "ivory", "light-blue"]
let labels = ["차콜", "딥 블루", "딥 그린", "플럼", "라이트 아이보리", "라이트 블루"]
let representations = [(16, 1), (16, 2), (32, 1), (32, 2), (128, 1), (128, 2), (256, 1), (256, 2), (512, 1), (512, 2)]
let colorSpace = CGColorSpace(name: CGColorSpace.sRGB)!
var sharedCoverage: [Int: [UInt8]] = [:]
let arguments = Array(CommandLine.arguments.dropFirst())
struct Failure: Error, CustomStringConvertible {
    let description: String
    init(_ message: String) { description = message }
}
func value(_ flag: String) throws -> URL? {
    guard let index = arguments.firstIndex(of: flag) else { return nil }
    guard index + 1 < arguments.count, !arguments[index + 1].hasPrefix("--") else { throw Failure("Missing value: \(flag)") }
    return URL(fileURLWithPath: arguments[index + 1])
}
func context(_ size: Int) -> CGContext {
    CGContext(data: nil, width: size, height: size, bitsPerComponent: 8, bytesPerRow: size * 4,
              space: colorSpace, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
}
func read(_ url: URL) throws -> CGImage {
    guard let source = CGImageSourceCreateWithURL(url as CFURL, nil), let image = CGImageSourceCreateImageAtIndex(source, 0, nil) else {
        throw Failure("Cannot decode \(url.path)")
    }
    return image
}
func write(_ image: CGImage, _ url: URL) throws {
    try FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
    guard let destination = CGImageDestinationCreateWithURL(url as CFURL, UTType.png.identifier as CFString, 1, nil) else { throw Failure("Cannot write \(url.path)") }
    CGImageDestinationAddImage(destination, image, nil)
    guard CGImageDestinationFinalize(destination) else { throw Failure("PNG encoding failed") }
}
func resized(_ image: CGImage, _ size: Int, framed: Bool = false) -> CGImage {
    let c = context(size)
    c.interpolationQuality = .high
    // The system encloses native .icon artwork. ICNS needs transparent padding.
    let inset = framed ? CGFloat(size) * 80 / 1024 : 0
    c.draw(image, in: CGRect(x: inset, y: inset, width: CGFloat(size) - inset * 2, height: CGFloat(size) - inset * 2))
    if framed {
        let bytes = c.data!.assumingMemoryBound(to: UInt8.self)
        // CoreGraphics can round edge alpha differently for light RGB values.
        // Retain the first (charcoal) Apple enclosure coverage for every color.
        if sharedCoverage[size] == nil { sharedCoverage[size] = (0..<(size * size)).map { bytes[$0 * 4 + 3] } }
        for (pixel, coverage) in sharedCoverage[size]!.enumerated() {
            let offset = pixel * 4
            let previous = Int(bytes[offset + 3])
            for channel in 0..<3 {
                bytes[offset + channel] = previous == 0 ? 0 : UInt8(min(255, (Int(bytes[offset + channel]) * Int(coverage) + previous / 2) / previous))
            }
            bytes[offset + 3] = coverage
        }
    }
    return c.makeImage()!
}
func run(_ executable: String, _ args: [String]) throws {
    let p = Process()
    p.executableURL = URL(fileURLWithPath: executable)
    p.arguments = args
    try p.run()
    p.waitUntilExit()
    guard p.terminationStatus == 0 else { throw Failure("\(executable) failed") }
}
func contactSheet(_ url: URL, _ dark: Bool, _ overview: Bool) throws {
    let width = overview ? 1200 : 960
    let height = overview ? 276 : 430
    let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: width, pixelsHigh: height,
                              bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
                              colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
    NSGraphicsContext.saveGraphicsState()
    NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
    (dark ? NSColor(srgbRed: 0.11, green: 0.13, blue: 0.16, alpha: 1) : NSColor(srgbRed: 0.95, green: 0.96, blue: 0.97, alpha: 1)).setFill()
    NSRect(x: 0, y: 0, width: width, height: height).fill()
    for (column, name) in names.enumerated() {
        let cg = try read(app.appendingPathComponent("\(name).png"))
        let ns = NSImage(cgImage: cg, size: NSSize(width: 1024, height: 1024))
        let sizes: [Int] = overview ? [160] : [32, 64, 128]
        for (row, size) in sizes.enumerated() {
            let cell = width / 6
            ns.draw(in: NSRect(x: CGFloat(column * cell + (cell - size) / 2), y: CGFloat(overview ? 80 : 340 - row * 145), width: CGFloat(size), height: CGFloat(size)))
        }
        let text = labels[column] as NSString
        let style: [NSAttributedString.Key: Any] = [.font: NSFont.systemFont(ofSize: overview ? 20 : 15), .foregroundColor: dark ? NSColor.white : NSColor(srgbRed: 0.13, green: 0.17, blue: 0.22, alpha: 1)]
        let textSize = text.size(withAttributes: style)
        text.draw(at: NSPoint(x: CGFloat(column * width / 6 + width / 12) - textSize.width / 2, y: overview ? 32 : 8), withAttributes: style)
    }
    NSGraphicsContext.restoreGraphicsState()
    guard let cg = rep.cgImage else { throw Failure("Cannot render contact sheet") }
    try write(cg, url)
}

do {
    let writeProduction = arguments.contains("--write-production")
    let qa = try value("--qa-dir")
    let renderDir = try value("--render-dir") ?? app.appendingPathComponent("liquid-exports")
    let known = Set(["--write-production", "--qa-dir", "--render-dir", "--readme-sheet"])
    for arg in arguments where arg.hasPrefix("--") && !known.contains(arg) { throw Failure("Unknown argument: \(arg)") }
    guard writeProduction || qa != nil else { throw Failure("Read-only by default. Use --qa-dir PATH to review or --write-production to explicitly regenerate shipping fallbacks.") }
    if writeProduction {
        try run("/usr/bin/env", ["python3", root.appendingPathComponent("scripts/generate-liquid-icons.py").path, "--verify-renders", renderDir.path])
        let exports = try names.map { try read(renderDir.appendingPathComponent("\($0).png")) }
        guard exports.allSatisfy({ $0.width == 1024 && $0.height == 1024 }) else { throw Failure("Expected 1024-square Apple exports") }
        for (index, name) in names.enumerated() {
            let source = exports[index]
            try write(resized(source, 1024, framed: true), app.appendingPathComponent("\(name).png"))
            let iconset = app.appendingPathComponent("\(name).iconset")
            for (points, scale) in representations {
                let file = "icon_\(points)x\(points)\(scale == 2 ? "@2x" : "").png"
                try write(resized(source, points * scale, framed: true), iconset.appendingPathComponent(file))
            }
            try run("/usr/bin/iconutil", ["--convert", "icns", "--output", app.appendingPathComponent("\(name).icns").path, iconset.path])
            let roundtrip = FileManager.default.temporaryDirectory.appendingPathComponent("Forkarak-\(UUID().uuidString).iconset")
            defer { try? FileManager.default.removeItem(at: roundtrip) }
            try run("/usr/bin/iconutil", ["--convert", "iconset", "--output", roundtrip.path, app.appendingPathComponent("\(name).icns").path])
            for (points, scale) in representations {
                let file = "icon_\(points)x\(points)\(scale == 2 ? "@2x" : "").png"
                let decoded = try read(roundtrip.appendingPathComponent(file))
                guard decoded.width == points * scale && decoded.height == points * scale else { throw Failure("Incorrect ICNS representation: \(file)") }
            }
            print("Verified \(name): Apple material export, 10 legacy ICNS representations")
        }
        for (source, destination) in [("app/charcoal.icns", "Forkarak.icns"), ("app/charcoal.png", "Forkarak-1024.png"), ("app/charcoal.png", "app-icon-1024.png")] {
            let target = icons.appendingPathComponent(destination)
            if FileManager.default.fileExists(atPath: target.path) { try FileManager.default.removeItem(at: target) }
            try FileManager.default.copyItem(at: icons.appendingPathComponent(source), to: target)
        }
    }
    if let qa = qa {
        try contactSheet(qa.appendingPathComponent("contact-light.png"), false, false)
        try contactSheet(qa.appendingPathComponent("contact-dark.png"), true, false)
        try contactSheet(qa.appendingPathComponent("icon-presets.png"), false, true)
    }
    if arguments.contains("--readme-sheet") {
        guard writeProduction else { throw Failure("--readme-sheet requires --write-production") }
        try contactSheet(root.appendingPathComponent("docs/images/icon-presets.png"), false, true)
    }
} catch {
    fputs("\(error)\n", stderr)
    exit(1)
}
