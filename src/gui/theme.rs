use super::*;

// ---------------------------------------------------------------------------
// Semantic Design System Tokens (UI/UX Pro Max — Minimalism & Swiss Style)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
pub(super) struct ThemePalette {
    pub(super) is_dark: bool,
    pub(super) bg_app: Color32,
    pub(super) bg_sidebar: Color32,
    pub(super) bg_chat_canvas: Color32,
    pub(super) bg_elevated: Color32,
    pub(super) bg_hover: Color32,
    pub(super) bg_selected: Color32,
    pub(super) bg_input: Color32,
    pub(super) border: Color32,
    pub(super) text_primary: Color32,
    pub(super) text_secondary: Color32,
    pub(super) text_muted: Color32,
    pub(super) primary: Color32,
    pub(super) primary_hover: Color32,
    pub(super) accent_online: Color32,
    pub(super) danger: Color32,
    pub(super) warning: Color32,
    pub(super) bubble_out_bg: Color32,
    pub(super) bubble_out_text: Color32,
    pub(super) bubble_out_meta: Color32,
    pub(super) bubble_in_bg: Color32,
    pub(super) bubble_in_border: Color32,
    pub(super) bubble_in_text: Color32,
    pub(super) bubble_in_meta: Color32,
}

impl ThemePalette {
    pub(super) fn dark() -> Self {
        // Official Atom One Dark (`one-dark-ui` + `one-dark-syntax`) color system
        Self {
            is_dark: true,
            bg_app: Color32::from_rgb(24, 26, 31), // #181A1F Activity Bar & Title Bar
            bg_sidebar: Color32::from_rgb(33, 37, 43), // #21252B Tree-View / Sidebar / Header
            bg_chat_canvas: Color32::from_rgb(40, 44, 52), // #282C34 Atom One Dark Editor Canvas
            bg_elevated: Color32::from_rgb(44, 49, 58), // #2C313A Elevated Card / Modal / Popover
            bg_hover: Color32::from_rgb(50, 56, 66), // #323842 List Hover Surface
            bg_selected: Color32::from_rgb(58, 63, 75), // #3A3F4B Active Selection Row
            bg_input: Color32::from_rgb(29, 31, 35), // #1D1F23 Input / Search Well
            border: Color32::from_rgb(52, 57, 66), // #343942 Subtle Panel Divider
            text_primary: Color32::from_rgb(215, 218, 224), // #D7DAE0 Bright Foreground
            text_secondary: Color32::from_rgb(171, 178, 191), // #ABB2BF Atom @mono-1
            text_muted: Color32::from_rgb(118, 126, 142), // #767E8E Atom @mono-3 (readable muted)
            primary: Color32::from_rgb(82, 139, 255), // #528BFF Atom UI Accent Blue
            primary_hover: Color32::from_rgb(97, 175, 239), // #61AFEF Atom @hue-2 Cyan-Blue
            accent_online: Color32::from_rgb(152, 195, 121), // #98C379 Atom @hue-4 Green
            danger: Color32::from_rgb(224, 108, 117), // #E06C75 Atom @hue-5 Coral Red
            warning: Color32::from_rgb(229, 192, 123), // #E5C07B Atom @hue-62 Warm Gold
            bubble_out_bg: Color32::from_rgb(37, 55, 77), // #25374D Zalo Dark Slate-Navy Outgoing Bubble
            bubble_out_text: Color32::from_rgb(235, 240, 246), // #EBF0F6
            bubble_out_meta: Color32::from_rgb(154, 171, 192), // #9AABC0
            bubble_in_bg: Color32::from_rgb(50, 56, 66),  // #323842 Atom Elevated Incoming Bubble
            bubble_in_border: Color32::from_rgb(62, 68, 81), // #3E4451 Atom Gutter Border
            bubble_in_text: Color32::from_rgb(215, 218, 224), // #D7DAE0
            bubble_in_meta: Color32::from_rgb(142, 150, 165), // #8E96A5
        }
    }

