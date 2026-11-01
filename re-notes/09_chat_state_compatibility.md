# Chat state compatibility evidence

## Phạm vi

Ghi chú này mô tả nguồn kiểm chứng, không chứa session, cookie, capture mạng, nội dung hội thoại thật hoặc định danh tài khoản. Chỉ đã kiểm thử trên macOS arm64. Protocol nội bộ không có cam kết ổn định từ nhà cung cấp.

## Ánh xạ kiểm chứng

| Surface | Implementation | Reproducible evidence |
| --- | --- | --- |
| Wire message identity, raw epochs, unknown fields | `message_model.rs`, `chat.rs`, `store.rs` | `tests/identity_regressions.rs`, `tests/transfer_regressions.rs` |
| Socket direct/group history, lifecycle and cancellation | `socket.rs`, `gui/group_history.rs`, `gui/phone_history.rs` | socket unit fixtures; `native/macos/test-history-codec.py` |
| Historic reaction baseline and replay | `original_reactions.rs`, `reactions.rs`, `reaction_journal.rs` | unit fixtures, `tests/reaction_journal_regressions.rs` |
| Read, delivery, manual unread and visible foreground gate | `read_receipts.rs`, `delivery_receipts.rs`, `unread.rs` | `tests/read_state_regressions.rs`, `tests/delivery_receipts_regressions.rs`, `tests/visible_read_build22.rs`, `tests/local_read_durability_build22.rs` |
| Account storage and restart | `store_v2.rs`, `gui/storage.rs`, `gui/persistence.rs` | `tests/store_v2_regressions.rs`, `tests/schema_migrations.rs`, `tests/unread_roundtrip.rs`, lifecycle/wipe unit fixtures |
| Media resource identity, cache and retry | `media_resources.rs`, `gui/media_preview.rs` | `tests/media_resources_regressions.rs`, `native/macos/test-media-preview.py` |
| Reply, mentions, sidebar metadata | `message_navigation.rs`, `gui/navigation.rs` | `tests/message_navigation_regressions.rs`, `tests/sidebar_presentation_regressions.rs` |
| Alias boundaries and Unicode graphemes | `gui/emoji_text.rs` | synthetic tokenizer fixture and renderer tests; `native/macos/test-emoji-preview.py` |

Fixtures synthetic và local mock HTTP/WebSocket không chứng minh server thật luôn chấp nhận mọi biến thể. Không sửa các phép mã hóa/signature để làm fixture pass. Các opcode và payload trong socket phải được maintainer đối chiếu thêm với runtime trước merge.

## Codec và artwork

History codec dùng helper JavaScriptCore/Security và static zstd. `prepare-history-assets.py` tải public Web assets đã pin SHA-256 khi người build chủ động yêu cầu bằng `--local-use`; hash sai thì dừng. Không commit/downloaded JS/WASM vào Git. Điều kiện phân phối codec assets cần được review trước release; việc tải được không phải bằng chứng license.

Artwork emoji không được đóng gói trong PR. Script chuẩn bị local opt-in từ package trên máy, default dùng Unicode/native fallback. Xem provenance và hướng dẫn emoji. Không đánh đồng việc chưa kèm artwork với việc bỏ renderer, alias tokenizer hoặc cache ra khỏi đóng góp.
