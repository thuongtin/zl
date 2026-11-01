//! Inline original emoji artwork with grapheme-safe native color fallback.
use super::*;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex, OnceLock};
use unicode_segmentation::UnicodeSegmentation;

const CACHE_LIMIT: usize = 256;
const QUEUE_LIMIT: usize = 32;
const DECODE_PER_PASS: usize = 8;
// A dedicated empty CJK glyph provides a square advance and legal line-break
// opportunities around images in epaint 0.29. It never replaces stored text.
const SLOT: &str = "\u{4e00}";
const PACK: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/emoji-sprites.bin"));

#[derive(Deserialize)]
struct Sprite {
    offset: usize,
    length: usize,
    width: usize,
    height: usize,
    sha256: String,
}
#[derive(Deserialize)]
struct Index {
    sprites: Vec<Sprite>,
    aliases: HashMap<String, u16>,
    unicode: HashMap<String, u16>,
    emoji_scalar_ranges: Vec<[u32; 2]>,
}
#[derive(Default)]
struct TrieNode {
    children: HashMap<u8, usize>,
    sprite: Option<u16>,
}
struct Catalog {
    index: Index,
    trie: Vec<TrieNode>,
    canonical: HashMap<u16, String>,
    validated: Vec<bool>,
}
fn catalog() -> &'static Catalog {
    static CATALOG: OnceLock<Catalog> = OnceLock::new();
    CATALOG.get_or_init(|| {
        let index: Index = serde_json::from_str(include_str!("../assets/emojis/zalo/index.json"))
            .expect("Bundled emoji index must be valid");
        let mut trie = vec![TrieNode::default()];
        for (key, &sprite) in &index.aliases {
            let mut node = 0;
            for byte in key.bytes().map(|b| b.to_ascii_lowercase()) {
                node = if let Some(&next) = trie[node].children.get(&byte) {
                    next
                } else {
                    let next = trie.len();
                    trie.push(TrieNode::default());
                    trie[node].children.insert(byte, next);
                    next
                };
            }
            trie[node].sprite = Some(sprite);
        }
        let mut canonical = HashMap::new();
        let mut unicode: Vec<_> = index.unicode.iter().collect();
        // Prefer the shortest sequence, then lexical order, independent of HashMap order.
        unicode.sort_by_key(|(text, _)| (text.len(), *text));
        for (text, &sprite) in unicode {
            canonical.entry(sprite).or_insert_with(|| text.clone());
        }
        let validated = index
            .sprites
            .iter()
            .map(|sprite| valid_sprite(PACK, sprite))
            .collect();
        Catalog {
            index,
            trie,
            canonical,
            validated,
        }
    })
}

fn valid_sprite(pack: &[u8], sprite: &Sprite) -> bool {
    let Some(end) = sprite.offset.checked_add(sprite.length) else {
        return false;
    };
    let Some(bytes) = pack.get(sprite.offset..end) else {
        return false;
    };
    sprite.width == 64
        && matches!(sprite.height, 63 | 64)
        && sprite.length <= 128 * 1024
        && bytes.starts_with(b"\x89PNG\r\n\x1a\n")
        && format!("{:x}", Sha256::digest(bytes)) == sprite.sha256
}

fn sprite_image(sprite: u16) -> Option<ImageKey> {
    let catalog = catalog();
    if catalog.validated.get(sprite as usize) == Some(&true) {
        Some(ImageKey::Original(sprite))
    } else {
        catalog
            .canonical
            .get(&sprite)
            .map(|text| ImageKey::Native(text.clone()))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum ImageKey {
    Original(u16),
    Native(String),
}
#[derive(Clone, Debug)]
struct Part<'a> {
    raw: &'a str,
    image: Option<ImageKey>,
}

// Match longest ASCII aliases first, then apply the original two-sided boundary
// policy to all candidates, including adjacent candidates later rejected.
fn aliases(text: &str) -> Vec<(usize, usize, u16)> {
    let trie = &catalog().trie;
    let bytes = text.as_bytes();
    let normalized: Vec<_> = text
        .char_indices()
        .map(|(offset, chr)| {
            let mut lower = chr.to_lowercase();
            let folded = lower
                .next()
                .filter(|c| c.is_ascii() && lower.next().is_none())
                .map(|c| c as u8);
            (offset, folded)
        })
        .collect();
    let mut candidates = Vec::new();
    let mut start = 0;
    while start < normalized.len() {
        let mut node = 0;
        let mut cursor = start;
        let mut found = None;
        while cursor < normalized.len() {
            let Some(byte) = normalized[cursor].1 else {
                break;
            };
            let Some(&next) = trie[node].children.get(&byte) else {
                break;
            };
            node = next;
            cursor += 1;
            if let Some(sprite) = trie[node].sprite {
                let end = normalized.get(cursor).map_or(text.len(), |v| v.0);
                found = Some((normalized[start].0, end, sprite, cursor));
            }
        }
        if let Some((a, b, sprite, next)) = found {
            start = next;
            candidates.push((a, b, sprite));
        } else {
            start += 1;
        }
    }
    let starts: HashSet<_> = candidates.iter().map(|v| v.0).collect();
    let ends: HashSet<_> = candidates.iter().map(|v| v.1).collect();
    candidates
        .into_iter()
        .filter(|&(a, b, _)| {
            (a == 0 || matches!(bytes[a - 1], b' ' | b'\n') || ends.contains(&a))
                && (b == bytes.len() || matches!(bytes[b], b' ' | b'\n') || starts.contains(&b))
        })
        .collect()
}

fn unicode_image(cluster: &str) -> Option<ImageKey> {
    if cluster.contains('\u{fe0e}') {
        return None;
    } // Explicit text presentation.
    if let Some(&sprite) = catalog().index.unicode.get(cluster) {
        return sprite_image(sprite);
    }
    // Normalize only a terminal presentation selector, never skin modifiers/ZWJ.
    if let Some(base) = cluster.strip_suffix('\u{fe0f}') {
        if let Some(&sprite) = catalog().index.unicode.get(base) {
            return sprite_image(sprite);
        }
    }
    let is_emoji = cluster.chars().any(|c| {
        let code = c as u32;
        code > 0x7f
            && catalog()
                .index
                .emoji_scalar_ranges
                .iter()
                .any(|&[a, b]| a <= code && code <= b)
    }) || (cluster.contains('\u{20e3}')
        && cluster.starts_with(|c: char| c.is_ascii_digit() || matches!(c, '#' | '*')));
    (is_emoji && cluster.len() <= 256).then(|| ImageKey::Native(cluster.to_owned()))
}

fn parts<'a>(text: &'a str) -> Vec<Part<'a>> {
    let mut parts = Vec::new();
    let mut cursor = 0;
    let append_text = |slice: &str, offset: usize, out: &mut Vec<Part<'a>>| {
        // Lifetime comes from the original text rather than this temporary slice.
        for (index, cluster) in slice.grapheme_indices(true) {
            let raw = &text[offset + index..offset + index + cluster.len()];
            out.push(Part {
                raw,
                image: unicode_image(raw),
            });
        }
    };
    for (start, end, sprite) in aliases(text) {
        append_text(&text[cursor..start], cursor, &mut parts);
        parts.push(Part {
            raw: &text[start..end],
            image: sprite_image(sprite),
        });
        cursor = end;
    }
    append_text(&text[cursor..], cursor, &mut parts);
    parts
}

