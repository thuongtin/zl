import Foundation
import JavaScriptCore
import Security
import zlib

@_silgen_name("ZSTD_getFrameContentSize")
private func zstdContentSize(_ source: UnsafeRawPointer?, _ size: Int) -> UInt64
@_silgen_name("ZSTD_decompress")
private func zstdDecompress(_ destination: UnsafeMutableRawPointer?, _ capacity: Int, _ source: UnsafeRawPointer?, _ size: Int) -> Int
@_silgen_name("ZSTD_isError")
private func zstdIsError(_ code: Int) -> UInt32

private let maximumBytes = 128 * 1024 * 1024
private enum CodecError: Error { case invalidInput, invalidAssets, missingAssets, initialization, decompression }

private func decompress(_ input: Data) throws -> Data {
    if input.starts(with: [0x28, 0xb5, 0x2f, 0xfd]) {
        let size = input.withUnsafeBytes { zstdContentSize($0.baseAddress, input.count) }
        if size == UInt64.max - 1 { throw CodecError.decompression }
        var capacity = size == UInt64.max ? min(maximumBytes, max(65536, input.count * 8)) : Int(min(size, UInt64(maximumBytes + 1)))
        guard capacity <= maximumBytes else { throw CodecError.decompression }
        repeat {
            var output = Data(count: max(1, capacity))
            let result = output.withUnsafeMutableBytes { destination in
                input.withUnsafeBytes { source in
                    zstdDecompress(destination.baseAddress, capacity, source.baseAddress, input.count)
                }
            }
            if zstdIsError(result) == 0 {
                output.count = result
                return output
            }
            if size != UInt64.max || capacity >= maximumBytes { throw CodecError.decompression }
            capacity = min(maximumBytes, capacity * 2)
        } while true
    }
    // Accept zlib and gzip wrappers, with validated stream completion.
    if (input.count >= 2 && input[input.startIndex] == 0x78) || input.starts(with: [0x1f, 0x8b]) {
        var capacity = min(maximumBytes, max(65536, input.count * 8))
        repeat {
            var output = Data(count: capacity)
            var stream = z_stream()
            guard inflateInit2_(&stream, 47, ZLIB_VERSION, Int32(MemoryLayout<z_stream>.size)) == Z_OK else {
                throw CodecError.decompression
            }
            let result = output.withUnsafeMutableBytes { destination in
                input.withUnsafeBytes { source -> Int32 in
                    stream.next_in = UnsafeMutablePointer(mutating: source.bindMemory(to: UInt8.self).baseAddress)
                    stream.avail_in = uInt(input.count)
                    stream.next_out = destination.bindMemory(to: UInt8.self).baseAddress
                    stream.avail_out = uInt(capacity)
                    return inflate(&stream, Z_FINISH)
                }
            }
            let complete = result == Z_STREAM_END && stream.avail_in == 0
            let count = Int(stream.total_out)
            let needsSpace = stream.avail_out == 0 && (result == Z_BUF_ERROR || result == Z_OK)
            inflateEnd(&stream)
            if complete { output.count = count; return output }
            if !needsSpace || capacity >= maximumBytes { throw CodecError.decompression }
            capacity = min(maximumBytes, capacity * 2)
        } while true
    }
    return input
}

private final class Codec {
    let context: JSContext

