# ADR-0001: Tách trạng thái hội thoại theo tài khoản

- **Status:** proposed
- **Date:** 2026-11-01
- **Deciders:** Maintainers review pending

## Context

Lịch sử, reaction, unread và media cùng phụ thuộc định danh tài khoản, hội thoại và tin nhắn. Việc mở chat hoặc tải một trang cũ không được tự biến thành hành động đã đọc. Reply và reaction cần giữ cả server ID lẫn client ID; không suy ra định danh chỉ từ nội dung hay thời gian gần nhau.

## Decision

Đề xuất tách model, storage, transport và GUI wiring thành chuỗi PR nhỏ theo dependency. Mỗi lớp phải build được; adapter chỉ được bật cùng model/store/worker contract cần thiết. Các PR phụ thuộc review trên fork để không cộng dồn diff, rồi rebase và gửi upstream lần lượt. `AccountStore` lưu tại `accounts/<digest>/history-v2.sqlite3`; legacy source được import có manifest và giữ nguyên. Worker sở hữu ghi dữ liệu, dùng generation/cancellation để loại kết quả của tài khoản cũ. UI giữ cửa sổ lịch sử có giới hạn, gửi read intent khi nội dung thực sự hiển thị ở cửa sổ foreground đủ thời gian.

Giữ raw wire identity và raw payload cho replay tương thích. Reaction có journal riêng, baseline có provenance và replay idempotent. Local read được lưu trước, độc lập thành công của network receipt. Không đưa session vào SQLite. Session được ghi tệp tạm private rồi rename.

## Consequences

- Positive: tách tài khoản, giữ dữ liệu qua restart, tránh unread/reaction bị hồi quy do cache cũ hoặc kết quả đến muộn.
- Negative: thay đổi chéo nhiều module; cần human audit schema/protocol trước merge. Đây không phải tuyên bố parity hoàn toàn với Zalo gốc.
- Migration: `user_version` 0→1→2 trong transaction, WAL/NORMAL/busy timeout và newer-version/owner guard trước mutation. Human audit policy vẫn chờ maintainer; không down-migration.
- Wipe: account generation/cancellation fence; quarantine cache/composer/media trước SQL clear, rollback khi lỗi trước commit. Sau commit không trả lỗi khiến UI giữ snapshot cũ; durable marker chặn reimport, giữ session và tài khoản khác.
- Shutdown: final snapshot/read/draft đi qua FIFO, cancel/drain producers rồi flush ACK; GUI chỉ đóng khi thành công, main bounded join trước portable update launch. Profile lookup hoặc trang HTTP chậm không giữ received delta chưa persist.
- Follow-ups: helper matrix đã cross-compile/validate arm64/Intel/universal; runtime chỉ kiểm trên Mac arm64. Full release/DMG, Windows/Linux, permission GUI và soak dài vẫn chưa được nghiệm thu. UI chức năng được đóng góp nhưng thiết kế chưa tối ưu.

## Alternatives Considered

| Option | Why rejected |
| --- | --- |
| Chỉ sửa renderer | Không sửa nguồn trạng thái hoặc persistence gây ra lỗi hiển thị |
| Thay toàn bộ cache JSON sau mỗi event | Chi phí tăng theo toàn bộ lịch sử, dễ ghi đè delta đến muộn |
| Xóa legacy ngay sau import | Làm mất đường phục hồi trước khi schema mới được nghiệm thu |
| Bật lại wipe legacy nguyên trạng | Không bao phủ V2, có nguy cơ import dữ liệu đã xóa trở lại |

## Verification and rollback

Xem `docs/chat-feature-contribution.md` và `re-notes/09_chat_state_compatibility.md`. Rollback source không phải down-migration. Trước thử trên dữ liệu thật, sao lưu riêng thư mục dữ liệu và đóng app bình thường. Giữ nguyên legacy; không dùng bản cũ ghi vào kho V2. Không commit backup tài khoản hoặc capture thô.