#[derive(Clone)]
enum Cached {
    Ready(TextureHandle),
    Failed,
}
struct EmojiCache {
    entries: HashMap<ImageKey, Cached>,
    order: VecDeque<ImageKey>,
    pending: HashSet<String>,
    sender: Option<std::sync::mpsc::SyncSender<(String, egui::Context)>>,
    decode_pass: u64,
    decoded_this_pass: usize,
    total_decodes: usize,
}
impl EmojiCache {
    fn get(&mut self, key: &ImageKey) -> Option<Cached> {
        let value = self.entries.get(key)?.clone();
        if matches!(key, ImageKey::Native(_)) {
            self.order.retain(|k| k != key);
            self.order.push_back(key.clone());
        }
        Some(value)
    }
    fn insert(&mut self, key: ImageKey, value: Cached) {
        // The original catalog is finite (1383 x <=64x64, under 22 MiB RGBA).
        // Keep each original once to avoid LRU thrash on emoji-heavy messages.
        // Only native fallback entries use the separate 256-entry LRU budget.
        if matches!(key, ImageKey::Native(_)) {
            self.order.retain(|k| k != &key);
            self.order.push_back(key.clone());
        }
        self.entries.insert(key, value);
        while self.order.len() > CACHE_LIMIT {
            if let Some(old) = self.order.pop_front() {
                self.entries.remove(&old);
            }
        }
    }
}
type SharedCache = Arc<Mutex<EmojiCache>>;
fn cache(ctx: &egui::Context) -> SharedCache {
    let id = Id::new("inline-original-emoji-cache-v1");
    ctx.data_mut(|data| {
        if let Some(cache) = data.get_temp::<SharedCache>(id) {
            return cache;
        }
        let (tx, rx) = std::sync::mpsc::sync_channel::<(String, egui::Context)>(QUEUE_LIMIT);
        let cache = Arc::new(Mutex::new(EmojiCache {
            entries: HashMap::new(),
            order: VecDeque::new(),
            pending: HashSet::new(),
            sender: Some(tx),
            decode_pass: 0,
            decoded_this_pass: 0,
            total_decodes: 0,
        }));
        let weak = Arc::downgrade(&cache);
        std::thread::spawn(move || {
            while let Ok((cluster, ctx)) = rx.recv() {
                let image = native_preview(&cluster).and_then(|bytes| decode_image(&bytes));
                let result = match image {
                    Some(image) => Cached::Ready(ctx.load_texture(
                        "native-inline-emoji",
                        image,
                        TextureOptions::LINEAR,
                    )),
                    None => Cached::Failed,
                };
                let Some(cache) = weak.upgrade() else { break };
                if let Ok(mut c) = cache.lock() {
                    c.pending.remove(&cluster);
                    c.insert(ImageKey::Native(cluster), result);
                }
                ctx.request_repaint();
            }
        });
        data.insert_temp(id, cache.clone());
        cache
    })
}
fn decode_image(bytes: &[u8]) -> Option<egui::ColorImage> {
    if bytes.len() > 128 * 1024 {
        return None;
    }
    let image = image::load_from_memory_with_format(bytes, image::ImageFormat::Png)
        .ok()?
        .to_rgba8();
    if image.width() != 64 || !matches!(image.height(), 63 | 64) {
        return None;
    }
    Some(egui::ColorImage::from_rgba_unmultiplied(
        [64, image.height() as usize],
        image.as_raw(),
    ))
}
fn image(ctx: &egui::Context, key: &ImageKey) -> Option<Cached> {
    let shared = cache(ctx);
    let mut cache = shared.lock().ok()?;
    if let Some(value) = cache.get(key) {
        return Some(value);
    }
    match key {
        ImageKey::Original(id) => {
            let sprite = catalog().index.sprites.get(*id as usize)?;
            if catalog().validated.get(*id as usize) != Some(&true) {
                return Some(Cached::Failed);
            }
            let bytes = PACK.get(sprite.offset..sprite.offset.checked_add(sprite.length)?)?;
            load_original(ctx, &mut cache, key, sprite, bytes)
        }
        ImageKey::Native(cluster) => {
            if !cache.pending.contains(cluster)
                && cache.pending.len() < QUEUE_LIMIT
                && cache
                    .sender
                    .as_ref()?
                    .try_send((cluster.clone(), ctx.clone()))
                    .is_ok()
            {
                cache.pending.insert(cluster.clone());
            }
            None
        }
    }
}