    init(assets: URL) throws {
        let required = ["web-module-hRO2.js", "web-module-Ho1Y.js", "web-module-1y5R.js", "web-module-Vox7.js", "libzproto.wasm"]
        guard required.allSatisfy({ FileManager.default.fileExists(atPath: assets.appendingPathComponent($0).path) }) else {
            throw CodecError.missingAssets
        }
        guard let context = JSContext() else { throw CodecError.initialization }
        self.context = context
        let encode: @convention(block) (String) -> [NSNumber] = { Array($0.utf8).map { NSNumber(value: $0) } }
        let decode: @convention(block) (JSValue) -> String = { value in
            let bytes = (value.toArray() as? [NSNumber] ?? []).map { $0.uint8Value }
            return String(decoding: bytes, as: UTF8.self)
        }
        let random: @convention(block) (Int) -> [NSNumber] = { count in
            guard count >= 0 && count <= 65536 else { return [] }
            var bytes = [UInt8](repeating: 0, count: count)
            guard SecRandomCopyBytes(kSecRandomDefault, count, &bytes) == errSecSuccess else { return [] }
            return bytes.map { NSNumber(value: $0) }
        }
        let fromBase64: @convention(block) (String) -> JSValue? = { value in
            guard let data = Data(base64Encoded: value), data.count <= maximumBytes,
                  let memory = malloc(max(1, data.count)) else { return nil }
            data.copyBytes(to: memory.assumingMemoryBound(to: UInt8.self), count: data.count)
            var exception: JSValueRef?
            guard let array = JSObjectMakeTypedArrayWithBytesNoCopy(
                context.jsGlobalContextRef, kJSTypedArrayTypeUint8Array, memory, data.count,
                { bytes, _ in free(bytes) }, nil, &exception), exception == nil else { return nil }
            return JSValue(jsValueRef: array, in: context)
        }
        let toBase64: @convention(block) (JSValue) -> String? = { value in
            var exception: JSValueRef?
            guard let object = JSValueToObject(context.jsGlobalContextRef, value.jsValueRef, &exception),
                  exception == nil else { return nil }
            let count = JSObjectGetTypedArrayByteLength(context.jsGlobalContextRef, object, &exception)
            guard exception == nil, count <= maximumBytes else { return nil }
            if count == 0 { return "" }
            let offset = JSObjectGetTypedArrayByteOffset(context.jsGlobalContextRef, object, &exception)
            guard exception == nil,
                  let bytes = JSObjectGetTypedArrayBytesPtr(context.jsGlobalContextRef, object, &exception),
                  exception == nil else { return nil }
            return Data(bytes: bytes.advanced(by: offset), count: count).base64EncodedString()
        }
        let unpack: @convention(block) (String) -> String? = { value in
            guard let data = Data(base64Encoded: value), let result = try? decompress(data) else { return nil }
            return result.base64EncodedString()
        }
        context.setObject(encode, forKeyedSubscript: "_encodeUTF8" as NSString)
        context.setObject(decode, forKeyedSubscript: "_decodeUTF8" as NSString)
        context.setObject(random, forKeyedSubscript: "_randomBytes" as NSString)
        context.setObject(fromBase64, forKeyedSubscript: "_fromBase64" as NSString)
        context.setObject(toBase64, forKeyedSubscript: "_toBase64" as NSString)
        context.setObject(unpack, forKeyedSubscript: "_unpack" as NSString)
        try evaluate(Self.bootstrap)
        for name in ["hRO2", "Ho1Y", "1y5R", "Vox7"] {
            let source = try String(contentsOf: assets.appendingPathComponent("web-module-\(name).js"), encoding: .utf8)
            let prefix = source.hasPrefix(name + ":function") ? name + ":" : "\"" + name + "\":"
            guard source.hasPrefix(prefix + "function") else { throw CodecError.invalidAssets }
            let factory = String(source.dropFirst(prefix.count))
            try evaluate("_factories[\"\(name)\"] = (\(factory));")
        }
        let wasm = try Data(contentsOf: assets.appendingPathComponent("libzproto.wasm"))
        context.setObject(wasm.base64EncodedString(), forKeyedSubscript: "_wasmBase64" as NSString)
        try evaluate("_begin(_wasmBase64);")
        let deadline = Date().addingTimeInterval(20)
        while !context.objectForKeyedSubscript("_ready").toBool() {
            if context.objectForKeyedSubscript("_initFailed").toBool() || Date() >= deadline { throw CodecError.initialization }
            RunLoop.current.run(until: Date().addingTimeInterval(0.002))
            try evaluate("void 0;")
        }
    }

    private func evaluate(_ source: String) throws {
        context.exception = nil
        context.evaluateScript(source)
        if context.exception != nil { context.exception = nil; throw CodecError.initialization }
    }

    func request(_ request: [String: Any]) throws -> Any {
        let data = try JSONSerialization.data(withJSONObject: request)
        context.setObject(String(decoding: data, as: UTF8.self), forKeyedSubscript: "_requestLine" as NSString)
        context.exception = nil
        guard let result = context.evaluateScript("JSON.stringify(_handle(JSON.parse(_requestLine)))"),
              context.exception == nil, let text = result.toString(), let bytes = text.data(using: .utf8) else {
            context.exception = nil
            throw CodecError.invalidInput
        }
        return try JSONSerialization.jsonObject(with: bytes)
    }