    pub(super) fn light() -> Self {
        Self {
            is_dark: false,
            bg_app: Color32::from_rgb(255, 255, 255), // #FFFFFF
            bg_sidebar: Color32::from_rgb(255, 255, 255), // #FFFFFF
            bg_chat_canvas: Color32::from_rgb(235, 240, 247), // Authentic Zalo PC cool blue-gray canvas
            bg_elevated: Color32::from_rgb(255, 255, 255),    // #FFFFFF
            bg_hover: Color32::from_rgb(241, 245, 249),       // #F1F5F9
            bg_selected: Color32::from_rgb(229, 241, 255),    // #E5F1FF Zalo active row
            bg_input: Color32::from_rgb(248, 250, 252),       // #F8FAFC
            border: Color32::from_rgb(226, 232, 240),         // #E2E8F0
            text_primary: Color32::from_rgb(15, 23, 42),      // #0F172A (16.1:1 contrast)
            text_secondary: Color32::from_rgb(71, 85, 105),   // #475569 (7.2:1 contrast)
            text_muted: Color32::from_rgb(100, 116, 139),     // #64748B (4.6:1 contrast)
            primary: Color32::from_rgb(0, 104, 255),          // #0068FF Zalo Blue
            primary_hover: Color32::from_rgb(37, 99, 235),    // #2563EB
            accent_online: Color32::from_rgb(5, 150, 105),    // #059669 Emerald
            danger: Color32::from_rgb(220, 38, 38),           // #DC2626
            warning: Color32::from_rgb(217, 119, 6),          // #D97706
            bubble_out_bg: Color32::from_rgb(229, 239, 255), // Zalo Desktop Light Outgoing (#E5EFFF)
            bubble_out_text: Color32::from_rgb(15, 23, 42),  // Crisp dark text on light blue (14:1)
            bubble_out_meta: Color32::from_rgb(71, 92, 122),
            bubble_in_bg: Color32::WHITE,
            bubble_in_border: Color32::from_rgb(216, 224, 236),
            bubble_in_text: Color32::from_rgb(15, 23, 42),
            bubble_in_meta: Color32::from_rgb(100, 116, 139),
        }
    }

    pub(super) fn apply_to_ctx(&self, ctx: &egui::Context) {
        let mut style = (*ctx.style()).clone();
        let mut visuals = if self.is_dark {
            egui::Visuals::dark()
        } else {
            egui::Visuals::light()
        };

        visuals.panel_fill = self.bg_app;
        visuals.window_fill = self.bg_elevated;
        visuals.extreme_bg_color = self.bg_input;
        visuals.faint_bg_color = self.bg_hover;
        visuals.override_text_color = Some(self.text_primary);
        visuals.selection.bg_fill =
            self.primary
                .linear_multiply(if self.is_dark { 0.45 } else { 0.22 });
        visuals.selection.stroke = Stroke::new(1.5_f32, self.primary);

        let r = Rounding::same(8.0);
        visuals.widgets.noninteractive.bg_fill = self.bg_app;
        visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, self.border);
        visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, self.text_primary);
        visuals.widgets.noninteractive.rounding = r;

        visuals.widgets.inactive.bg_fill = self.bg_elevated;
        visuals.widgets.inactive.weak_bg_fill = self.bg_elevated;
        visuals.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, self.border);
        visuals.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, self.text_primary);
        visuals.widgets.inactive.rounding = r;

        visuals.widgets.hovered.bg_fill = self.bg_hover;
        visuals.widgets.hovered.weak_bg_fill = self.bg_hover;
        visuals.widgets.hovered.bg_stroke = Stroke::new(1.2_f32, self.primary);
        visuals.widgets.hovered.fg_stroke = Stroke::new(1.2_f32, self.text_primary);
        visuals.widgets.hovered.rounding = r;

        visuals.widgets.active.bg_fill = self.primary;
        visuals.widgets.active.weak_bg_fill = self.primary;
        visuals.widgets.active.bg_stroke = Stroke::new(1.5_f32, self.primary);
        visuals.widgets.active.fg_stroke = Stroke::new(1.5_f32, Color32::WHITE);
        visuals.widgets.active.rounding = r;

        style.spacing.item_spacing = Vec2::new(8.0, 6.0);
        style.spacing.button_padding = Vec2::new(10.0, 6.0);
        style.spacing.scroll = egui::style::ScrollStyle::solid();
        style.spacing.scroll.bar_width = 6.0;
        style.visuals = visuals;
        ctx.set_style(style);
    }
}

// ---------------------------------------------------------------------------
// Zero-allocation Vector Icon Painter Primitives (Replaces Emoji Icons)
// ---------------------------------------------------------------------------

