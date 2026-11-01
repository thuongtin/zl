import AppKit
import AVKit
import ImageIO
import UniformTypeIdentifiers

// The viewer accepts local files only and never resolves a remote URL.
func verifiedFile(_ path: String, mime: String) throws -> URL {
    guard !path.contains("://"), path.hasPrefix("/") else { throw NSError(domain: "LocalFileRequired", code: 1) }
    let url = URL(fileURLWithPath: path).resolvingSymlinksInPath()
    let values = try url.resourceValues(forKeys: [.isRegularFileKey, .fileSizeKey])
    guard values.isRegularFile == true, (values.fileSize ?? 0) > 0 else { throw NSError(domain: "MissingMedia", code: 2) }
    if mime.hasPrefix("image/") {
        guard let source = CGImageSourceCreateWithURL(url as CFURL, nil),
              let type = CGImageSourceGetType(source),
              let actual = UTType(type as String), actual.conforms(to: .image),
              (mime == "image/*" || UTType(mimeType: mime).map { actual.conforms(to: $0) } == true) else {
            throw NSError(domain: "ImageTypeMismatch", code: 3)
        }
    } else if mime.hasPrefix("video/") {
        let handle = try FileHandle(forReadingFrom: url)
        defer { try? handle.close() }
        let header = try handle.read(upToCount: 16) ?? Data()
        let iso = header.count >= 12 && String(data: header[4..<8], encoding: .ascii) == "ftyp"
        let webm = header.starts(with: [0x1a, 0x45, 0xdf, 0xa3])
        guard (iso && ["video/mp4", "video/quicktime"].contains(mime)) || (webm && mime == "video/webm") else {
            throw NSError(domain: "VideoTypeMismatch", code: 4)
        }
        // Confirm that AVFoundation identifies a real video track, not only an extension.
        let asset = AVURLAsset(url: url)
        guard !asset.tracks(withMediaType: .video).isEmpty else { throw NSError(domain: "InvalidVideo", code: 5) }
    } else { throw NSError(domain: "UnsupportedMedia", code: 6) }
    return url
}

final class ViewerDelegate: NSObject, NSApplicationDelegate, NSWindowDelegate {
    var window: NSWindow?
    var player: AVPlayer?
    let url: URL
    let mime: String
    init(url: URL, mime: String) { self.url = url; self.mime = mime }
    func applicationDidFinishLaunching(_ notification: Notification) {
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 960, height: 720),
                              styleMask: [.titled, .closable, .resizable, .miniaturizable], backing: .buffered, defer: false)
        window.title = "Media Viewer"
        window.delegate = self
        if mime.hasPrefix("video/") {
            let view = AVPlayerView(frame: window.contentView!.bounds)
            let player = AVPlayer(url: url)
            self.player = player
            view.player = player
            view.controlsStyle = .floating
            view.autoresizingMask = [.width, .height]
            window.contentView = view
            // Playback starts only through the player's explicit controls.
        } else {
            let scroll = NSScrollView(frame: window.contentView!.bounds)
            scroll.hasHorizontalScroller = true
            scroll.hasVerticalScroller = true
            scroll.allowsMagnification = true
            scroll.minMagnification = 0.05
            scroll.maxMagnification = 16
            guard let image = NSImage(contentsOf: url) else { NSApp.terminate(nil); return }
            let view = NSImageView(frame: NSRect(origin: .zero, size: image.size))
            view.image = image
            view.imageScaling = .scaleProportionallyUpOrDown
            scroll.documentView = view
            scroll.autoresizingMask = [.width, .height]
            window.contentView = scroll
            let fit = min(1, min(960 / max(1, image.size.width), 720 / max(1, image.size.height)))
            scroll.magnification = fit
        }
        self.window = window
        window.center()
        window.makeKeyAndOrderFront(nil)
    }
    func windowWillClose(_ notification: Notification) { player?.pause(); NSApp.terminate(nil) }
}

let arguments = Array(CommandLine.arguments.dropFirst())
if arguments == ["--self-test"] {
    do { _ = try verifiedFile("https://invalid.example/a.png", mime: "image/png"); exit(1) } catch {}
    do { _ = try verifiedFile("/nonexistent-daily-media-fixture", mime: "video/mp4"); exit(1) } catch {}
    let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
    do {
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        let png = directory.appendingPathComponent("fixture.media")
        let bytes = Data(base64Encoded: "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aX1sAAAAASUVORK5CYII=")!
        try bytes.write(to: png)
        _ = try verifiedFile(png.path, mime: "image/png")
        _ = try verifiedFile(png.path, mime: "image/*")
        do { _ = try verifiedFile(png.path, mime: "image/jpeg"); exit(1) } catch {}
        do { _ = try verifiedFile(png.path, mime: "video/mp4"); exit(1) } catch {}
    } catch { fputs("SELF_TEST: fixture validation FAIL\n", stderr); exit(1) }
    print("SELF_TEST: local-only, file existence, image MIME, and video spoof rejection PASS")
    exit(0)
}
guard arguments.count == 4, arguments[0] == "--file", arguments[2] == "--mime" else {
    fputs("Usage: DailyMediaViewer --file ABSOLUTE_PATH --mime MIME\n", stderr); exit(2)
}
do {
    let url = try verifiedFile(arguments[1], mime: arguments[3])
    let application = NSApplication.shared
    application.setActivationPolicy(.regular)
    let delegate = ViewerDelegate(url: url, mime: arguments[3])
    application.delegate = delegate
    withExtendedLifetime(delegate) { application.run() }
} catch {
    // Do not print source paths, message identity, URLs, or account information.
    fputs("Media validation failed\n", stderr); exit(3)
}