    private static let bootstrap = #"""
    globalThis.self = globalThis;
    globalThis.window = globalThis;
    globalThis.proto = {};
    globalThis.COMPILED = true;
    globalThis.TextEncoder = class { encode(s = '') { return new Uint8Array(_encodeUTF8(String(s))); } };
    globalThis.TextDecoder = class { decode(b = new Uint8Array()) { return _decodeUTF8(Array.from(b)); } };
    globalThis.crypto = { getRandomValues(array) {
        const bytes = _randomBytes(array.byteLength);
        if (!bytes || bytes.length !== array.byteLength) throw Error('Random source failed');
        new Uint8Array(array.buffer, array.byteOffset, array.byteLength).set(bytes);
        return array;
    }};
    var _factories = {}, _modules = {}, _ready = false, _initFailed = false, _phase = 'initialization', _decodeDiagnostic = {};
    function _require(name) {
        if (name === 'yLpj') return globalThis;
        if (name === '3UD+') return function(module) { return module; };
        if (name === 'N3ek') return {};
        if (_modules[name]) return _modules[name].exports;
        if (!_factories[name]) throw Error('Missing codec module');
        const module = {exports: {}};
        _modules[name] = module;
        _factories[name](module, module.exports, _require);
        return module.exports;
    }
    _require.d = function(exports, name, getter) { Object.defineProperty(exports, name, {enumerable: true, get: getter}); };
    function _bytes(value) {
        if (typeof value !== 'string') throw Error('Expected base64');
        const bytes = _fromBase64(value);
        if (!bytes) throw Error('Invalid base64');
        return bytes;
    }
    function _b64(value) { return _toBase64(value); }
    var _zproto, _message, _conversation, _identity = null, _sessions = new Map();
    function _begin(wasm) {
        _zproto = _require('Vox7');
        _message = _require('1y5R').SyncChunk;
        _conversation = _require('Ho1Y').SyncChunk;
        _zproto.b(_bytes(wasm)).then(function() { _ready = true; }, function() { _initFailed = true; });
    }
    function _newSession(id) {
        if (typeof id !== 'string' || !id.length || id.length > 512) throw Error('Invalid session id');
        if (_sessions.has(id)) _sessions.get(id).key.free();
        const session = {key: _zproto.e(), record: null};
        _sessions.set(id, session);
        return {ik: _b64(_identity.publicKey), ek: _b64(session.key.publicKey)};
    }
    function _split(blob) {
        const chunks = [];
        let position = 0;
        const view = new DataView(blob.buffer, blob.byteOffset, blob.byteLength);
        while (position < blob.length) {
            if (blob.length - position < 4) throw Error('Truncated frame header');
            const length = view.getUint32(position, true);
            position += 4;
            if (!length || length > blob.length - position) throw Error('Invalid frame length');
            chunks.push(blob.subarray(position, position + length));
            position += length;
        }
        if (!chunks.length) throw Error('Empty batch');
        return chunks;
    }
    function _handle(request) {
        _phase = 'request';
        _decodeDiagnostic = {};
        if (request.op === 'init') {
            for (const session of _sessions.values()) session.key.free();
            _sessions.clear();
            if (_identity) _identity.free();
            _identity = _zproto.e();
            return _newSession(request.id || 'default');
        }
        if (!_identity) throw Error('Initialize first');
        if (request.op === 'selfTest') {
            _phase = 'selfTestAgreement';
            const first = _zproto.e(), second = _zproto.e();
            try {
                if (_b64(_zproto.c(first.privateKey, second.publicKey)) !==
                    _b64(_zproto.c(second.privateKey, first.publicKey))) throw Error('Agreement mismatch');
                _phase = 'selfTestProtobuf';
                for (const codec of [_conversation, _message]) {
                    const fixture = new codec();
                    fixture.setPlatform(1);
                    if (codec.deserializeBinary(fixture.serializeBinary()).toObject().platform !== 1) throw Error('Protobuf mismatch');
                }
                _phase = 'selfTestCompression';
                for (const fixture of ['KLUv/SACEQAACAE=', 'eJzjYAQAABMACg==', 'H4sIAAAAAAAC/+NgBABhqAf+AgAAAA==']) {
                    if (_unpack(fixture) !== 'CAE=') throw Error('Decompression mismatch');
                }
                if (_b64(new Uint8Array([0, 8, 1, 0]).subarray(1, 3)) !== 'CAE=') throw Error('Typed array offset mismatch');
                let rejected = false;
                try { _split(new Uint8Array([2, 0, 0, 0, 1])); } catch (_) { rejected = true; }
                if (!rejected) throw Error('Frame validation mismatch');
                return {checks: ['keyAgreement', 'conversationProtobuf', 'messageProtobuf', 'zstd', 'zlib', 'gzip', 'strictFraming', 'typedArrayTransfer']};
            } finally { first.free(); second.free(); }
        }
        if (request.op === 'newSession') return _newSession(request.id);
        if (request.op === 'releaseSession') {
            const session = _sessions.get(request.id);
            if (session) { session.key.free(); _sessions.delete(request.id); }
            return {};
        }
        if (request.op !== 'decode') throw Error('Unknown operation');
        const session = _sessions.get(request.id || 'default');
        if (!session) throw Error('Unknown session');
        if (request.kind !== 'conversation' && request.kind !== 'message') throw Error('Invalid protobuf kind');
        if (request.batchType !== 1 && request.batchType !== 2) throw Error('Invalid batch type');
        _decodeDiagnostic = {kind: request.kind, batchType: request.batchType, chunkIndex: 0, chunkBytes: 0,
            hasMetadata: !!session.record, recordBytes: session.record ? session.record.length : 0,
            publicKeyBytes: session.key.publicKey.length, reason: 'framing'};
        _phase = 'framing';
        const chunks = _split(_bytes(request.blob));
        const objects = [];
        const codec = request.kind === 'conversation' ? _conversation : _message;
        for (let chunkIndex = 0; chunkIndex < chunks.length; chunkIndex++) {
            const chunk = chunks[chunkIndex];
            Object.assign(_decodeDiagnostic, {chunkIndex, chunkBytes: chunk.length, hasMetadata: !!session.record,
                recordBytes: session.record ? session.record.length : 0, reason: 'decryptFailure'});
            let decrypted, plaintext;
            try {
                if (request.batchType === 2) {
                    _phase = 'decryptMetadata';
                    decrypted = _zproto.j(chunk, _identity.publicKey, _identity.privateKey, session.key.privateKey);
                    session.record = decrypted.sessionRecord;
                    plaintext = decrypted.plaintext;
                } else {
                    _phase = 'decryptMessage';
                    if (!session.record) { _decodeDiagnostic.reason = 'missingMetadata'; throw Error('Metadata required'); }
                    decrypted = _zproto.i(chunk, session.record);
                    plaintext = decrypted.plaintext;
                }
                _decodeDiagnostic.reason = 'decompression';
                _phase = 'decompression';
                const unpacked = _unpack(_b64(plaintext));
                if (unpacked === null || unpacked === undefined) throw Error('Decompression failed');
                _decodeDiagnostic.reason = 'protobuf';
                _phase = 'protobuf';
                objects.push(codec.deserializeBinary(_bytes(unpacked)).toObject());
            } finally { if (decrypted) decrypted.free(); }
        }
        _decodeDiagnostic.reason = "complete";
        return {objects: objects};
    }
    """#
}

// Explicit local opt-in. Only bounded structural metadata is recorded, never data or keys.
private func recordDecodeDiagnostic(_ request: [String: Any], _ envelope: [String: Any], _ codec: Codec?) {
    guard request["op"] as? String == "decode",
          FileManager.default.fileExists(atPath: ".history-codec-diagnostics-enabled"),
          let raw = codec?.context.objectForKeyedSubscript("_decodeDiagnostic")?.toDictionary() else { return }
    var entry: [String: Any] = ["time_ms": Int64(Date().timeIntervalSince1970 * 1000), "ok": envelope["ok"] as? Bool ?? false]
    if let kind = raw["kind"] as? String, ["conversation", "message"].contains(kind) { entry["kind"] = kind }
    if let reason = raw["reason"] as? String, ["framing", "missingMetadata", "decryptFailure", "decompression", "protobuf", "complete"].contains(reason) { entry["reason"] = reason }
    for key in ["batchType", "chunkIndex", "chunkBytes", "recordBytes", "publicKeyBytes"] {
        if let value = raw[key] as? NSNumber, value.int64Value >= 0, value.int64Value <= maximumBytes { entry[key] = value }
    }
    entry["hasMetadata"] = raw["hasMetadata"] as? Bool ?? false
    let path = URL(fileURLWithPath: "history_codec_diagnostics.json")
    var entries = (try? Data(contentsOf: path)).flatMap { try? JSONSerialization.jsonObject(with: $0) as? [[String: Any]] } ?? []
    entries.append(entry)
    if entries.count > 40 { entries.removeFirst(entries.count - 40) }
    guard let data = try? JSONSerialization.data(withJSONObject: entries, options: [.sortedKeys]) else { return }
    if !FileManager.default.fileExists(atPath: path.path) {
        guard FileManager.default.createFile(atPath: path.path, contents: nil, attributes: [.posixPermissions: 0o600]) else { return }
    }
    try? FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: path.path)
    guard let handle = try? FileHandle(forWritingTo: path) else { return }
    defer { try? handle.close() }
    try? handle.truncate(atOffset: 0)
    try? handle.write(contentsOf: data)
}

private var codec: Codec?
private func response(_ request: [String: Any]) -> [String: Any] {
    var envelope: [String: Any] = [:]
    if let id = request["id"] { envelope["id"] = id }
    do {
        if request["op"] as? String == "init", codec == nil {
            let executable = URL(fileURLWithPath: CommandLine.arguments[0]).standardizedFileURL
            let siblingAssets = executable.deletingLastPathComponent().appendingPathComponent("HistoryCodecAssets")
            let bundleAssets = executable.deletingLastPathComponent().deletingLastPathComponent().appendingPathComponent("Resources/HistoryCodecAssets")
            let fallback = FileManager.default.fileExists(atPath: siblingAssets.path) ? siblingAssets : bundleAssets
            let assets = (request["assetsDir"] as? String).map { URL(fileURLWithPath: $0) } ?? fallback
            codec = try Codec(assets: assets)
        }
        guard let codec = codec else { throw CodecError.invalidInput }
        envelope["result"] = try codec.request(request)
        envelope["ok"] = true
    } catch {
        // Never serialize JavaScript exceptions, plaintext, key material, or signed URLs.
        envelope["ok"] = false
        let allowedPhases = ["initialization", "request", "framing", "decryptMetadata", "decryptMessage", "decompression", "protobuf", "selfTestAgreement", "selfTestProtobuf", "selfTestCompression"]
        let phase = codec?.context.objectForKeyedSubscript("_phase")?.toString() ?? "initialization"
        if let error = error as? CodecError, case .missingAssets = error {
            envelope["code"] = "history_codec_assets_missing"
            envelope["error"] = "Thiếu tài nguyên HistoryCodec. Chạy python3 native/macos/prepare-history-assets.py --local-use --output /path/to/HistoryCodecAssets trong mã nguồn, rồi cấu hình assetsDir đến thư mục đó. Bản phát hành công khai không kèm JS/WASM bên ngoài."
        } else {
            envelope["error"] = "History codec operation failed at " + (allowedPhases.contains(phase) ? phase : "request")
        }
    }
    recordDecodeDiagnostic(request, envelope, codec)
    return envelope
}

while let line = readLine() {
    let result: [String: Any]
    if line.utf8.count > maximumBytes * 2 {
        result = ["ok": false, "error": "History codec request exceeds size limit"]
    } else if let data = line.data(using: .utf8),
              let request = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any] {
        result = response(request)
    } else {
        result = ["ok": false, "error": "Invalid JSON request"]
    }
    if let data = try? JSONSerialization.data(withJSONObject: result, options: [.sortedKeys]) {
        FileHandle.standardOutput.write(data)
        FileHandle.standardOutput.write(Data([10]))
    }
}