pub(super) fn avatar_color_for(name: &str, is_group: bool) -> Color32 {
    const PALETTE: &[Color32] = &[
        Color32::from_rgb(64, 120, 192), // Atom Blue (#4078C0)
        Color32::from_rgb(56, 148, 160), // Atom Cyan (#3894A0)
        Color32::from_rgb(152, 88, 178), // Atom Purple (#9858B2)
        Color32::from_rgb(190, 80, 70),  // Atom Red (#BE5046)
        Color32::from_rgb(184, 118, 62), // Atom Orange (#B8763E)
        Color32::from_rgb(98, 148, 72),  // Atom Green (#629448)
        Color32::from_rgb(82, 116, 196), // Atom Indigo
        Color32::from_rgb(72, 138, 182), // Atom Cerulean
    ];
    let mut hash: usize = if is_group { 3 } else { 0 };
    for b in name.bytes() {
        hash = hash.wrapping_mul(31).wrapping_add(b as usize);
    }
    PALETTE[hash % PALETTE.len()]
}

pub(super) fn truncate_chars<'a>(s: &'a str, max_chars: usize) -> std::borrow::Cow<'a, str> {
    if let Some((byte_idx, _)) = s.char_indices().nth(max_chars) {
        std::borrow::Cow::Owned(format!("{}…", &s[..byte_idx]))
    } else {
        std::borrow::Cow::Borrowed(s)
    }
}

/// Extract up to 2 uppercase initials from a display name (e.g. `"Nguyễn An"` -> `"NA"`, `"Hoang"` -> `"H"`).
pub(super) fn avatar_initials(name: &str) -> String {
    let words: Vec<&str> = name.split_whitespace().filter(|w| !w.is_empty()).collect();
    if words.len() >= 2 {
        let c1 = words[0].chars().next().unwrap_or('Z').to_uppercase();
        let c2 = words[words.len() - 1]
            .chars()
            .next()
            .unwrap_or('U')
            .to_uppercase();
        format!("{c1}{c2}")
    } else {
        name.chars()
            .next()
            .unwrap_or('Z')
            .to_uppercase()
            .to_string()
    }
}

/// Fold Vietnamese text to lowercase ASCII for diacritic-insensitive search
/// (e.g. `"Nguyễn Văn An"` -> `"nguyen van an"`).
pub(super) fn fold_vn_search(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for raw in s.chars() {
        for c in raw.to_lowercase() {
            let mapped = match c {
                'à' | 'á' | 'ạ' | 'ả' | 'ã' | 'â' | 'ầ' | 'ấ' | 'ậ' | 'ẩ' | 'ẫ' | 'ă' | 'ằ'
                | 'ắ' | 'ặ' | 'ẳ' | 'ẵ' => 'a',
                'è' | 'é' | 'ẹ' | 'ẻ' | 'ẽ' | 'ê' | 'ề' | 'ế' | 'ệ' | 'ể' | 'ễ' => {
                    'e'
                }
                'ì' | 'í' | 'ị' | 'ỉ' | 'ĩ' => 'i',
                'ò' | 'ó' | 'ọ' | 'ỏ' | 'õ' | 'ô' | 'ồ' | 'ố' | 'ộ' | 'ổ' | 'ỗ' | 'ơ' | 'ờ'
                | 'ớ' | 'ợ' | 'ở' | 'ỡ' => 'o',
                'ù' | 'ú' | 'ụ' | 'ủ' | 'ũ' | 'ư' | 'ừ' | 'ứ' | 'ự' | 'ử' | 'ữ' => {
                    'u'
                }
                'ỳ' | 'ý' | 'ỵ' | 'ỷ' | 'ỹ' => 'y',
                'đ' => 'd',
                other => other,
            };
            out.push(mapped);
        }
    }
    out
}

/// Check whether `haystack` matches `query_lower` (or `query_folded` without Vietnamese accents).
pub(super) fn matches_vn_query(haystack: &str, query_lower: &str, query_folded: &str) -> bool {
    if query_lower.is_empty() {
        return true;
    }
    if haystack.to_lowercase().contains(query_lower) {
        return true;
    }
    !query_folded.is_empty() && fold_vn_search(haystack).contains(query_folded)
}

