# Đóng góp tính năng hội thoại theo từng lớp

## Phạm vi và thứ tự review

Chuỗi PR thay thế monolith #42, không loại emoji, ảnh/video hoặc các thay đổi UI cần cho chức năng. **Giao diện chưa được tối ưu/hoàn thiện về thiết kế. Chỉ đã kiểm thử trên Mac, cụ thể macOS arm64.** Windows, Linux và Mac Intel chưa được nghiệm thu runtime. Cross-compile Intel/Universal không thay thế kiểm thử trên máy đích.

Các phần độc lập gửi trực tiếp sang upstream; phần phụ thuộc được xếp thành stacked PR trên fork để mỗi diff chỉ chứa delta của một lớp. Sau khi dependency vào upstream, rebase và gửi từng lớp, không mở nhiều PR cộng dồn cùng một diff lớn. PR draft trong chuỗi là review queue, không phải release hoặc xác nhận maintainer đã chấp thuận.

Thứ tự nhóm:

1. Time/data directory nền; tách format cơ học.
2. Message/reaction/unread models và private atomic session writer.
3. Bounded media resources và transactional updater engine.
4. Chat parser/identity/sidebar metadata, transfer/preview renewal và socket contract.
5. Read/delivery/presence semantics, account store/migration, reaction journal, runtime drafts/outbox và notification IPC.
6. Native helpers/build matrix, emoji/fonts, lifecycle, worker/history, phone history, renderer/navigation/sidebar/composer và modal cleanup.
7. Synthetic benchmark, tài liệu và kiểm tra integration tip.

## Chức năng giữ đầy đủ

- Emoji trong bubble/sidebar: alias longest-match/boundary, Unicode grapheme/ZWJ/flag/keycap, raw selection/copy, native color fallback, Noto Emoji/Math OFL và sprite artwork opt-in local.
- Media: metadata đa dạng, photo/video thumbnail, photo source fallback, video source không dùng làm ảnh, signed URL giữ nguyên bytes, bounded input/pixel/cache/queue/retry và native ImageIO/viewer. Preview hết hạn có thể cần server cấp lại; không khẳng định mọi media cũ đều phục hồi được.
- Lịch sử 1-1/nhóm: socket pull, pagination, bounded UI window và sync archive từ điện thoại qua native codec. Sync điện thoại chỉ bắt đầu khi có yêu cầu.
- Reaction: provenance baseline, journal riêng tài khoản, idempotent replay theo target ID, aggregate không bị double-count hoặc mất khi lịch sử/live đến khác thứ tự.
- Read/unread/receipt: server metadata, manual unread/mention/mute, badge/tên in đậm, foreground/dwell/visible-row gate, local read bền qua restart và không phụ thuộc network ACK.
- Điều hướng/presentation: bấm reply tìm/cuộn tới tin gốc, clickable mention UTF-16, tên người gửi cuối, avatar collage nhóm, ngày sau 24 giờ và presence không tự mặc định vừa truy cập.
- Persistence/runtime: Application Support/`--data-dir`, private atomic session, account V2 store, giữ legacy source khi import, generation/cancellation, bounded snapshot queue, durable drafts/outbox và shutdown ACK/join.
- Native notification, attachment picker và synthetic storage benchmark. Tính năng UI có trong đóng góp nhưng thiết kế chưa được tối ưu.

## Build checkout sạch trên Mac

```sh
export CMAKE_POLICY_VERSION_MINIMUM=3.5
cargo test --locked
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
SWIFT_TARGETS=arm64-apple-macosx11.0 bash native/macos/build-native-helpers.sh
cargo build --release --locked --bin ctdozlite
```

Builder tạo đủ năm helper: `EmojiPreview`, `MediaPreview`, `DailyMediaViewer`, `DailyNotifier.app` và `HistoryCodec`. zstd 1.5.7 được pin SHA-256 và build static theo kiến trúc, không cần Homebrew dylib. Các bước này chỉ ghi build output, không thay app trong Applications.

Public CI build/validate/sign đủ helper arm64/x86_64/universal và không tải/copy JS/WASM codec ngoài. HistoryCodec báo `history_codec_assets_missing` nếu chưa chuẩn bị codec assets; đây là trạng thái có chủ đích, không phải sync thành công.

