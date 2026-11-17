#!/usr/bin/env swift
// Deterministic format/alpha preparation only. Artwork is the preserved image_gen PNGs.
// Run from any directory: swift scripts/generate-app-icons.swift [--qa-dir /tmp/icon-qa]
import CoreGraphics
import Foundation
import ImageIO
import UniformTypeIdentifiers

let root = URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent()
let icons = root.appendingPathComponent("assets/icons")
let app = icons.appendingPathComponent("app")
let names = ["charcoal", "deep-blue", "deep-green", "plum", "ivory", "light-blue"]
let representations = [(16, 1), (16, 2), (32, 1), (32, 2), (128, 1), (128, 2), (256, 1), (256, 2), (512, 1), (512, 2)]
let colorSpace = CGColorSpace(name: CGColorSpace.sRGB)!
let sourceSize = 1254
// The same small inward trim on every source removes generated fringe/islands.
// This leaves the spork and its shadow untouched, with the original canvas/inset.
// CoreGraphics has a bottom-left origin: source top 114, bottom 1172 -> y 82.
let tile = CGRect(x: 100, y: 82, width: 1058, height: 1058)
let silhouette = CGPath(roundedRect: tile, cornerWidth: 244, cornerHeight: 244, transform: nil)
var coverageCache: [Int: [UInt8]] = [:]