/// Paint a circular user or group avatar: uses the real circular-masked Zalo profile image when
/// `avatar_url` is available (fetching it asynchronously via `FetchStickerImage`), and falls back
/// to a colored circle with initials while loading or when no URL exists.
// The painter boundary keeps geometry and avatar metadata explicit.
#[allow(clippy::too_many_arguments)]
pub(super) fn paint_user_avatar(
    painter: &egui::Painter,
    center: Pos2,
    radius: f32,
    name: &str,
    is_group: bool,
    avatar_url: Option<&str>,
    textures: &HashMap<String, TextureHandle>,
    loading: &mut HashSet<String>,
) {
    if let Some(url) = avatar_url.map(str::trim).filter(|u| {
        !u.is_empty()
            && !u.ends_with("/default")
            && (u.starts_with("http://") || u.starts_with("https://"))
    }) {
        let file_or_base = crate::chat::avatar_file_key(Some(url))
            .unwrap_or_else(|| url.split('?').next().unwrap_or(url).to_string());
        let key = format!("ava_{}", crate::crypto::md5_hex(&file_or_base));
        if let Some(tex) = textures.get(&key) {
            let rect = Rect::from_center_size(center, Vec2::splat(radius * 2.0));
            painter.image(
                tex.id(),
                rect,
                Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                Color32::WHITE,
            );
            painter.circle_stroke(
                center,
                radius,
                Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(255, 255, 255, 34)),
            );
            return;
        }
        if loading.insert(key.clone()) {
            if let Some(tx) = WORKER_TX.get() {
                let _ = tx.send(WorkerCmd::FetchStickerImage {
                    key,
                    url: url.to_string(),
                });
            }
        }
    }

    let av_col = avatar_color_for(name, is_group);
    painter.circle_filled(center, radius, av_col);
    let initial = if radius >= 18.0 {
        avatar_initials(name)
    } else {
        name.chars()
            .next()
            .unwrap_or('Z')
            .to_uppercase()
            .to_string()
    };
    painter.text(
        center,
        egui::Align2::CENTER_CENTER,
        initial,
        FontId::proportional((radius * 0.72).max(11.0)),
        Color32::WHITE,
    );
}

pub mod lucide {
    pub const SEARCH: &str = "\u{e151}";
    pub const REFRESH: &str = "\u{e145}";
    pub const SMILE: &str = "\u{e164}";
    pub const SEND: &str = "\u{e4f2}";
    pub const SUN: &str = "\u{e178}";
    pub const MOON: &str = "\u{e11e}";
    pub const PHONE: &str = "\u{e133}";
    pub const PHONE_CALL: &str = "\u{e134}";
    pub const PHONE_OFF: &str = "\u{e138}";
    pub const VIDEO: &str = "\u{e1a5}";
    pub const VIDEO_OFF: &str = "\u{e1a6}";
    pub const CAMERA: &str = "\u{e062}";
    pub const IMAGE: &str = "\u{e0f6}";
    pub const PAPERCLIP: &str = "\u{e12d}";
    pub const CHECK: &str = "\u{e06c}";
    pub const CHECK_CHECK: &str = "\u{e38e}";
    pub const MESSAGE_SQUARE: &str = "\u{e117}";
    pub const USERS: &str = "\u{e1a4}";
    pub const USER_PLUS: &str = "\u{e1a2}";
    pub const CLOUD: &str = "\u{e088}";
    pub const SETTINGS: &str = "\u{e154}";
    pub const PANEL_RIGHT: &str = "\u{e431}";
    pub const PIN: &str = "\u{e259}";
    pub const COPY: &str = "\u{e09e}";
    pub const STAR: &str = "\u{e176}";
    pub const GLOBE: &str = "\u{e0e8}";
    pub const LINK: &str = "\u{e102}";
    pub const EXTERNAL_LINK: &str = "\u{e0b9}";
    pub const X: &str = "\u{e1b2}";
    pub const PLUS: &str = "\u{e13d}";
    pub const MINUS: &str = "\u{e11c}";
    pub const STORE: &str = "\u{e3e4}";
    pub const FLAME: &str = "\u{e0d2}";
    pub const SPARKLES: &str = "\u{e412}";
    pub const BAN: &str = "\u{e051}";
    pub const MIC: &str = "\u{e118}";
    pub const MIC_OFF: &str = "\u{e119}";
    pub const LOG_OUT: &str = "\u{e10e}";
    pub const FILE_TEXT: &str = "\u{e0cc}";
    pub const FILM: &str = "\u{e0d0}";
    pub const SHIELD_CHECK: &str = "\u{e15c}";
    pub const LOCK: &str = "\u{e10b}";
    pub const BELL: &str = "\u{e059}";
    pub const PAINTBRUSH: &str = "\u{e2e7}";
    pub const SLIDERS: &str = "\u{e29a}";
    pub const FOLDER_OPEN: &str = "\u{e247}";
    pub const CHEVRON_RIGHT: &str = "\u{e06f}";
    pub const CHEVRON_DOWN: &str = "\u{e06d}";
    pub const USER: &str = "\u{e19f}";
}