fn load_original(
    ctx: &egui::Context,
    cache: &mut EmojiCache,
    key: &ImageKey,
    sprite: &Sprite,
    bytes: &[u8],
) -> Option<Cached> {
    let pass = ctx.cumulative_pass_nr();
    if cache.decode_pass != pass {
        cache.decode_pass = pass;
        cache.decoded_this_pass = 0;
    }
    if cache.decoded_this_pass >= DECODE_PER_PASS {
        ctx.request_repaint();
        return None;
    }
    let decoded = decode_image(bytes)?;
    if decoded.size != [sprite.width, sprite.height] {
        return None;
    }
    cache.decoded_this_pass += 1;
    cache.total_decodes += 1;
    let texture = ctx.load_texture("original-inline-emoji", decoded, TextureOptions::LINEAR);
    let value = Cached::Ready(texture);
    cache.insert(key.clone(), value.clone());
    Some(value)
}

#[cfg(test)]
pub(super) fn seed_synthetic_images(ctx: &egui::Context, text: &str) {
    // Isolate renderer integration from optional artwork and async native codecs.
    let texture = ctx.load_texture(
        "synthetic-emoji-integration",
        egui::ColorImage::new([64, 64], Color32::from_rgb(240, 100, 20)),
        TextureOptions::LINEAR,
    );
    let shared = cache(ctx);
    let mut cache = shared.lock().unwrap();
    for part in parts(text) {
        if let Some(key) = part.image {
            cache.insert(key, Cached::Ready(texture.clone()));
        }
    }
}

#[cfg(target_os = "macos")]
fn native_preview(cluster: &str) -> Option<Vec<u8>> {
    use std::io::{Read, Write};
    use std::process::{Command, Stdio};
    if cluster.len() > 256 || cluster.graphemes(true).count() != 1 {
        return None;
    }
    let packaged = std::env::current_exe()
        .ok()?
        .parent()?
        .parent()?
        .join("Resources/EmojiPreview");
    let helper = if packaged.is_file() {
        packaged
    } else {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("native/macos/build/EmojiPreview")
    };
    let mut child = Command::new(helper)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    if child.stdin.take()?.write_all(cluster.as_bytes()).is_err() {
        let _ = child.kill();
        let _ = child.wait();
        return None;
    }
    let stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout.take(128 * 1024 + 1).read_to_end(&mut bytes).ok()?;
        Some(bytes)
    });
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    let success = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.success(),
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(5))
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break false;
            }
        }
    };
    let bytes = reader.join().ok()??;
    (success && bytes.len() <= 128 * 1024).then_some(bytes)
}
#[cfg(not(target_os = "macos"))]
fn native_preview(_: &str) -> Option<Vec<u8>> {
    None
}

struct InlineLayout {
    galley: Arc<egui::Galley>,
    images: HashMap<u32, ImageKey>,
    raw_images: HashMap<u32, String>,
}
fn layout(ui: &egui::Ui, text: &str, size: f32, color: Color32, width: f32) -> InlineLayout {
    layout_with_mode(ui, text, size, color, width, false)
}
fn layout_with_mode(
    ui: &egui::Ui,
    text: &str,
    size: f32,
    color: Color32,
    width: f32,
    single_line: bool,
) -> InlineLayout {
    let mut job = egui::text::LayoutJob::default();
    job.wrap.max_width = width.max(1.0);
    if single_line {
        job.wrap.max_rows = 1;
        job.wrap.break_anywhere = true;
        // Epaint inherits the last section's format for its ellipsis. That
        // section may be an empty emoji slot, so paint the ellipsis separately.
        job.wrap.overflow_character = None;
        job.break_on_newline = false;
    }
    let mut images = HashMap::new();
    let mut raw_images = HashMap::new();
    let emoji_size = (size * 1.43).round();
    let plain = egui::TextFormat {
        font_id: FontId::proportional(size),
        color,
        ..Default::default()
    };
    for part in parts(text) {
        if let Some(key) = part.image {
            let section = job.sections.len() as u32;
            job.append(
                SLOT,
                0.0,
                egui::TextFormat {
                    font_id: FontId::new(emoji_size, egui::FontFamily::Name("emoji_slot".into())),
                    line_height: Some(emoji_size),
                    color: Color32::TRANSPARENT,
                    valign: Align::Center,
                    ..Default::default()
                },
            );
            images.insert(section, key);
            raw_images.insert(section, part.raw.to_owned());
        } else {
            // Coalesce text runs so ordinary messages retain normal kerning/layout.
            if let Some(last) = job.sections.last_mut().filter(|s| s.format == plain) {
                job.text.push_str(part.raw);
                last.byte_range.end = job.text.len();
            } else {
                job.append(part.raw, 0.0, plain.clone());
            }
        }
    }
    InlineLayout {
        galley: ui.fonts(|f| f.layout_job(job)),
        images,
        raw_images,
    }
}
pub(super) fn measure(ui: &egui::Ui, text: &str, size: f32, width: f32) -> Vec2 {
    layout(ui, text, size, Color32::WHITE, width).galley.size()
}
// Restore the original text/cursor domain without laying it out again. Each
// image keeps one visible cell; extra raw scalars have zero advance at the same
// midpoint so pointer hit testing jumps over the complete alias/grapheme.
fn selection_galley(layout: &InlineLayout) -> Arc<egui::Galley> {
    let mut galley = (*layout.galley).clone();
    let mut job = (*galley.job).clone();
    let old = job.text.clone();
    job.text.clear();
    for (index, section) in job.sections.iter_mut().enumerate() {
        let raw = layout
            .raw_images
            .get(&(index as u32))
            .map(String::as_str)
            .unwrap_or(&old[section.byte_range.clone()]);
        let start = job.text.len();
        job.text.push_str(raw);
        section.byte_range = start..job.text.len();
    }
    galley.job = Arc::new(job);
    for row in &mut galley.rows {
        let mut expanded = Vec::with_capacity(row.glyphs.len());
        for glyph in &row.glyphs {
            if let Some(raw) = layout.raw_images.get(&glyph.section_index) {
                for (index, chr) in raw.chars().enumerate() {
                    let mut copy = *glyph;
                    copy.chr = chr;
                    if index > 0 {
                        copy.pos.x += glyph.advance_width * 0.5;
                        copy.advance_width = 0.0;
                    }
                    expanded.push(copy);
                }
            } else {
                expanded.push(*glyph);
            }
        }
        row.glyphs = expanded;
    }
    Arc::new(galley)
}