func context(_ size: Int) -> CGContext {
    CGContext(data: nil, width: size, height: size, bitsPerComponent: 8,
              bytesPerRow: size * 4, space: colorSpace,
              bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
}
func read(_ url: URL) throws -> CGImage {
    guard let source = CGImageSourceCreateWithURL(url as CFURL, nil),
          let image = CGImageSourceCreateImageAtIndex(source, 0, nil) else {
        throw Failure("Cannot decode \(url.path)")
    }
    return image
}
func write(_ image: CGImage, _ url: URL) throws {
    try FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
    guard let destination = CGImageDestinationCreateWithURL(url as CFURL, UTType.png.identifier as CFString, 1, nil) else {
        throw Failure("Cannot write \(url.path)")
    }
    CGImageDestinationAddImage(destination, image, nil)
    guard CGImageDestinationFinalize(destination) else { throw Failure("PNG encoding failed") }
}
func resized(_ image: CGImage, _ size: Int) -> CGImage {
    let c = context(size)
    c.setBlendMode(.copy)
    c.interpolationQuality = .high
    c.draw(image, in: CGRect(x: 0, y: 0, width: size, height: size))
    // Apply a single color-independent native coverage mask. CoreGraphics' general
    // image resampler otherwise rounds alpha differently for different RGB colors.
    if coverageCache[size] == nil {
        let mask = context(size)
        mask.scaleBy(x: CGFloat(size) / CGFloat(sourceSize), y: CGFloat(size) / CGFloat(sourceSize))
        mask.addPath(silhouette)
        mask.setFillColor(CGColor(gray: 1, alpha: 1))
        mask.fillPath()
        let bytes = mask.data!.assumingMemoryBound(to: UInt8.self)
        coverageCache[size] = (0..<(size * size)).map { bytes[$0 * 4 + 3] }
    }
    let bytes = c.data!.assumingMemoryBound(to: UInt8.self)
    for (pixel, coverage) in coverageCache[size]!.enumerated() {
        let offset = pixel * 4
        let previous = Int(bytes[offset + 3])
        for channel in 0..<3 {
            bytes[offset + channel] = previous == 0 ? 0 : UInt8(min(255, (Int(bytes[offset + channel]) * Int(coverage) + previous / 2) / previous))
        }
        bytes[offset + 3] = coverage
    }
    return c.makeImage()!
}
func normalized(_ image: CGImage) throws -> CGImage {
    guard image.width == sourceSize && image.height == sourceSize else { throw Failure("Unexpected source canvas") }
    let rect = CGRect(x: 0, y: 0, width: sourceSize, height: sourceSize)
    // Opaque backing prevents residual source-alpha noise from changing the shared outline.
    // The backing is covered by the source throughout the retained tile interior.
    let opaque = context(sourceSize)
    opaque.setFillColor(CGColor(gray: 0.5, alpha: 1))
    opaque.fill(rect)
    opaque.draw(image, in: rect)
    return opaque.makeImage()!
}
func alpha(_ image: CGImage) -> [UInt8] {
    let storage = image.dataProvider!.data!
    let data = CFDataGetBytePtr(storage)!
    return (0..<(image.width * image.height)).map { data[($0 / image.width) * image.bytesPerRow + ($0 % image.width) * 4 + 3] }
}
func run(_ arguments: [String]) throws {
    let p = Process()
    p.executableURL = URL(fileURLWithPath: "/usr/bin/iconutil")
    p.arguments = arguments
    try p.run()
    p.waitUntilExit()
    guard p.terminationStatus == 0 else { throw Failure("iconutil failed: \(arguments)") }
}
struct Failure: Error, CustomStringConvertible {
    let description: String
    init(_ description: String) { self.description = description }
}

do {
    var sharedAlpha: [Int: [UInt8]] = [:]
    for name in names {
        let image = try normalized(read(app.appendingPathComponent("sources/\(name).png")))
        try write(resized(image, 1024), app.appendingPathComponent("\(name).png"))
        let iconset = app.appendingPathComponent("\(name).iconset")
        for (points, scale) in representations {
            let size = points * scale
            let frame = resized(image, size)
            let file = "icon_\(points)x\(points)\(scale == 2 ? "@2x" : "").png"
            try write(frame, iconset.appendingPathComponent(file))
            let a = alpha(frame)
            if let expected = sharedAlpha[size] {
                guard a == expected else {
                    throw Failure("Inconsistent alpha silhouette: \(name) \(size)")
                }
            } else { sharedAlpha[size] = a }
            // The full outer row/column must be empty at every resolution, even 16 px.
            guard (0..<size).allSatisfy({ a[$0] == 0 && a[(size - 1) * size + $0] == 0 && a[$0 * size] == 0 && a[$0 * size + size - 1] == 0 }) else {
                throw Failure("Nontransparent canvas edge: \(name) \(size)")
            }
        }
        try run(["--convert", "icns", "--output", app.appendingPathComponent("\(name).icns").path, iconset.path])
        // Verify the encoded container actually exposes every required representation.
        let roundtrip = FileManager.default.temporaryDirectory.appendingPathComponent("Forkarak-icon-\(UUID().uuidString).iconset")
        defer { try? FileManager.default.removeItem(at: roundtrip) }
        try run(["--convert", "iconset", "--output", roundtrip.path, app.appendingPathComponent("\(name).icns").path])
        for (points, scale) in representations {
            let file = "icon_\(points)x\(points)\(scale == 2 ? "@2x" : "").png"
            let decoded = try read(roundtrip.appendingPathComponent(file))
            guard decoded.width == points * scale && decoded.height == points * scale else { throw Failure("Incorrect ICNS representation: \(file)") }
        }
        print("Verified \(name): shared clean alpha, 10 ICNS representations (16–1024 px)")
    }
    // Existing CFBundleIconFile/package copy already consumes this exact default path.
    let fm = FileManager.default
    for (source, destination) in [("app/charcoal.icns", "Forkarak.icns"), ("app/charcoal.png", "Forkarak-1024.png"), ("app/charcoal.png", "app-icon-1024.png")] {
        let target = icons.appendingPathComponent(destination)
        if fm.fileExists(atPath: target.path) { try fm.removeItem(at: target) }
        try fm.copyItem(at: icons.appendingPathComponent(source), to: target)
    }
    if let flag = CommandLine.arguments.firstIndex(of: "--qa-dir"), flag + 1 < CommandLine.arguments.count {
        let qa = URL(fileURLWithPath: CommandLine.arguments[flag + 1])
        // QA contact sheets on both light/dark grounds, with actual 32/64/128 px renders.
        for (background, gray) in [("light", CGFloat(0.95)), ("dark", CGFloat(0.12))] {
            let c = CGContext(data: nil, width: 960, height: 380, bitsPerComponent: 8,
                              bytesPerRow: 960 * 4, space: colorSpace,
                              bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
            c.setFillColor(CGColor(gray: gray, alpha: 1))
            c.fill(CGRect(x: 0, y: 0, width: 960, height: 380))
            for (column, name) in names.enumerated() {
                for (row, size) in [32, 64, 128].enumerated() {
                    let image = resized(try read(app.appendingPathComponent("\(name).png")), size)
                    c.draw(image, in: CGRect(x: column * 160 + (160 - size) / 2, y: 300 - row * 130, width: size, height: size))
                }
            }
            try write(c.makeImage()!, qa.appendingPathComponent("contact-\(background).png"))
        }
    }
} catch {
    fputs("\(error)\n", stderr)
    exit(1)
}
