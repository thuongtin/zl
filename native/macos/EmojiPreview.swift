import Foundation
import CoreText
import CoreGraphics
import ImageIO

private let maximumInputBytes = 256
private let canvasSize = 64
private enum PreviewError: Error { case invalidInput, rendering, encoding }

private func preview(_ input: Data) throws -> Data {
    guard !input.isEmpty, input.count <= maximumInputBytes,
          let text = String(data: input, encoding: .utf8), text.count == 1,
          text.unicodeScalars.contains(where: { $0.properties.isEmoji }),
          !text.unicodeScalars.contains(where: {
              $0.properties.generalCategory == .control ||
              $0.properties.generalCategory == .lineSeparator ||
              $0.properties.generalCategory == .paragraphSeparator ||
              $0.properties.isWhitespace
          }) else { throw PreviewError.invalidInput }

    let font = CTFontCreateWithName("AppleColorEmoji" as CFString, 52, nil)
    let attributes = [kCTFontAttributeName: font] as [CFString: Any]
    let attributed = NSAttributedString(string: text, attributes: attributes.reduce(into: [:]) {
        $0[NSAttributedString.Key($1.key as String)] = $1.value
    })
    // Shape the entire extended grapheme cluster in one line, never scalar by scalar.
    let line = CTLineCreateWithAttributedString(attributed)
    for run in CTLineGetGlyphRuns(line) as! [CTRun] {
        let count = CTRunGetGlyphCount(run)
        guard count > 0 else { throw PreviewError.rendering }
        var glyphs = [CGGlyph](repeating: 0, count: count)
        CTRunGetGlyphs(run, CFRange(location: 0, length: 0), &glyphs)
        guard !glyphs.contains(0) else { throw PreviewError.rendering }
    }
    guard let context = CGContext(data: nil, width: canvasSize, height: canvasSize,
                                  bitsPerComponent: 8, bytesPerRow: canvasSize * 4,
                                  space: CGColorSpaceCreateDeviceRGB(),
                                  bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) else {
        throw PreviewError.rendering
    }
    context.clear(CGRect(x: 0, y: 0, width: canvasSize, height: canvasSize))
    let bounds = CTLineGetImageBounds(line, context)
    guard !bounds.isNull, bounds.width > 0, bounds.height > 0,
          bounds.width.isFinite, bounds.height.isFinite else { throw PreviewError.rendering }
    let scale = min(1, 56 / max(bounds.width, bounds.height))
    context.translateBy(x: (64 - bounds.width * scale) / 2 - bounds.minX * scale,
                        y: (64 - bounds.height * scale) / 2 - bounds.minY * scale)
    context.scaleBy(x: scale, y: scale)
    context.textPosition = .zero
    CTLineDraw(line, context)
    guard let image = context.makeImage() else { throw PreviewError.rendering }
    let output = NSMutableData()
    guard let destination = CGImageDestinationCreateWithData(output, "public.png" as CFString, 1, nil) else {
        throw PreviewError.encoding
    }
    CGImageDestinationAddImage(destination, image, nil)
    guard CGImageDestinationFinalize(destination), output.length <= 128 * 1024 else {
        throw PreviewError.encoding
    }
    return output as Data
}

do {
    var input = Data()
    while input.count <= maximumInputBytes {
        let chunk = try FileHandle.standardInput.read(upToCount: maximumInputBytes + 1 - input.count) ?? Data()
        if chunk.isEmpty { break }
        input.append(chunk)
    }
    try FileHandle.standardOutput.write(contentsOf: preview(input))
} catch {
    FileHandle.standardError.write(Data("Emoji preview failed\n".utf8))
    exit(1)
}
