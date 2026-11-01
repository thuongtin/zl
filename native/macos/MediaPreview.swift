import Foundation
import ImageIO
import UniformTypeIdentifiers

private let maximumBytes = 16 * 1024 * 1024
private let maximumPixels: UInt64 = 64 * 1024 * 1024
private enum PreviewError: Error { case invalidInput, decoding, encoding }

private func recognizedImage(_ data: Data) -> Bool {
    let bytes = [UInt8](data.prefix(64))
    if bytes.starts(with: [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]) { return true }
    if bytes.starts(with: [0xff, 0xd8, 0xff]) { return true }
    if bytes.starts(with: [0xff, 0x0a]) { return true }
    if bytes.starts(with: [0, 0, 0, 12, 0x4a, 0x58, 0x4c, 0x20, 0x0d, 0x0a, 0x87, 0x0a]) { return true }
    if bytes.count >= 12, String(bytes: bytes[0..<4], encoding: .ascii) == "RIFF",
       String(bytes: bytes[8..<12], encoding: .ascii) == "WEBP" { return true }
    if bytes.count >= 16, String(bytes: bytes[4..<8], encoding: .ascii) == "ftyp" {
        let brands = ["heic", "heix", "hevc", "hevx", "mif1", "msf1"]
        return stride(from: 8, through: bytes.count - 4, by: 4).contains {
            brands.contains(String(bytes: bytes[$0..<$0 + 4], encoding: .ascii) ?? "")
        }
    }
    return false
}

private func preview(_ data: Data) throws -> Data {
    guard !data.isEmpty, data.count <= maximumBytes, recognizedImage(data) else { throw PreviewError.invalidInput }
    let options = [kCGImageSourceShouldCache: false] as CFDictionary
    guard let source = CGImageSourceCreateWithData(data as CFData, options),
          CGImageSourceGetCount(source) > 0,
          let properties = CGImageSourceCopyPropertiesAtIndex(source, 0, options) as? [CFString: Any],
          let width = properties[kCGImagePropertyPixelWidth] as? NSNumber,
          let height = properties[kCGImagePropertyPixelHeight] as? NSNumber else { throw PreviewError.decoding }
    let w = width.uint64Value, h = height.uint64Value
    guard w > 0, h > 0, w <= 32768, h <= 32768, w * h <= maximumPixels else { throw PreviewError.invalidInput }
    let thumbnailOptions = [
        kCGImageSourceCreateThumbnailFromImageAlways: true,
        kCGImageSourceCreateThumbnailWithTransform: true,
        kCGImageSourceThumbnailMaxPixelSize: 1024,
        kCGImageSourceShouldCacheImmediately: true
    ] as CFDictionary
    guard let image = CGImageSourceCreateThumbnailAtIndex(source, 0, thumbnailOptions),
          image.width <= 1024, image.height <= 1024 else { throw PreviewError.decoding }
    let output = NSMutableData()
    guard let destination = CGImageDestinationCreateWithData(output, UTType.png.identifier as CFString, 1, nil) else { throw PreviewError.encoding }
    CGImageDestinationAddImage(destination, image, nil)
    guard CGImageDestinationFinalize(destination), output.length <= 8 * 1024 * 1024 else { throw PreviewError.encoding }
    return output as Data
}

do {
    // Read at most one byte beyond the limit, without buffering an unbounded stream.
    var input = Data()
    while input.count <= maximumBytes {
        let chunk = try FileHandle.standardInput.read(upToCount: min(65536, maximumBytes + 1 - input.count)) ?? Data()
        if chunk.isEmpty { break }
        input.append(chunk)
    }
    try FileHandle.standardOutput.write(contentsOf: preview(input))
} catch {
    // A fixed diagnostic never reveals media content or origin.
    FileHandle.standardError.write(Data("Media preview failed\n".utf8))
    exit(1)
}