pub(super) fn show(
    ui: &mut egui::Ui,
    text: &str,
    size: f32,
    color: Color32,
    width: f32,
) -> egui::Response {
    let layout = layout(ui, text, size, color, width);
    if layout.images.is_empty() {
        return ui.add(egui::Label::new(RichText::new(text).size(size).color(color)).wrap());
    }
    // Keep standard label selection, accessibility, multi-label copy and raw
    // clipboard content. The internal slot character never reaches selection.
    let response = ui.add(egui::Label::new(selection_galley(&layout)));
    paint_images(ui, &layout, response.rect.min, ui.clip_rect(), color);
    response
}

/// Use the same galley for selection, emoji artwork and mention hit testing.
/// The byte cursor follows the restored raw text, including emoji aliases.
pub(super) fn show_with_mentions(
    ui: &mut egui::Ui,
    text: &str,
    size: f32,
    color: Color32,
    width: f32,
    mentions: &[crate::message_navigation::MentionSpan],
) -> Option<usize> {
    if mentions.is_empty() {
        show(ui, text, size, color, width);
        return None;
    }
    let layout = layout(ui, text, size, color, width);
    let galley = selection_galley(&layout);
    let response = ui.add(egui::Label::new(galley.clone()));
    paint_images(ui, &layout, response.rect.min, ui.clip_rect(), color);
    let rectangles = mention_rectangles(&galley, mentions);
    let mut clicked = None;
    for (index, row, rect) in rectangles {
        let rect = rect.translate(response.rect.min.to_vec2());
        if !ui.is_rect_visible(rect) {
            continue;
        }
        let hit = ui
            .interact(
                rect,
                response.id.with(("mention", index, row)),
                Sense::click(),
            )
            .on_hover_cursor(CursorIcon::PointingHand);
        hit.widget_info(|| {
            egui::WidgetInfo::labeled(
                egui::WidgetType::Link,
                ui.is_enabled(),
                format!("Mở hồ sơ {}", &text[mentions[index].byte_range.clone()]),
            )
        });
        ui.painter().line_segment(
            [rect.left_bottom(), rect.right_bottom()],
            Stroke::new(0.7, color),
        );
        if hit.clicked() {
            clicked = Some(index);
        }
    }
    clicked
}

fn mention_rectangles(
    galley: &egui::Galley,
    mentions: &[crate::message_navigation::MentionSpan],
) -> Vec<(usize, usize, Rect)> {
    let mut byte = 0;
    let mut rectangles = Vec::new();
    for (row_index, row) in galley.rows.iter().enumerate() {
        let mut row_rectangles: HashMap<usize, Rect> = HashMap::new();
        for glyph in &row.glyphs {
            let end = byte + glyph.chr.len_utf8();
            for (index, span) in mentions.iter().enumerate() {
                if span.byte_range.start <= byte && end <= span.byte_range.end {
                    let rect = glyph.logical_rect();
                    row_rectangles
                        .entry(index)
                        .and_modify(|old| *old = old.union(rect))
                        .or_insert(rect);
                }
            }
            byte = end;
        }
        rectangles.extend(
            row_rectangles
                .into_iter()
                .map(|(index, rect)| (index, row_index, rect)),
        );
        if row.ends_with_newline {
            byte += 1;
        }
    }
    rectangles
}