pub(super) fn draw_lucide_icon(
    painter: &egui::Painter,
    center: Pos2,
    icon: &str,
    size: f32,
    color: Color32,
) {
    painter.text(
        center,
        egui::Align2::CENTER_CENTER,
        icon,
        FontId::proportional(size),
        color,
    );
}

pub(super) fn draw_icon_search(painter: &egui::Painter, center: Pos2, size: f32, color: Color32) {
    draw_lucide_icon(painter, center, lucide::SEARCH, size, color);
}

pub(super) fn draw_icon_refresh(painter: &egui::Painter, center: Pos2, size: f32, color: Color32) {
    draw_lucide_icon(painter, center, lucide::REFRESH, size, color);
}

pub(super) fn draw_icon_sticker(painter: &egui::Painter, center: Pos2, size: f32, color: Color32) {
    draw_lucide_icon(painter, center, lucide::SMILE, size, color);
}

pub(super) fn draw_icon_send(painter: &egui::Painter, center: Pos2, size: f32, color: Color32) {
    draw_lucide_icon(painter, center, lucide::SEND, size, color);
}

pub(super) fn draw_icon_phone(painter: &egui::Painter, center: Pos2, size: f32, color: Color32) {
    draw_lucide_icon(painter, center, lucide::PHONE, size, color);
}

pub(super) fn draw_icon_video(painter: &egui::Painter, center: Pos2, size: f32, color: Color32) {
    draw_lucide_icon(painter, center, lucide::VIDEO, size, color);
}

pub(super) fn draw_icon_image(painter: &egui::Painter, center: Pos2, size: f32, color: Color32) {
    draw_lucide_icon(painter, center, lucide::IMAGE, size, color);
}

pub(super) fn draw_icon_attachment(
    painter: &egui::Painter,
    center: Pos2,
    size: f32,
    color: Color32,
) {
    draw_lucide_icon(painter, center, lucide::PAPERCLIP, size, color);
}

pub(super) fn draw_icon_chat_nav(painter: &egui::Painter, center: Pos2, size: f32, color: Color32) {
    draw_lucide_icon(painter, center, lucide::MESSAGE_SQUARE, size, color);
}

pub(super) fn draw_icon_contacts(painter: &egui::Painter, center: Pos2, size: f32, color: Color32) {
    draw_lucide_icon(painter, center, lucide::USERS, size, color);
}

pub(super) fn draw_icon_cloud(painter: &egui::Painter, center: Pos2, size: f32, color: Color32) {
    draw_lucide_icon(painter, center, lucide::CLOUD, size, color);
}

pub(super) fn draw_icon_settings(painter: &egui::Painter, center: Pos2, size: f32, color: Color32) {
    draw_lucide_icon(painter, center, lucide::SETTINGS, size, color);
}

pub(super) fn draw_icon_drawer(painter: &egui::Painter, center: Pos2, size: f32, color: Color32) {
    draw_lucide_icon(painter, center, lucide::PANEL_RIGHT, size, color);
}

pub(super) fn draw_icon_pin(painter: &egui::Painter, center: Pos2, size: f32, color: Color32) {
    draw_lucide_icon(painter, center, lucide::PIN, size, color);
}

pub(super) fn draw_icon_copy(painter: &egui::Painter, center: Pos2, size: f32, color: Color32) {
    draw_lucide_icon(painter, center, lucide::COPY, size, color);
}

pub(super) fn draw_icon_user_plus(
    painter: &egui::Painter,
    center: Pos2,
    size: f32,
    color: Color32,
) {
    draw_lucide_icon(painter, center, lucide::USER_PLUS, size, color);
}

pub const REACTION_LIST: [(&str, &str, &str); 6] = [
    ("👍", "like", "Thích"),
    ("❤️", "heart", "Yêu thích"),
    ("😆", "smile", "Haha"),
    ("😮", "surprised", "Wow"),
    ("😢", "cry", "Buồn"),
    ("😠", "rage", "Phẫn nộ"),
];

pub(super) const APP_ICON_PNG: &[u8] = include_bytes!("../assets/app_icon.png");

/// Decode the embedded 256x256 RGBA application icon for the OS window/taskbar (`ViewportBuilder::with_icon`).
pub fn load_app_icon() -> Option<egui::IconData> {
    let img = image::load_from_memory(APP_ICON_PNG).ok()?.into_rgba8();
    let (width, height) = img.dimensions();
    Some(egui::IconData {
        rgba: img.into_raw(),
        width,
        height,
    })
}

