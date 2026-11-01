# Emoji trong tin nhắn

## Mặc định khi clone

Bản build công khai không chứa artwork Zalo. Cargo luôn ghi pack rỗng trong
`OUT_DIR` nếu không có `ZL_EMOJI_PACK`, kể cả sau một lần build có pack. Build
không tự tải mạng hoặc giải nén ứng dụng. Unicode được giữ nguyên theo grapheme;
macOS có thể dùng helper CoreText, còn trường hợp helper thiếu, lỗi hoặc đang
chờ thì hiển thị glyph Noto thay vì ô trống.

Alias có Unicode canonical chuyển sang Unicode để hiển thị nhưng selection,
copy và nội dung lưu vẫn giữ raw alias. 88 sprite không có Unicode canonical
được giữ nguyên dạng văn bản, không đoán biểu tượng thay thế. Reverse map dùng
Unicode ngắn nhất rồi thứ tự lexical để kết quả không phụ thuộc HashMap.

## Artwork local tùy chọn

Chỉ thực hiện khi chủ động chọn nguồn local phù hợp. Script không tải dữ liệu
và yêu cầu output ngoài repository:

```sh
python3 native/macos/prepare-emoji-assets.py \
  --asar /path/to/local/app.asar \
  --output-pack /tmp/local-emoji/sprites.bin
ZL_EMOJI_PACK=/tmp/local-emoji/sprites.bin cargo build --locked
```

Mỗi sprite chỉ dùng renderer original khi bounds, kích thước metadata, PNG
signature và SHA-256 hợp lệ. Pack thiếu, hỏng hoặc không đúng index sẽ dùng
Unicode canonical hoặc raw alias. Pack không được thêm vào Git; việc chuẩn bị
pack không xác lập quyền phân phối artwork.

## Kiểm thử

```sh
cargo test --locked gui::emoji_text --lib
ZL_EMOJI_PACK=/tmp/local-emoji/sprites.bin cargo test --locked --lib \
  all_assets_are_exact_original_pngs -- --ignored
```

Suite mặc định dùng PNG tổng hợp để kiểm decode, hash/bounds, cache, layout,
copy raw và fallback; không yêu cầu ứng dụng Zalo hoặc pack local. Test original
1383 PNG và các export hình headless là opt-in có mô tả trong thuộc tính ignore.
Headless chỉ chứng minh layout/painter, không thay nghiệm thu giao diện thật.