fn paint_images(ui: &egui::Ui, layout: &InlineLayout, origin: Pos2, clip: Rect, color: Color32) {
    let origin = origin.to_vec2();
    let painter = ui.painter().with_clip_rect(ui.clip_rect().intersect(clip));
    for row in &layout.galley.rows {
        for glyph in &row.glyphs {
            if glyph.chr == SLOT.chars().next().unwrap() {
                let Some(key) = layout.images.get(&glyph.section_index) else {
                    continue;
                };
                let cell = glyph.logical_rect().translate(origin);
                let size = Vec2::splat(glyph.advance_width.min(glyph.line_height));
                let target = Rect::from_center_size(cell.center(), size);
                // Long messages must not decode/enqueue off-screen rows.
                if !ui.is_rect_visible(target) {
                    continue;
                }
                match image(ui.ctx(), key) {
                    Some(Cached::Ready(texture)) => {
                        let actual = Vec2::new(
                            size.x,
                            size.x * texture.size()[1] as f32 / texture.size()[0] as f32,
                        );
                        painter.image(
                            texture.id(),
                            Rect::from_center_size(target.center(), actual),
                            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                            Color32::WHITE,
                        );
                    }
                    Some(Cached::Failed) | None => {
                        let fallback = match key {
                            ImageKey::Native(cluster) => Some(cluster.as_str()),
                            ImageKey::Original(sprite) => catalog()
                                .canonical
                                .get(sprite)
                                .map(String::as_str)
                                .or_else(|| {
                                    layout
                                        .raw_images
                                        .get(&glyph.section_index)
                                        .map(String::as_str)
                                }),
                        };
                        if let Some(cluster) = fallback {
                            painter.with_clip_rect(target).text(
                                target.center(),
                                egui::Align2::CENTER_CENTER,
                                cluster,
                                FontId::proportional(size.y * 0.8),
                                color,
                            );
                        }
                    }
                }
            }
        }
    }
}

// No Label allocation or interaction: the parent conversation row owns clicks.
// Parse the full preview before pixel elision so aliases and graphemes stay intact.
pub(super) fn paint_preview(ui: &egui::Ui, text: &str, rect: Rect, size: f32, color: Color32) {
    if rect.width() <= 0.0 || !ui.is_rect_visible(rect) {
        return;
    }
    let (layout, ellipsis) = preview_layout(ui, text, size, color, rect.width());
    let origin = Pos2::new(rect.left(), rect.center().y - layout.galley.size().y * 0.5);
    ui.painter()
        .with_clip_rect(ui.clip_rect().intersect(rect))
        .galley(origin, layout.galley.clone(), color);
    paint_images(ui, &layout, origin, rect, color);
    if let Some(ellipsis) = ellipsis {
        let position = Pos2::new(
            origin.x + layout.galley.size().x,
            rect.center().y - ellipsis.size().y * 0.5,
        );
        ui.painter()
            .with_clip_rect(ui.clip_rect().intersect(rect))
            .galley(position, ellipsis, color);
    }
}

fn preview_layout(
    ui: &egui::Ui,
    text: &str,
    size: f32,
    color: Color32,
    width: f32,
) -> (InlineLayout, Option<Arc<egui::Galley>>) {
    let layout = layout_with_mode(ui, text, size, color, width, true);
    if !layout.galley.elided && layout.galley.size().x <= width {
        return (layout, None);
    }
    let ellipsis =
        ui.fonts(|fonts| fonts.layout_no_wrap("…".into(), FontId::proportional(size), color));
    let mut layout = layout_with_mode(
        ui,
        text,
        size,
        color,
        (width - ellipsis.size().x).max(1.0),
        true,
    );
    // Epaint retains an over-wide first glyph. Keep an atomic emoji hidden
    // rather than painting half of it when only the ellipsis fits.
    if layout.galley.size().x + ellipsis.size().x > width {
        layout = layout_with_mode(ui, "", size, color, width, true);
    }
    (layout, Some(ellipsis))
}