pub(super) fn load_app_logo_texture(ctx: &egui::Context) -> Option<TextureHandle> {
    let img = image::load_from_memory(APP_ICON_PNG).ok()?.into_rgba8();
    let (w, h) = img.dimensions();
    let color_img =
        egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], img.as_raw());
    Some(ctx.load_texture("zalo_app_logo", color_img, TextureOptions::LINEAR))
}

pub(super) fn load_default_emoji_textures(ctx: &egui::Context) -> HashMap<String, TextureHandle> {
    let raw: [(&str, &[u8]); 6] = [
        ("like", include_bytes!("../assets/emojis/like.png")),
        ("heart", include_bytes!("../assets/emojis/heart.png")),
        ("smile", include_bytes!("../assets/emojis/smile.png")),
        (
            "surprised",
            include_bytes!("../assets/emojis/surprised.png"),
        ),
        ("cry", include_bytes!("../assets/emojis/cry.png")),
        ("rage", include_bytes!("../assets/emojis/rage.png")),
    ];
    let mut map = HashMap::new();
    for (name, bytes) in raw {
        if let Ok(img) = image::load_from_memory(bytes) {
            let rgba = img.to_rgba8();
            let (w, h) = rgba.dimensions();
            let color_img =
                egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], rgba.as_raw());
            let tex = ctx.load_texture(
                format!("zalo_emoji_{name}"),
                color_img,
                TextureOptions::LINEAR,
            );
            map.insert(name.to_string(), tex);
        }
    }
    map
}

/// Render an iOS/Zalo PC style rounded Toggle Switch in pure egui.
pub(super) fn render_toggle_switch(
    ui: &mut egui::Ui,
    value: &mut bool,
    pal: &ThemePalette,
) -> egui::Response {
    let (rect, mut resp) = ui.allocate_exact_size(Vec2::new(38.0, 20.0), Sense::click());
    if resp.clicked() {
        *value = !*value;
        resp.mark_changed();
    }
    resp = resp.on_hover_cursor(CursorIcon::PointingHand);

    let bg = if *value {
        pal.primary
    } else if pal.is_dark {
        Color32::from_rgb(58, 63, 75)
    } else {
        Color32::from_rgb(203, 213, 225)
    };
    ui.painter().rect_filled(rect, Rounding::same(10.0), bg);

    let knob_radius = 7.5_f32;
    let knob_x = if *value {
        rect.right() - 10.0
    } else {
        rect.left() + 10.0
    };
    ui.painter().circle_filled(
        Pos2::new(knob_x, rect.center().y),
        knob_radius,
        Color32::WHITE,
    );
    resp
}

fn cjk_font_key() -> egui::Id {
    egui::Id::new("cjk-font-initialized")
}

fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x1100..=0x11FF | 0x3040..=0x30FF | 0x3130..=0x318F | 0x3400..=0x4DBF
        | 0x4E00..=0x9FFF | 0xAC00..=0xD7AF | 0xF900..=0xFAFF)
}

/// Load the CJK fallback the first time Han / Kana / Hangul text shows up. msyh.ttc alone is
/// ~20 MB held in memory for the whole run, which a Vietnamese-only user never needs.
pub(super) fn ensure_cjk_font<'a>(ctx: &egui::Context, mut texts: impl Iterator<Item = &'a str>) {
    if ctx.data_mut(|data| data.get_temp::<bool>(cjk_font_key()).unwrap_or(false))
        || !texts.any(|t| t.chars().any(is_cjk))
    {
        return;
    }
    // Each context owns its font atlas; one window must not suppress another window's fallback.
    ctx.data_mut(|data| data.insert_temp(cjk_font_key(), true));
    ctx.set_fonts(custom_font_definitions_with_cjk(true));
}

fn custom_font_definitions() -> egui::FontDefinitions {
    custom_font_definitions_with_cjk(false)
}

