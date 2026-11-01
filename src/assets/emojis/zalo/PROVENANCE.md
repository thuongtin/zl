# Emoji metadata provenance

`index.json` is generated coordinate, alias, Unicode and integrity metadata, not
emoji artwork. It describes 1383 PNG slots, 1440 ASCII aliases and 1295 Unicode
mappings. Coordinates use COLUMN ROW; member paths use row then column. Of the
sprite IDs, 1295 have canonical Unicode and 88 do not. Canonical reverse lookup
chooses the shortest sequence, then lexical order, deterministically.

`slot.ttf` is a project-generated empty-outline font. Its one square-advance
CJK glyph gives epaint 0.29 an indivisible inline image cell and line breaks.
Selection, accessibility and copy restore the raw source text.

The original artwork is not included or downloaded. No redistribution license
for that artwork is asserted. A local pack may be prepared explicitly using
`native/macos/prepare-emoji-assets.py`; see `docs/zalo-message-emoji.md`.
The script verifies members against the committed hashes and writes outside
the repository without regenerating metadata or fonts.

The scalar emoji-property ranges were generated using Swift Unicode properties.
Native fallback renders whole graphemes via CoreText on macOS, not bundled Apple
font bytes. The portable text fallback uses the project's Noto emoji font.