pub(super) fn truncate_graphemes(text: &str, max: usize) -> std::borrow::Cow<'_, str> {
    if let Some((end, _)) = text.grapheme_indices(true).nth(max) {
        std::borrow::Cow::Owned(format!("{}…", &text[..end]))
    } else {
        std::borrow::Cow::Borrowed(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synthetic_png() -> Vec<u8> {
        let pixels = ::image::RgbaImage::from_pixel(64, 64, ::image::Rgba([240, 100, 20, 255]));
        let mut bytes = std::io::Cursor::new(Vec::new());
        pixels
            .write_to(&mut bytes, ::image::ImageFormat::Png)
            .unwrap();
        bytes.into_inner()
    }

    #[test]
    fn pack_validation_rejects_missing_corrupt_and_out_of_bounds_bytes() {
        let bytes = synthetic_png();
        let mut sprite = Sprite {
            offset: 0,
            length: bytes.len(),
            width: 64,
            height: 64,
            sha256: format!("{:x}", Sha256::digest(&bytes)),
        };
        assert!(valid_sprite(&bytes, &sprite));
        assert_eq!(decode_image(&bytes).unwrap().size, [64, 64]);
        assert!(!valid_sprite(&[], &sprite));
        let mut corrupt = bytes.clone();
        corrupt[20] ^= 1;
        assert!(!valid_sprite(&corrupt, &sprite));
        sprite.offset = usize::MAX;
        assert!(!valid_sprite(&bytes, &sprite));
        assert!(decode_image(b"not a PNG").is_none());
    }

    #[test]
    fn aliases_without_canonical_unicode_remain_raw_when_pack_is_missing() {
        let catalog = catalog();
        assert_eq!(catalog.canonical.len(), 1295);
        let noncanonical: HashSet<_> = catalog
            .index
            .aliases
            .values()
            .filter(|id| !catalog.canonical.contains_key(id))
            .collect();
        assert_eq!(noncanonical.len(), 88);
        for (alias, &sprite) in &catalog.index.aliases {
            if catalog.validated[sprite as usize] {
                continue;
            }
            let parsed = parts(alias);
            assert_eq!(
                parsed.iter().map(|part| part.raw).collect::<String>(),
                *alias
            );
            if let Some(unicode) = catalog.canonical.get(&sprite) {
                assert_eq!(parsed[0].image, Some(ImageKey::Native(unicode.clone())));
            } else {
                assert!(
                    parsed[0].image.is_none(),
                    "Keep unknown alias literal: {alias}"
                );
            }
        }
    }

    #[test]
    fn sidebar_preview_paints_unicode_when_image_rendering_fails() {
        let ctx = egui::Context::default();
        setup_custom_fonts(&ctx);
        let key = unicode_image("🎃").unwrap();
        cache(&ctx).lock().unwrap().insert(key, Cached::Failed);
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                paint_preview(
                    ui,
                    "🎃 HotSale Halloween chỉ từ 79.000Đ",
                    Rect::from_min_size(Pos2::new(10.0, 10.0), Vec2::new(220.0, 20.0)),
                    12.0,
                    Color32::WHITE,
                );
            });
        });
        assert!(
            output.shapes.iter().any(|shape| matches!(&shape.shape,
            egui::Shape::Text(text) if text.galley.job.text == "🎃"
                && text.fallback_color != Color32::TRANSPARENT)),
            "Failed artwork/native helper must paint the Unicode fallback, not a blank cell"
        );
    }

    #[test]
    fn sidebar_preview_elides_without_splitting_emoji_or_allocating_widgets() {
        let ctx = egui::Context::default();
        setup_custom_fonts(&ctx);
        for text in [
            "🎃",
            "🎃 HotSale Halloween chỉ từ 79.000Đ",
            ":)) :D 👩‍💻 👍🏽 🇻🇳 kết thúc",
            "🎃🎃🎃🎃🎃🎃🎃🎃🎃🎃",
            "Dòng một\nDòng hai",
            "",
        ] {
            for width in [8.0, 24.0, 64.0, 140.0, 240.0] {
                let _ = ctx.run(egui::RawInput::default(), |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        let (layout, ellipsis) =
                            preview_layout(ui, text, 12.0, Color32::WHITE, width);
                        assert!(layout.galley.rows.len() <= 1);
                        assert!(layout.galley.size().x <= width, "Never clip half an emoji");
                        if width >= 24.0 {
                            assert!(
                                layout.galley.size().x
                                    + ellipsis.as_ref().map_or(0.0, |g| g.size().x)
                                    <= width + 1.0,
                                "text={text:?} width={width} galley={} ellipsis={:?}",
                                layout.galley.size().x,
                                ellipsis.as_ref().map(|g| g.size())
                            );
                        }
                        for row in &layout.galley.rows {
                            for glyph in &row.glyphs {
                                if layout.images.contains_key(&glyph.section_index) {
                                    assert_eq!(glyph.chr.to_string(), SLOT);
                                    assert!((glyph.advance_width - 17.0).abs() < 0.1);
                                }
                            }
                        }
                        if let Some(ellipsis) = ellipsis {
                            assert_eq!(ellipsis.job.text, "…");
                            assert_eq!(ellipsis.job.sections[0].format.color, Color32::WHITE);
                            assert_eq!(
                                ellipsis.job.sections[0].format.font_id,
                                FontId::proportional(12.0)
                            );
                        }
                        let before = ui.next_widget_position();
                        paint_preview(
                            ui,
                            text,
                            Rect::from_min_size(Pos2::new(10.0, 10.0), Vec2::new(width, 20.0)),
                            12.0,
                            Color32::WHITE,
                        );
                        assert_eq!(ui.next_widget_position(), before);
                    });
                });
            }
        }
    }

    #[test]
    fn original_alias_tokenizer_matches_all_recovered_cases() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/zalo-emoji-tokenizer.json"
        ))
        .unwrap();
        for case in fixture["cases"].as_array().unwrap() {
            let text = case["input"].as_str().unwrap();
            let actual: Vec<_> = aliases(text)
                .into_iter()
                .map(|(a, b, _)| {
                    serde_json::json!({"start_utf16":text[..a].encode_utf16().count(),
                    "end_utf16":text[..b].encode_utf16().count(),"raw":&text[a..b]})
                })
                .collect();
            let expected: Vec<_> = case["segments"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|v| v["kind"] == "emoji")
                .map(|v| {
                    serde_json::json!({
                    "start_utf16":v["start_utf16"],"end_utf16":v["end_utf16"],"raw":v["raw"]})
                })
                .collect();
            assert_eq!(actual, expected, "case {}", case["id"]);
        }
    }
    #[test]
    #[ignore = "Requires explicit ZL_EMOJI_PACK with all 1383 original PNGs"]
    fn all_assets_are_exact_original_pngs() {
        let metadata: serde_json::Value =
            serde_json::from_str(include_str!("../assets/emojis/zalo/index.json")).unwrap();
        assert_eq!(catalog().index.sprites.len(), 1383);
        assert_eq!(catalog().index.aliases.len(), 1440);
        assert_eq!(catalog().index.unicode.len(), 1295);
        for (sprite, raw) in catalog()
            .index
            .sprites
            .iter()
            .zip(metadata["sprites"].as_array().unwrap())
        {
            let bytes = &PACK[sprite.offset..sprite.offset + sprite.length];
            assert_eq!(
                format!("{:x}", Sha256::digest(bytes)),
                raw["sha256"].as_str().unwrap()
            );
            let decoded = decode_image(bytes).unwrap();
            assert_eq!(decoded.size, [sprite.width, sprite.height]);
        }
    }
    #[test]
    fn extended_clusters_are_never_split_or_downgraded_to_prefix() {
        let input = "Tiếng Việt :) 😀 ❤️ 👍🏽 👩‍💻 🇻🇳 1️⃣ 👨‍👩‍👧‍👦 🫶\n❤︎";
        let parsed = parts(input);
        assert_eq!(parsed.iter().map(|p| p.raw).collect::<String>(), input);
        for cluster in ["👍🏽", "👩‍💻", "🫶"] {
            let p = parsed.iter().find(|p| p.raw == cluster).unwrap();
            assert_eq!(p.image, Some(ImageKey::Native(cluster.into())));
        }
        for known in ["❤️", "1️⃣"] {
            assert_eq!(
                parsed.iter().find(|p| p.raw == known).unwrap().image,
                unicode_image(known)
            );
        }
        assert!(parsed
            .iter()
            .find(|p| p.raw == "❤︎")
            .unwrap()
            .image
            .is_none());
        assert_eq!(truncate_graphemes("a👩‍💻b", 2), "a👩‍💻…");
        assert!(parts("a️ 🄰 𝔸 123 # *").iter().all(|p| p.image.is_none()));
    }
    #[test]
    fn original_aliases_keep_distinct_artwork() {
        let smile = aliases(":)")[0].2;
        assert_ne!(smile, aliases(":))")[0].2);
        assert_ne!(aliases("/-heart")[0].2, aliases(":heart:")[0].2);
        assert_eq!(aliases(":D")[0].2, aliases(":d")[0].2);
    }
    #[test]
    fn inline_slots_wrap_and_stay_within_narrow_widths() {
        let ctx = egui::Context::default();
        setup_custom_fonts(&ctx);
        for width in [32.0, 64.0, 90.0, 140.0, 320.0] {
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let result = layout(
                        ui,
                        "Xin chào :):D 😀 ❤️\n🇻🇳 👨‍👩‍👧‍👦",
                        14.0,
                        Color32::WHITE,
                        width,
                    );
                    let mut cells = 0;
                    for row in &result.galley.rows {
                        assert!(
                            row.rect.width() <= width + 1.0,
                            "width {width}, row {:?}",
                            row.rect
                        );
                        for glyph in &row.glyphs {
                            if result.images.contains_key(&glyph.section_index) {
                                cells += 1;
                                assert!((glyph.advance_width - 20.0).abs() < 0.1, "{:?}", glyph);
                                assert!(glyph.line_height >= 20.0);
                            }
                        }
                    }
                    assert_eq!(
                        cells,
                        parts("Xin chào :):D 😀 ❤️\n🇻🇳 👨‍👩‍👧‍👦")
                            .iter()
                            .filter(|p| p.image.is_some())
                            .count()
                    );
                });
            });
        }
    }
    #[test]
    fn cache_is_bounded_and_original_artwork_does_not_thrash() {
        let ctx = egui::Context::default();
        let bytes = synthetic_png();
        let sprite = Sprite {
            offset: 0,
            length: bytes.len(),
            width: 64,
            height: 64,
            sha256: format!("{:x}", Sha256::digest(&bytes)),
        };
        for frame in 0..40 {
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                let shared = cache(ctx);
                let mut c = shared.lock().unwrap();
                for id in 0..257 {
                    let key = ImageKey::Original(id);
                    if c.get(&key).is_none() {
                        let _ = load_original(ctx, &mut c, &key, &sprite, &bytes);
                    }
                }
                assert!(c.decoded_this_pass <= DECODE_PER_PASS, "frame {frame}");
            });
        }
        let shared = cache(&ctx);
        let mut c = shared.lock().unwrap();
        assert_eq!(c.total_decodes, 257);
        let first = match c.get(&ImageKey::Original(0)).unwrap() {
            Cached::Ready(t) => t.id(),
            _ => panic!(),
        };
        let second = match c.get(&ImageKey::Original(0)).unwrap() {
            Cached::Ready(t) => t.id(),
            _ => panic!(),
        };
        assert_eq!(first, second);
        for id in 0..1000 {
            c.insert(ImageKey::Native(format!("fixture-{id}")), Cached::Failed);
        }
        assert_eq!(c.order.len(), CACHE_LIMIT);
        assert_eq!(c.entries.len(), 257 + CACHE_LIMIT);
        assert!(c.entries.contains_key(&ImageKey::Original(0)));
    }

    #[test]
    fn raw_selection_domain_preserves_aliases_and_extended_clusters() {
        let ctx = egui::Context::default();
        setup_custom_fonts(&ctx);
        let text = ":)) 👍🏽 👩‍💻\nXin chào ❤️";
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let layout = layout(ui, text, 14.0, Color32::WHITE, 110.0);
                let raw = selection_galley(&layout);
                assert_eq!(raw.job.text, text);
                assert_eq!(
                    raw.rows
                        .iter()
                        .map(|r| r.glyphs.len() + usize::from(r.ends_with_newline))
                        .sum::<usize>(),
                    text.chars().count()
                );
                for row in &raw.rows {
                    for (index, glyph) in row.glyphs.iter().enumerate() {
                        if glyph.advance_width > 0.0
                            && layout.raw_images.contains_key(&glyph.section_index)
                        {
                            let start = raw
                                .rows
                                .iter()
                                .take_while(|r| !std::ptr::eq(*r, row))
                                .map(|r| r.char_count_including_newline())
                                .sum::<usize>()
                                + index;
                            let count = layout.raw_images[&glyph.section_index].chars().count();
                            let low = Pos2::new(
                                glyph.pos.x + glyph.advance_width * 0.25,
                                row.rect.center().y,
                            );
                            let high = Pos2::new(
                                glyph.pos.x + glyph.advance_width * 0.75,
                                row.rect.center().y,
                            );
                            assert_eq!(raw.cursor_from_pos(low.to_vec2()).ccursor.index, start);
                            assert_eq!(
                                raw.cursor_from_pos(high.to_vec2()).ccursor.index,
                                start + count
                            );
                        }
                    }
                }
            });
        });
    }
    #[test]
    fn pointer_selection_copy_event_returns_raw_emoji_text() {
        let text = "Prefix :)) 👍🏽 👩‍💻\nXin chào ❤️ suffix";
        let total = text.chars().count();
        let middle_start = "Prefix ".chars().count();
        let middle_end = "Prefix :)) 👍🏽 👩‍💻".chars().count();
        for (start_index, end_index) in [(0, total), (middle_start, middle_end)] {
            let ctx = egui::Context::default();
            setup_custom_fonts(&ctx);
            let mut start = Pos2::ZERO;
            let mut end = Pos2::ZERO;
            for frame in 0..5 {
                let event = |pos, pressed| egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: Default::default(),
                };
                let events = match frame {
                    1 => vec![egui::Event::PointerMoved(start), event(start, true)],
                    2 => vec![egui::Event::PointerMoved(end)],
                    3 => vec![event(end, false)],
                    4 => vec![egui::Event::Copy],
                    _ => vec![],
                };
                let output = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(640.0, 480.0))),
                        time: Some(frame as f64),
                        events,
                        ..Default::default()
                    },
                    |ctx| {
                        egui::CentralPanel::default().show(ctx, |ui| {
                            let raw =
                                selection_galley(&layout(ui, text, 14.0, Color32::WHITE, 130.0));
                            let response = show(ui, text, 14.0, Color32::WHITE, 130.0);
                            start = raw
                                .pos_from_ccursor(egui::text::CCursor::new(start_index))
                                .center()
                                + response.rect.min.to_vec2();
                            end = raw
                                .pos_from_ccursor(egui::text::CCursor::new(end_index))
                                .center()
                                + response.rect.min.to_vec2();
                        });
                    },
                );
                if frame == 4 {
                    let expected: String = text
                        .chars()
                        .skip(start_index)
                        .take(end_index - start_index)
                        .collect();
                    assert_eq!(output.platform_output.copied_text, expected);
                }
            }
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "Requires the built native EmojiPreview helper"]
    fn native_color_fallback_preserves_skin_and_zwj() {
        for cluster in ["👍🏽", "👩‍💻", "🫶", "1️⃣"] {
            let bytes = native_preview(cluster).unwrap();
            let image = decode_image(&bytes).unwrap();
            assert_eq!(image.size, [64, 64]);
            assert!(image
                .pixels
                .iter()
                .any(|p| p.a() > 0 && p.r().abs_diff(p.g()) > 20));
        }
    }
}