fn custom_font_definitions_with_cjk(with_cjk: bool) -> egui::FontDefinitions {
    let mut fonts = egui::FontDefinitions::default();

    // 1. Embed official Lucide Icon Font (vector glyphs for all UI icons)
    const LUCIDE_BYTES: &[u8] = include_bytes!("../assets/fonts/lucide.ttf");
    fonts.font_data.insert(
        "lucide_font".to_owned(),
        egui::FontData::from_static(LUCIDE_BYTES),
    );

    fonts.font_data.insert(
        "emoji_slot".to_owned(),
        egui::FontData::from_static(include_bytes!("../assets/emojis/zalo/slot.ttf")),
    );
    fonts.families.insert(
        egui::FontFamily::Name("emoji_slot".into()),
        vec!["emoji_slot".to_owned()],
    );

    // Keep a bundled Vietnamese fallback without replacing the custom emoji families.
    fonts.font_data.insert(
        "bundled_sans".to_owned(),
        egui::FontData::from_static(include_bytes!("../assets/fonts/BeVietnamPro-Regular.ttf")),
    );

    // 2. Load system Vietnamese sans-serif font
    let font_candidates = [
        "C:\\Windows\\Fonts\\segoeui.ttf",
        "C:\\Windows\\Fonts\\arial.ttf",
        "C:\\Windows\\Fonts\\tahoma.ttf",
        "/System/Library/Fonts/Supplemental/Arial.ttf",
        "/System/Library/Fonts/Supplemental/Tahoma.ttf",
        "/Library/Fonts/Arial.ttf",
        "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf",
        "/usr/share/fonts/noto/NotoSans-Regular.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        "/usr/share/fonts/TTF/DejaVuSans.ttf",
        "/usr/share/fonts/truetype/ubuntu/Ubuntu-R.ttf",
        "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
    ];

    for path in font_candidates {
        if let Ok(font_data) = std::fs::read(path) {
            fonts.font_data.insert(
                "vietnamese_font".to_owned(),
                egui::FontData::from_owned(font_data),
            );
            break;
        }
    }

    // 3. Optional CJK (Chinese / Japanese / Korean) system font fallback (#5, #6)
    let cjk_candidates = [
        "C:\\Windows\\Fonts\\msyh.ttc",
        "C:\\Windows\\Fonts\\simsun.ttc",
        "C:\\Windows\\Fonts\\malgun.ttf",
        "C:\\Windows\\Fonts\\YuGothR.ttc",
        "/System/Library/Fonts/PingFang.ttc",
        "/System/Library/Fonts/Hiragino Sans GB.ttc",
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/google-noto-cjk/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/wqy-zenhei/wqy-zenhei.ttc",
    ];
    for path in cjk_candidates.into_iter().filter(|_| with_cjk) {
        if let Ok(font_data) = std::fs::read(path) {
            fonts
                .font_data
                .insert("cjk_font".to_owned(), egui::FontData::from_owned(font_data));
            break;
        }
    }

    // Lucide precedes fallback fonts so private-use UI icons keep their original glyphs.
    // The bundled fallbacks contain TrueType outlines usable by egui/ab_glyph.
    // Apple Color Emoji uses bitmap/color tables and is not an outline fallback.
    for (name, bytes) in [
        (
            "noto_sans_math",
            include_bytes!("../assets/fonts/NotoSansMath-Regular.ttf").as_slice(),
        ),
        (
            "noto_emoji_outline",
            include_bytes!("../assets/fonts/NotoEmoji-Variable.ttf").as_slice(),
        ),
    ] {
        fonts
            .font_data
            .insert(name.to_owned(), egui::FontData::from_static(bytes));
    }

    let prop = fonts
        .families
        .entry(egui::FontFamily::Proportional)
        .or_default();
    if fonts.font_data.contains_key("vietnamese_font") {
        prop.insert(0, "vietnamese_font".to_owned());
        prop.push("lucide_font".to_owned());
    } else {
        prop.insert(0, "lucide_font".to_owned());
    }

    let mono = fonts
        .families
        .entry(egui::FontFamily::Monospace)
        .or_default();
    if fonts.font_data.contains_key("vietnamese_font") {
        mono.push("vietnamese_font".to_owned());
    }
    mono.push("lucide_font".to_owned());
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        let fallbacks = fonts.families.entry(family).or_default();
        if fonts.font_data.contains_key("cjk_font") {
            fallbacks.push("cjk_font".to_owned());
        }
        fallbacks.push("bundled_sans".to_owned());
        fallbacks.push("noto_sans_math".to_owned());
        fallbacks.push("noto_emoji_outline".to_owned());
    }
    let mut bold = fonts
        .families
        .get(&egui::FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    for path in [
        "/System/Library/Fonts/Supplemental/Arial Bold.ttf",
        "C:\\Windows\\Fonts\\segoeuib.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
    ] {
        if let Ok(data) = std::fs::read(path) {
            fonts
                .font_data
                .insert("sidebar_bold".into(), egui::FontData::from_owned(data));
            bold.insert(0, "sidebar_bold".into());
            break;
        }
    }
    fonts
        .families
        .insert(egui::FontFamily::Name("sidebar-semibold".into()), bold);
    fonts
}

pub(super) fn setup_custom_fonts(ctx: &egui::Context) {
    ctx.data_mut(|data| data.insert_temp(cjk_font_key(), false));
    ctx.set_fonts(custom_font_definitions());
}