### Codec opt-in local

```sh
python3 native/macos/prepare-history-assets.py --local-use \
  --output /absolute/path/HistoryCodecAssets
ZL_HISTORY_CODEC_ASSETS=/absolute/path/HistoryCodecAssets \
  target/release/ctdozlite --gui --data-dir /tmp/chat-feature-test
```

Đường dẫn override runtime phải tuyệt đối. Assets được kiểm hash; không commit hoặc tự phân phối chúng. Xem `packaging/macos/NATIVE_HELPERS.md` về local bundle opt-in, public validator và kiến trúc. Việc tải được assets không phải bằng chứng quyền phân phối.

## Kiểm tra và giới hạn

```sh
python3 tests/native_packaging.py
python3 native/macos/test-emoji-preview.py
python3 native/macos/test-media-preview.py
python3 native/macos/test-history-codec.py
cargo run --release --locked --bin daily_parity_bench -- \
  --messages 5000 --switches 200 --soak-seconds 2 \
  --output /tmp/chat-feature-metrics.json
```

Rust regressions dùng synthetic fixtures/disposable storage hoặc loopback HTTP/WebSocket, không gọi tài khoản Zalo thật. Benchmark mô phỏng storage/window; nó không điều khiển GUI và không chứng minh app chạy 60 phút không crash hoặc tăng RAM. Một số visual/native opt-in tests được ignore mặc định; kiểm chúng cần môi trường/asset tương ứng.

- Shutdown: enqueue final snapshot/read/draft, cancel và drain task, flush rồi ACK; GUI chỉ đóng khi ACK thành công, main có bounded join. Lỗi flush giữ cửa sổ và cho retry. Received deltas được publish/persist trước profile lookup hoặc trang HTTP kế tiếp.
- SQLite: schema `user_version` 0→1→2, migration transaction, WAL/NORMAL/busy timeout, readonly newer-version/owner guard trước mutation. Không có down-migration.
- Wipe: account/generation fence, quarantine cache/composer/media trước SQL clear, rollback khi lỗi trước commit; cleanup quarantine sau commit là best-effort để không giữ UI snapshot cũ rồi tái ghi dữ liệu. Session/account khác và legacy source được giữ; durable marker ngăn import lại dữ liệu đã xóa.
- Update: bounded/hash-validated download, atomic stage/backup, portable launch sau shutdown/join và rollback khi launch lỗi. Unit tests không mở installer hoặc app thật.

Manual checklist sau human audit: dùng data-dir riêng, đăng nhập; so emoji/raw copy; mở ảnh/video mới/cũ; sync 1-1/nhóm lúc điện thoại rảnh; kiểm reaction aggregate; đọc chat foreground rồi restart; reply/mention jump; đổi tài khoản không lẫn dữ liệu. Không đăng log/capture hoặc dữ liệu tài khoản lên PR.

## Rủi ro còn mở và rollback

Emoji artwork độc quyền không nằm trong Git; alias không có Unicode tương đương giữ raw text khi chưa có pack local. Protocol/codec/Web assets có thể đổi; fixture pass không chứng minh server chấp nhận mọi biến thể. Notification/viewer permission, accessibility, visual QA và GUI soak dài chưa được nghiệm thu đầy đủ. Không khẳng định parity hoàn toàn với Zalo gốc hoặc release đa nền tảng đã được chứng nhận.

Xem ADR và protocol evidence map. Rollback source bằng revert lớp liên quan trên nhánh thử riêng; giữ backup storage và legacy. Không dùng app cũ ghi vào schema mới hơn, không xóa dữ liệu hoặc cache để che lỗi và không coi source rollback là down-migration.

## Rebase upstream mới nhất

Chuỗi đã tích hợp upstream `1345f5d` ngày 2026-11-01: tên binary `ctdozlite`, tối ưu lịch sử/render/font CJK, avatar nhóm và build Windows/release. Giữ riêng các lớp chức năng và test hồi quy; không dùng heuristics cùng nội dung/thời gian để gộp những message có ID khác nhau. Preview trong bubble giới hạn 480 px, viewer raster dùng quality riêng trong pixel budget. Không thay phiên hoặc kho dữ liệu thật khi rebase/test.