#[cfg(test)]
mod mention_interaction_tests {
    use super::*;
    use crate::message_navigation::MentionSpan;

    #[test]
    fn mention_hit_rectangles_follow_raw_emoji_newlines_and_wrapping() {
        let ctx = egui::Context::default();
        setup_custom_fonts(&ctx);
        let text = "😀 :))\n@Tên dài với dấu và tên thứ hai";
        let start = text.find('@').unwrap();
        let spans = vec![MentionSpan {
            user_id: "123".into(),
            byte_range: start..text.len(),
        }];
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let layout = layout(ui, text, 14.0, Color32::WHITE, 90.0);
                let galley = selection_galley(&layout);
                assert_eq!(galley.job.text, text);
                let rectangles = mention_rectangles(&galley, &spans);
                assert!(
                    rectangles.len() >= 2,
                    "Long mentions must retain multiple hit rows"
                );
                for (index, row, rect) in rectangles {
                    assert_eq!(index, 0);
                    assert!(
                        row >= 1,
                        "Emoji on the first line must not become a mention"
                    );
                    assert!(rect.width() > 0.0);
                }
            });
        });
    }

    #[test]
    fn semantic_pointer_events_click_exact_uid_after_emoji_without_changing_layout() {
        let ctx = egui::Context::default();
        setup_custom_fonts(&ctx);
        let text = "😀 :)) @An @An";
        let spans = vec![
            MentionSpan {
                user_id: "11".into(),
                byte_range: 9..12,
            },
            MentionSpan {
                user_id: "22".into(),
                byte_range: 13..16,
            },
        ];
        let mut target = Pos2::ZERO;
        let mut clicked = None;
        for frame in 0..3 {
            let events = if frame == 0 {
                vec![]
            } else {
                vec![
                    egui::Event::PointerMoved(target),
                    egui::Event::PointerButton {
                        pos: target,
                        button: egui::PointerButton::Primary,
                        pressed: frame == 1,
                        modifiers: Default::default(),
                    },
                ]
            };
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(400.0, 300.0))),
                    events,
                    time: Some(frame as f64 * 0.016),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        let layout = layout(ui, text, 14.0, Color32::WHITE, 300.0);
                        let galley = selection_galley(&layout);
                        let original_size = measure(ui, text, 14.0, 300.0);
                        let rect = mention_rectangles(&galley, &spans)
                            .into_iter()
                            .find(|(index, _, _)| *index == 1)
                            .unwrap()
                            .2;
                        target = rect.center() + ui.next_widget_position().to_vec2();
                        clicked = show_with_mentions(ui, text, 14.0, Color32::WHITE, 300.0, &spans)
                            .or(clicked);
                        assert_eq!(galley.size(), original_size);
                    });
                },
            );
        }
        assert_eq!(
            clicked.map(|index| spans[index].user_id.as_str()),
            Some("22")
        );
    }
}

#[cfg(test)]
#[path = "emoji_text/visual_tests.rs"]
mod visual_tests;
