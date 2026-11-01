# Native helpers trong gói macOS

`SWIFT_TARGETS=arm64-apple-macosx11.0,x86_64-apple-macosx11.0 bash native/macos/build-native-helpers.sh /path/to/ctdozlite.app` build đủ EmojiPreview, MediaPreview, DailyMediaViewer, DailyNotifier và HistoryCodec. Gói đơn kiến trúc truyền một target tương ứng. `NATIVE_BUILD_DIR` đổi thư mục cache/output. Binary chính trong bundle là `Contents/MacOS/ctdozlite`, đồng nhất với tên mới của upstream.

HistoryCodec liên kết tĩnh zstd 1.5.7, tải source theo version và SHA-256 cố định, build riêng từng kiến trúc bằng SDK macOS. Không dùng thư viện Homebrew động hoặc archive host. Gói app kèm BSD license của zstd trong `Contents/Resources/ThirdPartyLicenses`.

## Tài nguyên codec bên ngoài

Chưa có bằng chứng về quyền phân phối lại JS/WASM Zalo. Vì vậy build public/CI không tải và không đóng gói các tài nguyên đó, kể cả khi cache local đã có. Helper vẫn có trong app và trả lỗi `history_codec_assets_missing` cùng hướng dẫn khi init thiếu tài nguyên. Không coi việc source có thể tải công khai là quyền phân phối lại.

Người dùng tự chuẩn bị cho local bằng lệnh opt-in:

```sh
python3 native/macos/prepare-history-assets.py --local-use --output /path/to/HistoryCodecAssets
python3 native/macos/prepare-history-assets.py --verify-only --output /path/to/HistoryCodecAssets
```

Truyền `assetsDir` trong JSON `init` của helper, hoặc khi build local đặt `HISTORY_CODEC_ASSETS=/path/to/HistoryCodecAssets` để copy các byte đã xác minh vào app local. CI xóa biến này và validator public từ chối thư mục asset. Nếu phân phối public cần đầy đủ history, phải giải quyết quyền phân phối trước hoặc thay codec bằng implementation độc lập có kiểm chứng. Không tự công bố gói local này.

## Kiểm tra

```sh
python3 tests/native_packaging.py
python3 packaging/macos/validate-native-bundle.py /path/to/ctdozlite.app --arches arm64,x86_64
```

Validator kiểm tất cả Mach-O trong bundle có đúng tập kiến trúc, không liên kết Homebrew, có license, không asset ngoài; chuyển app sang thư mục temporary rồi chạy contract emoji/media/history không mở UI. Trên host arm64, slice Intel chỉ được build/kiểm metadata, không tuyên bố runtime Intel. DailyNotifier và DailyMediaViewer chưa được nghiệm thu GUI/permission trên máy người dùng bằng bài kiểm này. Test matrix dùng main executable synthetic, không thay thế full release build Rust/DMG hoặc UI QA.
