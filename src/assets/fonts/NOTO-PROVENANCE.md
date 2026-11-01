# Bundled Unicode fallback fonts

Source: official Google Fonts repository, pinned commit `6c1dbe27dcf7d817d4b62cec033ba77fa8e2a5ec`.

- `NotoEmoji-Variable.ttf` (1982596 bytes), SHA-256 `de6c18832938afc99caf132b39d6a30a19bac7f2e812e28db2535b4608d27551`
  - https://raw.githubusercontent.com/google/fonts/6c1dbe27dcf7d817d4b62cec033ba77fa8e2a5ec/ofl/notoemoji/NotoEmoji%5Bwght%5D.ttf
- `NotoEmoji-OFL.txt` (4330 bytes), SHA-256 `500bb1ccf43df7bbb522112f9133a52b16e1c35e809632f5d8609b179152de5b`
  - https://raw.githubusercontent.com/google/fonts/6c1dbe27dcf7d817d4b62cec033ba77fa8e2a5ec/ofl/notoemoji/OFL.txt
- `NotoSansMath-Regular.ttf` (1015396 bytes), SHA-256 `3f495fe933c06786e4d5f6d86b8ee70b6753a68ee3b9d87528726de0f6e2c47d`
  - https://raw.githubusercontent.com/google/fonts/6c1dbe27dcf7d817d4b62cec033ba77fa8e2a5ec/ofl/notosansmath/NotoSansMath-Regular.ttf
- `NotoSansMath-OFL.txt` (4380 bytes), SHA-256 `403a95275b469061b7d4371c328e0ada3bc7d63328abe2e88aad5cd243b2fe21`
  - https://raw.githubusercontent.com/google/fonts/6c1dbe27dcf7d817d4b62cec033ba77fa8e2a5ec/ofl/notosansmath/OFL.txt

Noto Sans Math provides mathematical alphanumeric outlines. Noto Emoji is the monochrome TrueType outline variable font, not Noto Color Emoji (CBDT/CBLC). Font bytes are unmodified. Both are licensed under SIL OFL 1.1; the complete upstream license notices are included alongside the fonts.

Runtime fallback keeps existing sans-serif text and Lucide icons before these fonts. The app does not normalize or rewrite stored Unicode text. Color emoji, ZWJ ligature shaping and skin-tone/flag composition are not provided by egui 0.29 character-by-character layout.