#[cfg(test)]
mod unicode_font_tests {
    use super::*;

    // Synthetic fixture using the exact non-ASCII codepoints of the reported message.
    const STYLED: &str = "𝗔𝗕𝗘𝗚𝗛𝗜𝗟𝗦𝗮𝗲𝗹𝗻𝗼𝘁𝘄 𝟬𝟭";
    const VIETNAMESE: &str = "Tiếng Việt: Nguyễn, Đặng, Ắ ằ ẵ ặ ế ệ ỡ ự ỵ";
    const SYMBOLS: &str = "🎃💥 ∑ ∫ ∞ → ≤ ≥";

    fn assert_visible_glyphs(ctx: &egui::Context, font: egui::FontId, text: &str) {
        ctx.fonts(|fonts| {
            for c in text.chars().filter(|c| !c.is_whitespace()) {
                assert!(
                    fonts.has_glyph(&font, c),
                    "missing glyph U+{:04X}",
                    c as u32
                );
            }
            let galley = fonts.layout_no_wrap(text.to_owned(), font, Color32::WHITE);
            let actual: String = galley
                .rows
                .iter()
                .flat_map(|row| row.glyphs.iter().map(|g| g.chr))
                .collect();
            assert_eq!(actual, text, "layout must preserve original Unicode text");
            for glyph in galley
                .rows
                .iter()
                .flat_map(|row| &row.glyphs)
                .filter(|g| !g.chr.is_whitespace())
            {
                assert!(
                    !glyph.uv_rect.is_nothing(),
                    "no rasterized outline U+{:04X}",
                    glyph.chr as u32
                );
                assert!(glyph.advance_width > 0.0);
            }
        });
    }

    #[test]
    fn cjk_loading_is_lazy_per_context_and_preserves_unicode_families() {
        let first = egui::Context::default();
        let second = egui::Context::default();
        for ctx in [&first, &second] {
            setup_custom_fonts(ctx);
            ensure_cjk_font(ctx, std::iter::once(VIETNAMESE));
            assert!(!ctx.data_mut(|data| data.get_temp::<bool>(cjk_font_key()).unwrap()));
            ensure_cjk_font(ctx, std::iter::once("漢字"));
            assert!(ctx.data_mut(|data| data.get_temp::<bool>(cjk_font_key()).unwrap()));
            ctx.begin_pass(egui::RawInput::default());
            assert_visible_glyphs(ctx, FontId::proportional(16.0), STYLED);
            assert_visible_glyphs(ctx, FontId::proportional(16.0), lucide::SEARCH);
            let _ = ctx.end_pass();
        }
        let fonts = custom_font_definitions_with_cjk(true);
        assert!(fonts
            .families
            .contains_key(&egui::FontFamily::Name("emoji_slot".into())));
        assert!(fonts
            .families
            .contains_key(&egui::FontFamily::Name("sidebar-semibold".into())));
    }

    #[test]
    fn unicode_fallbacks_preserve_styled_text_vietnamese_and_lucide() {
        let ctx = egui::Context::default();
        setup_custom_fonts(&ctx);
        ctx.begin_pass(egui::RawInput::default());
        for font in [
            egui::FontId::proportional(16.0),
            egui::FontId::monospace(16.0),
        ] {
            for text in [STYLED, VIETNAMESE, SYMBOLS, lucide::SEARCH, lucide::SEND] {
                assert_visible_glyphs(&ctx, font.clone(), text);
            }
        }
        let _ = ctx.end_pass();
    }

    #[test]
    fn bundled_fonts_rasterize_real_outlines_without_system_fallbacks() {
        let ctx = egui::Context::default();
        let mut fonts = custom_font_definitions();
        // An isolated family prevents a default/system font from masking a broken asset.
        for name in ["noto_sans_math", "noto_emoji_outline", "lucide_font"] {
            fonts
                .families
                .insert(egui::FontFamily::Name(name.into()), vec![name.to_owned()]);
        }
        ctx.set_fonts(fonts);
        ctx.begin_pass(egui::RawInput::default());
        for (name, text) in [
            ("noto_sans_math", STYLED),
            ("noto_emoji_outline", "🎃💥"),
            ("lucide_font", "\u{e151}\u{e4f2}"),
        ] {
            assert_visible_glyphs(
                &ctx,
                egui::FontId::new(18.0, egui::FontFamily::Name(name.into())),
                text,
            );
        }
        let _ = ctx.end_pass();
    }
}
