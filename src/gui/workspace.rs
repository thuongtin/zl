use super::navigation::paint_sidebar_avatar;
use super::*;

#[derive(Clone)]
struct SearchDebounce {
    query: String,
    changed_at: std::time::Instant,
    sent: bool,
}
#[derive(Clone)]
struct DraftDebounce {
    signature: String,
    changed_at: std::time::Instant,
    saved: bool,
}

/// Per-frame avatar index. An explicit UID never falls back to a namesake's avatar.
#[derive(Default)]
struct SenderAvatars<'a> {
    by_uid: HashMap<&'a str, &'a str>,
    by_name: HashMap<&'a str, Option<(&'a str, &'a str)>>,
}
impl<'a> SenderAvatars<'a> {
    fn new(threads: &'a [Thread]) -> Self {
        let mut index = Self::default();
        for thread in threads.iter().filter(|thread| !thread.is_group) {
            if let Some(avatar) = thread.avatar.as_deref().filter(|url| !url.is_empty()) {
                index.by_uid.insert(&thread.id, avatar);
            }
            // Include contacts without an avatar in ambiguity detection too.
            let value = thread
                .avatar
                .as_deref()
                .map(|avatar| (thread.id.as_str(), avatar));
            index
                .by_name
                .entry(&thread.name)
                .and_modify(|existing| {
                    if existing.as_ref().map(|(id, _)| *id) != Some(thread.id.as_str()) {
                        *existing = None;
                    }
                })
                .or_insert(value);
        }
        index
    }

    fn get(&self, sender: Option<&serde_json::Value>, name: &str) -> Option<&'a str> {
        match sender {
            Some(serde_json::Value::String(uid)) if !uid.is_empty() => {
                self.by_uid.get(uid.as_str()).copied()
            }
            Some(serde_json::Value::Number(uid)) => {
                self.by_uid.get(uid.to_string().as_str()).copied()
            }
            _ => self
                .by_name
                .get(name)
                .and_then(|entry| entry.as_ref().map(|(_, avatar)| *avatar)),
        }
    }
}

impl ZaloGuiApp {
    /// Start loading the full-resolution texture the image viewer shows. Message bubbles and
    /// the media grid keep a 480 px copy; the `hd_` key keeps this one (and its disk cache) apart.
    fn request_hd_texture(&mut self, full_url: &str) -> Option<String> {
        let lower_u = full_url.to_lowercase();
        let is_direct_img = lower_u.contains("zdn.vn")
            || lower_u.ends_with(".png")
            || lower_u.ends_with(".jpg")
            || lower_u.ends_with(".jpeg")
            || lower_u.ends_with(".webp")
            || lower_u.ends_with(".gif")
            || lower_u.ends_with(".bmp")
            || (!lower_u.starts_with("http://") && !lower_u.starts_with("https://"));
        if full_url.is_empty() || !is_direct_img {
            return None;
        }
        let hk = format!("hd_{}", crate::crypto::md5_hex(full_url));
        if !self.sticker_textures.contains_key(&hk) && self.loading_stickers.insert(hk.clone()) {
            if let Some(tx) = WORKER_TX.get() {
                let _ = tx.send(WorkerCmd::FetchStickerImage {
                    key: hk.clone(),
                    url: full_url.to_string(),
                });
            }
        }
        Some(hk)
    }

    pub(super) fn render_chat_workspace(&mut self, ctx: &egui::Context, pal: &ThemePalette) {
        ensure_cjk_font(ctx, std::iter::once(self.input_message.as_str()));
        let ime_key = Id::new(("composer-ime", &self.user_uin));
        let mut ime_active = ctx.data_mut(|d| d.get_temp::<bool>(ime_key).unwrap_or(false));
        let mut ime_frame = ime_active;
        ctx.input(|input| {
            for event in &input.events {
                if let egui::Event::Ime(event) = event {
                    ime_frame = true;
                    ime_active =
                        matches!(event, egui::ImeEvent::Enabled | egui::ImeEvent::Preedit(_));
                }
            }
        });
        ctx.data_mut(|d| d.insert_temp(ime_key, ime_active));
        if let Some(thread) = self.threads.get(self.selected_thread_index) {
            let thread_id = thread.id.clone();
            let initial_key = Id::new(("workspace-initial-page", &self.user_uin, &thread_id));
            if !ctx.data_mut(|data| data.get_temp::<bool>(initial_key).unwrap_or(false)) {
                if self.input_message.is_empty()
                    && self.replying_to.is_none()
                    && self.composer_attachments.is_empty()
                {
                    if let Some(store) = self.composer_store.as_ref() {
                        let draft = store.draft(&thread_id);
                        self.input_message = draft.text;
                        self.replying_to = draft.reply;
                        self.composer_attachments = draft.attachments;
                    }
                }
                if let Some(tx) = WORKER_TX.get() {
                    let _ = tx.send(WorkerCmd::LoadLocalPage {
                        request_id: {
                            self.navigation_request = self.navigation_request.wrapping_add(1);
                            self.pending_jump = None;
                            self.navigation_request
                        },
                        thread_id: thread_id.clone(),
                        cursor: None,
                        direction: crate::store_v2::PageDirection::Before,
                        replace: true,
                    });
                }
                ctx.data_mut(|data| data.insert_temp(initial_key, true));
            }
            let search_key = Id::new(("workspace-search-active", &self.user_uin, &thread_id));
            let active = self.in_chat_search_open && !self.in_chat_search_query.trim().is_empty();
            let was_active =
                ctx.data_mut(|data| data.get_temp::<bool>(search_key).unwrap_or(false));
            if !was_active && active {
                ctx.data_mut(|data| {
                    data.remove::<SearchDebounce>(Id::new((
                        "db-search-debounce",
                        &self.user_uin,
                        &Some(thread_id.clone()),
                    )))
                });
            }
            if was_active && !active {
                self.message_window.newer = 0;
                if let Some(tx) = WORKER_TX.get() {
                    let _ = tx.send(WorkerCmd::LoadLocalPage {
                        request_id: {
                            self.navigation_request = self.navigation_request.wrapping_add(1);
                            self.pending_jump = None;
                            self.navigation_request
                        },
                        thread_id,
                        cursor: None,
                        direction: crate::store_v2::PageDirection::Before,
                        replace: true,
                    });
                }
            }
            ctx.data_mut(|data| data.insert_temp(search_key, active));
        }
        // Responsive breakpoints based on live viewport width
        // -------------------------------------------------------------------
        let screen_rect = ctx.screen_rect();
        let screen_w = screen_rect.width();
        let is_compact_w = screen_w < 780.0;
        let is_narrow_w = screen_w < 620.0;
        let left_rail_w = if is_narrow_w { 52.0 } else { 62.0 };

        // -------------------------------------------------------------------
        // 1. LEFT NAVIGATION RAIL (Replaces top bar & bottom bar)
        // -------------------------------------------------------------------
        egui::SidePanel::left("left_nav_rail")
            .resizable(false)
            .exact_width(left_rail_w)
            .frame(
                egui::Frame::none()
                    .fill(pal.bg_app)
                    .inner_margin(Margin::symmetric(if is_narrow_w { 4.0 } else { 8.0 }, 14.0))
                    .stroke(Stroke::new(1.0_f32, pal.border)),
            )
            .show(ctx, |ui| {
                ui.vertical_centered(|ui| {
                    // User Avatar Button with Cutout Online Status Dot
                    let (av_rect, av_resp) =
                        ui.allocate_exact_size(Vec2::new(40.0, 40.0), Sense::click());
                    let user_disp = if self.user_name.is_empty() {
                        "Người dùng Zalo"
                    } else {
                        &self.user_name
                    };
                    paint_user_avatar(
                        ui.painter(),
                        av_rect.center(),
                        20.0,
                        user_disp,
                        false,
                        self.user_avatar.as_deref(),
                        &self.sticker_textures,
                        &mut self.loading_stickers,
                    );
                    if av_resp.hovered() || self.profile_menu_open {
                        ui.painter().circle_stroke(
                            av_rect.center(),
                            21.0,
                            Stroke::new(2.0_f32, pal.primary_hover),
                        );
                    }
                    // Cutout status dot
                    if self.settings.show_online_status {
                        let dot_c = Pos2::new(av_rect.right() - 4.0, av_rect.bottom() - 4.0);
                        ui.painter().circle_filled(dot_c, 6.5, pal.bg_app);
                        ui.painter().circle_filled(dot_c, 4.5, pal.accent_online);
                    }

                    if av_resp
                        .on_hover_cursor(CursorIcon::PointingHand)
                        .on_hover_text(format!(
                            "{} (Đang trực tuyến - Bấm để mở menu tài khoản)",
                            if self.user_name.is_empty() {
                                "Tài khoản Zalo"
                            } else {
                                &self.user_name
                            }
                        ))
                        .clicked()
                    {
                        self.profile_menu_open = !self.profile_menu_open;
                    }

                    ui.add_space(18.0);

                    // Navigation Icon 1: Tin nhắn (All / Active Chat)
                    let is_msg_nav = self.thread_filter == ThreadFilter::All;
                    let (nav1_rect, nav1_resp) =
                        ui.allocate_exact_size(Vec2::new(42.0, 42.0), Sense::click());
                    if is_msg_nav {
                        ui.painter().rect_filled(
                            nav1_rect,
                            Rounding::same(11.0),
                            pal.primary.linear_multiply(0.18),
                        );
                        let pill = Rect::from_min_size(
                            Pos2::new(nav1_rect.left() - 6.0, nav1_rect.center().y - 10.0),
                            Vec2::new(3.5, 20.0),
                        );
                        ui.painter()
                            .rect_filled(pill, Rounding::same(2.0), pal.primary);
                    } else if nav1_resp.hovered() {
                        ui.painter()
                            .rect_filled(nav1_rect, Rounding::same(11.0), pal.bg_hover);
                    }
                    draw_icon_chat_nav(
                        ui.painter(),
                        nav1_rect.center(),
                        19.0,
                        if is_msg_nav {
                            pal.primary_hover
                        } else {
                            pal.text_secondary
                        },
                    );
                    if nav1_resp
                        .on_hover_cursor(CursorIcon::PointingHand)
                        .on_hover_text("Tin nhắn")
                        .clicked()
                    {
                        self.thread_filter = ThreadFilter::All;
                    }

                    ui.add_space(6.0);

                    // Navigation Icon 2: Danh bạ / Cá nhân
                    let is_contacts_nav = self.thread_filter == ThreadFilter::Direct;
                    let (nav2_rect, nav2_resp) =
                        ui.allocate_exact_size(Vec2::new(42.0, 42.0), Sense::click());
                    if is_contacts_nav {
                        ui.painter().rect_filled(
                            nav2_rect,
                            Rounding::same(11.0),
                            pal.primary.linear_multiply(0.18),
                        );
                        let pill = Rect::from_min_size(
                            Pos2::new(nav2_rect.left() - 6.0, nav2_rect.center().y - 10.0),
                            Vec2::new(3.5, 20.0),
                        );
                        ui.painter()
                            .rect_filled(pill, Rounding::same(2.0), pal.primary);
                    } else if nav2_resp.hovered() {
                        ui.painter()
                            .rect_filled(nav2_rect, Rounding::same(11.0), pal.bg_hover);
                    }
                    draw_icon_contacts(
                        ui.painter(),
                        nav2_rect.center(),
                        19.0,
                        if is_contacts_nav {
                            pal.primary_hover
                        } else {
                            pal.text_secondary
                        },
                    );
                    if nav2_resp
                        .on_hover_cursor(CursorIcon::PointingHand)
                        .on_hover_text(format!("Danh bạ ({} liên hệ)", self.threads.len()))
                        .clicked()
                    {
                        self.thread_filter = if self.thread_filter == ThreadFilter::Direct {
                            ThreadFilter::All
                        } else {
                            ThreadFilter::Direct
                        };
                    }

                    ui.add_space(6.0);

                    // Navigation Icon 3: Cloud của tôi
                    let is_cloud_selected = self
                        .threads
                        .get(self.selected_thread_index)
                        .map(|t| t.name.contains("Cloud của tôi"))
                        .unwrap_or(false);
                    let (nav3_rect, nav3_resp) =
                        ui.allocate_exact_size(Vec2::new(42.0, 42.0), Sense::click());
                    if is_cloud_selected {
                        ui.painter().rect_filled(
                            nav3_rect,
                            Rounding::same(11.0),
                            pal.primary.linear_multiply(0.18),
                        );
                    } else if nav3_resp.hovered() {
                        ui.painter()
                            .rect_filled(nav3_rect, Rounding::same(11.0), pal.bg_hover);
                    }
                    draw_icon_cloud(
                        ui.painter(),
                        nav3_rect.center(),
                        19.0,
                        if is_cloud_selected {
                            pal.primary_hover
                        } else {
                            pal.text_secondary
                        },
                    );
                    if nav3_resp
                        .on_hover_cursor(CursorIcon::PointingHand)
                        .on_hover_text("Cloud của tôi")
                        .clicked()
                    {
                        if let Some((cidx, cthread)) = self
                            .threads
                            .iter()
                            .enumerate()
                            .find(|(_, t)| t.name.contains("Cloud của tôi"))
                        {
                            let cid = cthread.id.clone();
                            self.workspace_select_conversation(ctx, cidx);
                            if let Some(tx) = WORKER_TX.get() {
                                let _ = tx.send(WorkerCmd::LoadGroupHistory {
                                    group_id: cid,
                                    is_group: false,
                                });
                            }
                        }
                    }

                    // Push bottom actions to the bottom of the Left Rail
                    let rem_h = (ui.available_height() - 100.0).max(12.0);
                    ui.add_space(rem_h);

                    // Bottom Action 1: Background Sync Indicator Icon
                    let (sync_rect, sync_resp) =
                        ui.allocate_exact_size(Vec2::new(40.0, 40.0), Sense::click());
                    if sync_resp.hovered() {
                        ui.painter()
                            .rect_filled(sync_rect, Rounding::same(10.0), pal.bg_hover);
                    }
                    draw_icon_refresh(
                        ui.painter(),
                        sync_rect.center(),
                        17.0,
                        if sync_resp.hovered() {
                            pal.accent_online
                        } else {
                            pal.text_secondary
                        },
                    );
                    ui.painter().circle_filled(
                        Pos2::new(sync_rect.right() - 9.0, sync_rect.top() + 9.0),
                        3.0,
                        pal.accent_online,
                    );
                    if sync_resp
                        .on_hover_cursor(CursorIcon::PointingHand)
                        .on_hover_text(format!(
                            "Trạng thái: {} · {} hội thoại (Bấm để đồng bộ ngay)",
                            self.status_msg,
                            self.threads.len()
                        ))
                        .clicked()
                    {
                        if let Some(tx) = WORKER_TX.get() {
                            let _ = tx.send(WorkerCmd::RefreshThreads);
                            let _ = tx.send(WorkerCmd::SyncDirectHistory);
                        }
                        self.show_toast(format!(
                            "Đang đồng bộ ngầm {} hội thoại…",
                            self.threads.len()
                        ));
                    }

                    ui.add_space(6.0);

                    // Bottom Action 2: Theme / Settings Icon
                    // Bottom Action 2: Settings Modal Button
                    let (set_rect, set_resp) =
                        ui.allocate_exact_size(Vec2::new(40.0, 40.0), Sense::click());
                    if self.settings_modal_open {
                        ui.painter().rect_filled(
                            set_rect,
                            Rounding::same(10.0),
                            pal.primary.linear_multiply(0.18),
                        );
                    } else if set_resp.hovered() {
                        ui.painter()
                            .rect_filled(set_rect, Rounding::same(10.0), pal.bg_hover);
                    }
                    draw_icon_settings(
                        ui.painter(),
                        set_rect.center(),
                        17.5,
                        if self.settings_modal_open {
                            pal.primary_hover
                        } else if set_resp.hovered() {
                            pal.text_primary
                        } else {
                            pal.text_secondary
                        },
                    );
                    if set_resp
                        .on_hover_cursor(CursorIcon::PointingHand)
                        .on_hover_text("Cài đặt Zalo")
                        .clicked()
                    {
                        self.settings_modal_open = !self.settings_modal_open;
                    }
                });
            });

        // -------------------------------------------------------------------
        // 2. CONVERSATION LIST SIDEBAR (Responsive width, auto-adapts on resize)
        // -------------------------------------------------------------------
        let sidebar_default_w = if is_narrow_w {
            76.0
        } else if is_compact_w {
            240.0
        } else {
            300.0
        };
        let (sidebar_id, sidebar_min_w, sidebar_max_w) = if is_narrow_w {
            ("conversation_sidebar_narrow", 68.0, 68.0)
        } else {
            (
                "conversation_sidebar_wide",
                190.0,
                (screen_w * 0.45).clamp(240.0, 420.0),
            )
        };

        egui::SidePanel::left(sidebar_id)
            .resizable(!is_narrow_w)
            .default_width(sidebar_default_w)
            .width_range(sidebar_min_w..=sidebar_max_w)
            .frame(
                egui::Frame::none()
                    .fill(pal.bg_sidebar)
                    .stroke(Stroke::new(1.0_f32, pal.border)),
            )
            .show(ctx, |ui| {
                egui::Frame::none()
                    .inner_margin(Margin {
                        left: if is_narrow_w { 4.0 } else { 14.0 },
                        right: if is_narrow_w { 4.0 } else { 14.0 },
                        top: 12.0,
                        bottom: 0.0,
                    })
                    .show(ui, |ui| {
                        if is_narrow_w {
                            // Compact mode: icon-only search / lookup button
                            ui.vertical_centered(|ui| {
                                let (srch_r, srch_resp) =
                                    ui.allocate_exact_size(Vec2::new(42.0, 32.0), Sense::click());
                                ui.painter()
                                    .rect_filled(srch_r, Rounding::same(8.0), pal.bg_input);
                                ui.painter().rect_stroke(
                                    srch_r,
                                    Rounding::same(8.0),
                                    Stroke::new(1.0_f32, pal.border),
                                );
                                draw_icon_search(
                                    ui.painter(),
                                    srch_r.center(),
                                    16.0,
                                    pal.primary_hover,
                                );
                                if srch_resp
                                    .on_hover_cursor(CursorIcon::PointingHand)
                                    .on_hover_text("Tìm kiếm hội thoại & Tra cứu SĐT")
                                    .clicked()
                                {
                                    self.phone_modal_open = true;
                                }
                            });
                            ui.add_space(6.0);
                        } else {
                            // Search Box + Quick Phone Lookup (`+👤`) Button
                            let avail_row_w = ui.available_width();
                            let mut trigger_phone_lookup: Option<String> = None;
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 6.0;
                                let search_box_w = (avail_row_w - 38.0).max(110.0);
                                let has_q = !self.search_query.is_empty();
                                egui::Frame::none()
                                    .fill(pal.bg_input)
                                    .rounding(Rounding::same(8.0))
                                    .stroke(Stroke::new(
                                        1.0_f32,
                                        if has_q {
                                            pal.primary.linear_multiply(0.65)
                                        } else {
                                            pal.border
                                        },
                                    ))
                                    .inner_margin(Margin::symmetric(9.0, 6.0))
                                    .show(ui, |ui| {
                                        let inner_w = (search_box_w - 18.0).max(80.0);
                                        ui.set_width(inner_w);
                                        ui.horizontal(|ui| {
                                            ui.spacing_mut().item_spacing.x = 6.0;
                                            let (icon_rect, _) = ui.allocate_exact_size(
                                                Vec2::new(15.0, 18.0),
                                                Sense::hover(),
                                            );
                                            draw_icon_search(
                                                ui.painter(),
                                                icon_rect.center(),
                                                13.5,
                                                if has_q {
                                                    pal.primary_hover
                                                } else {
                                                    pal.text_muted
                                                },
                                            );
                                            let edit_w = if has_q {
                                                (inner_w - 42.0).max(48.0)
                                            } else {
                                                (inner_w - 22.0).max(60.0)
                                            };
                                            let s_resp = ui.add(
                                                egui::TextEdit::singleline(&mut self.search_query)
                                                    .id(Id::new("sidebar_search_input"))
                                                    .frame(false)
                                                    .hint_text("Tìm hội thoại, tin nhắn, SĐT...")
                                                    .desired_width(edit_w),
                                            );
                                            let s_resp = s_resp
                                                .on_hover_text("Tìm hội thoại, tin nhắn, SĐT");
                                            let s_enter = (s_resp.lost_focus()
                                                || s_resp.has_focus())
                                                && ui.input(|i| i.key_pressed(Key::Enter));
                                            let digits_cnt = self
                                                .search_query
                                                .chars()
                                                .filter(|c| c.is_ascii_digit())
                                                .count();
                                            if s_enter && digits_cnt >= 8 {
                                                trigger_phone_lookup =
                                                    Some(self.search_query.trim().to_string());
                                            }
                                            if has_q {
                                                let (clr_rect, clr_resp) = ui.allocate_exact_size(
                                                    Vec2::new(16.0, 16.0),
                                                    Sense::click(),
                                                );
                                                if clr_resp.hovered() {
                                                    ui.painter().circle_filled(
                                                        clr_rect.center(),
                                                        8.0,
                                                        pal.bg_hover,
                                                    );
                                                }
                                                draw_lucide_icon(
                                                    ui.painter(),
                                                    clr_rect.center(),
                                                    lucide::X,
                                                    11.5,
                                                    if clr_resp.hovered() {
                                                        pal.text_primary
                                                    } else {
                                                        pal.text_muted
                                                    },
                                                );
                                                if clr_resp
                                                    .on_hover_cursor(CursorIcon::PointingHand)
                                                    .on_hover_text("Xóa tìm kiếm")
                                                    .clicked()
                                                {
                                                    self.search_query.clear();
                                                }
                                            }
                                        });
                                    });

                                // Add Friend / Lookup by Phone Button (`+👤`)
                                let (add_rect, add_resp) =
                                    ui.allocate_exact_size(Vec2::new(32.0, 32.0), Sense::click());
                                ui.painter().rect_filled(
                                    add_rect,
                                    Rounding::same(8.0),
                                    if self.phone_modal_open {
                                        pal.primary.linear_multiply(0.22)
                                    } else if add_resp.hovered() {
                                        pal.bg_hover
                                    } else {
                                        pal.bg_input
                                    },
                                );
                                ui.painter().rect_stroke(
                                    add_rect,
                                    Rounding::same(8.0),
                                    Stroke::new(
                                        1.0_f32,
                                        if self.phone_modal_open || add_resp.hovered() {
                                            pal.primary
                                        } else {
                                            pal.border
                                        },
                                    ),
                                );
                                draw_icon_user_plus(
                                    ui.painter(),
                                    add_rect.center(),
                                    16.5,
                                    if self.phone_modal_open || add_resp.hovered() {
                                        pal.primary_hover
                                    } else {
                                        pal.text_secondary
                                    },
                                );
                                if add_resp
                                    .on_hover_cursor(CursorIcon::PointingHand)
                                    .on_hover_text("Tìm kiếm số điện thoại Zalo & Kết bạn")
                                    .clicked()
                                {
                                    self.phone_modal_open = true;
                                    let digits_cnt = self
                                        .search_query
                                        .chars()
                                        .filter(|c| c.is_ascii_digit())
                                        .count();
                                    if digits_cnt >= 8 && self.phone_modal_input.is_empty() {
                                        self.phone_modal_input =
                                            self.search_query.trim().to_string();
                                    }
                                }
                            });

                            if let Some(phone_str) = trigger_phone_lookup {
                                self.phone_modal_open = true;
                                self.phone_modal_input = phone_str.clone();
                                self.phone_lookup_loading = true;
                                self.phone_lookup_error = None;
                                self.phone_lookup_result = None;
                                if let Some(tx) = WORKER_TX.get() {
                                    let _ = tx.send(WorkerCmd::LookupPhone { phone: phone_str });
                                }
                            }
                            ui.add_space(10.0);

                            // Clean Underline Segmented Tabs (`Tất cả`, `Cá nhân`, `Nhóm`) + `Đã ghim (1)`
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 16.0;
                                for (filter, label) in [
                                    (ThreadFilter::All, "Tất cả"),
                                    (ThreadFilter::Direct, "Cá nhân"),
                                    (ThreadFilter::Groups, "Nhóm"),
                                ] {
                                    let active = self.thread_filter == filter;
                                    let text_w = ui.fonts(|f| {
                                        f.layout_no_wrap(
                                            label.to_string(),
                                            FontId::proportional(12.5),
                                            pal.text_primary,
                                        )
                                        .size()
                                        .x
                                    });
                                    let (tab_rect, tab_resp) = ui.allocate_exact_size(
                                        Vec2::new(text_w + 4.0, 26.0),
                                        Sense::click(),
                                    );
                                    tab_resp.widget_info(|| {
                                        egui::WidgetInfo::selected(
                                            egui::WidgetType::SelectableLabel,
                                            ui.is_enabled(),
                                            active,
                                            label,
                                        )
                                    });
                                    let fg = if active {
                                        pal.primary_hover
                                    } else if tab_resp.hovered() {
                                        pal.text_primary
                                    } else {
                                        pal.text_secondary
                                    };
                                    ui.painter().text(
                                        Pos2::new(tab_rect.center().x, tab_rect.top() + 8.0),
                                        egui::Align2::CENTER_CENTER,
                                        label,
                                        FontId::proportional(12.5),
                                        fg,
                                    );
                                    if active {
                                        let line_y = tab_rect.bottom() - 1.0;
                                        ui.painter().line_segment(
                                            [
                                                Pos2::new(tab_rect.left(), line_y),
                                                Pos2::new(tab_rect.right(), line_y),
                                            ],
                                            Stroke::new(2.2_f32, pal.primary),
                                        );
                                    }
                                    if tab_resp.on_hover_cursor(CursorIcon::PointingHand).clicked()
                                    {
                                        self.thread_filter = filter;
                                    }
                                }

                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    let pinned_cnt = self
                                        .threads
                                        .iter()
                                        .filter(|t| t.name.contains("Cloud của tôi"))
                                        .count();
                                    if pinned_cnt > 0 {
                                        ui.label(
                                            RichText::new(format!("Đã ghim ({pinned_cnt})"))
                                                .size(11.0)
                                                .color(pal.text_muted),
                                        );
                                    }
                                });
                            });
                        }
                    });

                ui.add(egui::Separator::default().spacing(1.0));

                // Sort threads: "Cloud của tôi" pinned first, then descending by latest message timestamp
                let query = self.search_query.trim().to_lowercase();
                let query_folded = fold_vn_search(&query);
                let query_digits: String = self
                    .search_query
                    .chars()
                    .filter(|c| c.is_ascii_digit())
                    .collect();
                let is_phone_like_query = query_digits.len() >= 8
                    && self
                        .search_query
                        .trim()
                        .chars()
                        .all(|c| c.is_ascii_digit() || matches!(c, '+' | ' ' | '-' | '.'));

                if is_phone_like_query {
                    let raw_q = self.search_query.trim().to_string();
                    egui::Frame::none()
                        .fill(pal.primary.linear_multiply(0.14))
                        .stroke(Stroke::new(1.0_f32, pal.primary))
                        .rounding(Rounding::same(9.0))
                        .inner_margin(Margin::symmetric(10.0, 7.0))
                        .outer_margin(Margin::symmetric(8.0, 4.0))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                let (ir, _) =
                                    ui.allocate_exact_size(Vec2::splat(20.0), Sense::hover());
                                draw_icon_user_plus(
                                    ui.painter(),
                                    ir.center(),
                                    16.0,
                                    pal.primary_hover,
                                );
                                ui.vertical(|ui| {
                                    ui.label(
                                        RichText::new(format!("Tìm Zalo qua SĐT: {raw_q}"))
                                            .size(12.2)
                                            .color(pal.text_primary)
                                            .strong(),
                                    );
                                    ui.label(
                                        RichText::new("Nhấn nút Tìm hoặc phím Enter để tra cứu")
                                            .size(10.8)
                                            .color(pal.text_secondary),
                                    );
                                });
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    let btn = egui::Button::new(
                                        RichText::new("Tìm SĐT")
                                            .size(11.5)
                                            .color(Color32::WHITE)
                                            .strong(),
                                    )
                                    .fill(pal.primary)
                                    .rounding(Rounding::same(6.0));
                                    if ui
                                        .add(btn)
                                        .on_hover_cursor(CursorIcon::PointingHand)
                                        .clicked()
                                    {
                                        self.phone_modal_open = true;
                                        self.phone_modal_input = raw_q.clone();
                                        self.phone_lookup_loading = true;
                                        self.phone_lookup_error = None;
                                        self.phone_lookup_result = None;
                                        if let Some(tx) = WORKER_TX.get() {
                                            let _ =
                                                tx.send(WorkerCmd::LookupPhone { phone: raw_q });
                                        }
                                    }
                                });
                            });
                        });
                }

                let mut filtered_indices: Vec<usize> = self
                    .threads
                    .iter()
                    .enumerate()
                    .filter(|(_, t)| {
                        let pass_filter = match self.thread_filter {
                            ThreadFilter::All => true,
                            ThreadFilter::Direct => !t.is_group,
                            ThreadFilter::Groups => t.is_group,
                        };
                        pass_filter
                            && (query.is_empty()
                                || matches_vn_query(&t.name, &query, &query_folded)
                                || t.id.contains(&query))
                    })
                    .map(|(i, _)| i)
                    .collect();

                filtered_indices.sort_by(|&a, &b| {
                    let ta = &self.threads[a];
                    let tb = &self.threads[b];
                    let a_cloud = ta.name.contains("Cloud của tôi");
                    let b_cloud = tb.name.contains("Cloud của tôi");
                    if a_cloud != b_cloud {
                        return b_cloud.cmp(&a_cloud);
                    }
                    let ts_a = ta.sidebar_activity_ts();
                    let ts_b = tb.sidebar_activity_ts();
                    ts_b.cmp(&ts_a)
                });

                let search_text = self.search_query.trim().to_owned();
                self.queue_workspace_search(ctx, &search_text, None);
                let msg_hits: Vec<(usize, String, String, String, bool)> = if query.len() >= 2 {
                    self.db_search_results
                        .iter()
                        .filter_map(|(thread_id, message)| {
                            let index = self.threads.iter().position(|t| &t.id == thread_id)?;
                            let thread = &self.threads[index];
                            Some((
                                index,
                                thread.name.clone(),
                                message.text.clone(),
                                message.time.clone(),
                                thread.is_group,
                            ))
                        })
                        .collect()
                } else {
                    Vec::new()
                };

                let mut clicked_thread: Option<(usize, String, bool)> = None;
                let mut clicked_msg_hit: Option<(usize, String)> = None;

                egui::ScrollArea::vertical()
                    .id_salt("threads_list_scroll")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.add_space(4.0);
                        for &i in &filtered_indices {
                            let t = &self.threads[i];
                            let is_selected = i == self.selected_thread_index;
                            let is_pinned_cloud = t.name.contains("Cloud của tôi");

                            let row_height = if is_narrow_w { 50.0 } else { 58.0 };
                            let row_width = (ui.available_width() - 8.0).max(44.0);
                            let (row_rect, row_resp) = ui.allocate_exact_size(
                                Vec2::new(row_width, row_height),
                                Sense::click(),
                            );
                            row_resp.widget_info(|| {
                                egui::WidgetInfo::selected(
                                    egui::WidgetType::SelectableLabel,
                                    ui.is_enabled(),
                                    is_selected,
                                    &t.name,
                                )
                            });
                            let row_rect = row_rect.translate(Vec2::new(4.0, 0.0));
                            if !ui.is_rect_visible(row_rect) {
                                continue;
                            }

                            let bg = if is_selected {
                                pal.bg_selected
                            } else if row_resp.hovered() {
                                pal.bg_hover
                            } else {
                                Color32::TRANSPARENT
                            };

                            ui.painter().rect_filled(row_rect, Rounding::same(11.0), bg);

                            // Left active indicator bar (`w-1 bg-zalo-500 rounded-r-full`)
                            if is_selected {
                                let bar_rect = Rect::from_min_size(
                                    Pos2::new(row_rect.left() + 1.0, row_rect.top() + 10.0),
                                    Vec2::new(3.5, 38.0),
                                );
                                ui.painter().rect_filled(
                                    bar_rect,
                                    Rounding::same(2.0),
                                    pal.primary,
                                );
                            }

                            // Avatar circle with real user/group image & Cutout Status Dot
                            let av_radius = if is_narrow_w { 18.0 } else { 20.5 };
                            let avatar_center = if is_narrow_w {
                                row_rect.center()
                            } else {
                                Pos2::new(row_rect.left() + 29.0, row_rect.center().y)
                            };
                            paint_sidebar_avatar(
                                ui.painter(),
                                avatar_center,
                                av_radius,
                                t,
                                &self.sticker_textures,
                                &mut self.loading_stickers,
                                pal,
                            );
                            // Presence belongs to direct conversations, not group collages.
                            if !t.is_group {
                                let cutout_col = if is_selected {
                                    pal.bg_selected
                                } else {
                                    pal.bg_sidebar
                                };
                                let badge_pos = Pos2::new(
                                    avatar_center.x + av_radius * 0.71,
                                    avatar_center.y + av_radius * 0.71,
                                );
                                ui.painter().circle_filled(badge_pos, 6.0, cutout_col);
                                ui.painter().circle_filled(
                                    badge_pos,
                                    4.0,
                                    if self.presence_map.get(&t.id).is_some_and(|p| {
                                        p.is_recent(crate::login::unix_now() * 1000)
                                    }) {
                                        pal.accent_online
                                    } else {
                                        pal.text_muted
                                    },
                                );
                            }

                            // Only show text labels when sidebar is expanded
                            if !is_narrow_w {
                                // Thread Name
                                let text_left = row_rect.left() + 58.0;
                                let name_color = if is_selected && pal.is_dark {
                                    Color32::WHITE
                                } else {
                                    pal.text_primary
                                };
                                let max_name_chars = if is_compact_w { 15 } else { 21 };
                                let display_name = truncate_chars(&t.name, max_name_chars);
                                ui.painter().text(
                                    Pos2::new(text_left, row_rect.top() + 12.0),
                                    egui::Align2::LEFT_TOP,
                                    display_name,
                                    if t.sidebar_font_style()
                                        == crate::chat::SidebarFontStyle::Semibold
                                    {
                                        FontId::new(
                                            13.6,
                                            egui::FontFamily::Name("sidebar-semibold".into()),
                                        )
                                    } else {
                                        FontId::proportional(13.6)
                                    },
                                    name_color,
                                );

                                // Pin icon + Timestamp (top-right)
                                let time_right = row_rect.right() - 9.0;
                                let sidebar_time =
                                    t.sidebar_timestamp(crate::login::unix_now() * 1000);
                                if !sidebar_time.is_empty() {
                                    let tw = ui.fonts(|f| {
                                        f.layout_no_wrap(
                                            sidebar_time.clone(),
                                            FontId::proportional(11.0),
                                            pal.text_muted,
                                        )
                                        .size()
                                        .x
                                    });
                                    ui.painter().text(
                                        Pos2::new(time_right, row_rect.top() + 13.5),
                                        egui::Align2::RIGHT_TOP,
                                        &sidebar_time,
                                        FontId::proportional(11.0),
                                        pal.text_muted,
                                    );
                                    if is_pinned_cloud {
                                        draw_icon_pin(
                                            ui.painter(),
                                            Pos2::new(time_right - tw - 9.0, row_rect.top() + 19.5),
                                            11.0,
                                            pal.text_muted,
                                        );
                                    }
                                }

                                // Last sender is retained in durable metadata for unloaded pages.
                                let mut raw_preview = if t.preview.is_empty() {
                                    "Chưa có tin nhắn".to_string()
                                } else {
                                    t.sidebar_preview()
                                };
                                if t.unread.has_unread_mentions() {
                                    raw_preview = format!("@ {raw_preview}");
                                }
                                let unread_n = t.unread.count();
                                let unread_badge = t.unread.badge(t.is_group);
                                // Reserve the unread badge, then elide the full emoji-aware
                                // preview by pixels instead of cutting Unicode scalars/aliases.
                                let badge_space = if unread_badge.is_some() {
                                    if unread_n >= 10 {
                                        36.0
                                    } else {
                                        32.0
                                    }
                                } else {
                                    9.0
                                };
                                let snippet_rect = Rect::from_min_max(
                                    Pos2::new(text_left, row_rect.top() + 31.5),
                                    Pos2::new(
                                        row_rect.right() - badge_space,
                                        row_rect.top() + 51.5,
                                    ),
                                );
                                emoji_text::paint_preview(
                                    ui,
                                    &raw_preview,
                                    snippet_rect,
                                    12.0,
                                    if unread_badge.is_some() {
                                        pal.text_primary
                                    } else {
                                        pal.text_secondary
                                    },
                                );
                                if unread_badge.is_some() {
                                    let badge_txt = unread_badge.clone().unwrap_or_default();
                                    let is_dot = badge_txt.is_empty();
                                    let bw = if is_dot {
                                        8.0
                                    } else if badge_txt.len() > 1 {
                                        22.0
                                    } else {
                                        18.0
                                    };
                                    let b_rect = Rect::from_min_size(
                                        Pos2::new(
                                            row_rect.right() - bw - 8.0,
                                            row_rect.top() + 31.5,
                                        ),
                                        Vec2::new(bw, if is_dot { 8.0 } else { 17.0 }),
                                    );
                                    ui.painter().rect_filled(
                                        b_rect,
                                        Rounding::same(8.5),
                                        if t.unread.is_muted() {
                                            pal.text_muted
                                        } else {
                                            pal.danger
                                        },
                                    );
                                    ui.painter().text(
                                        b_rect.center(),
                                        egui::Align2::CENTER_CENTER,
                                        badge_txt,
                                        FontId::proportional(10.2),
                                        Color32::WHITE,
                                    );
                                }
                            }

                            let row_resp = row_resp.on_hover_cursor(CursorIcon::PointingHand);
                            let row_resp = if is_narrow_w {
                                row_resp.on_hover_text(&t.name)
                            } else {
                                row_resp
                            };
                            if row_resp.clicked() {
                                clicked_thread = Some((i, t.id.clone(), t.is_group));
                            }
                        }

                        // Global Message Content Matches (`Tin nhắn tìm thấy`)
                        if !msg_hits.is_empty() {
                            ui.add_space(8.0);
                            ui.horizontal(|ui| {
                                ui.add_space(12.0);
                                ui.label(
                                    RichText::new(format!(
                                        "TIN NHẮN TÌM THẤY ({})",
                                        msg_hits.len()
                                    ))
                                    .size(11.0)
                                    .color(pal.primary_hover)
                                    .strong(),
                                );
                            });
                            ui.add_space(4.0);
                            for (t_idx, t_name, m_text, m_time, t_group) in &msg_hits {
                                let row_width = ui.available_width() - 12.0;
                                let (r_rect, r_resp) = ui.allocate_exact_size(
                                    Vec2::new(row_width, 52.0),
                                    Sense::click(),
                                );
                                let r_rect = r_rect.translate(Vec2::new(6.0, 0.0));
                                if r_resp.hovered() {
                                    ui.painter().rect_filled(
                                        r_rect,
                                        Rounding::same(10.0),
                                        pal.bg_hover,
                                    );
                                }
                                let av_c = Pos2::new(r_rect.left() + 24.0, r_rect.center().y);
                                let hit_ava =
                                    self.threads.get(*t_idx).and_then(|th| th.avatar.as_deref());
                                paint_user_avatar(
                                    ui.painter(),
                                    av_c,
                                    16.0,
                                    t_name,
                                    *t_group,
                                    hit_ava,
                                    &self.sticker_textures,
                                    &mut self.loading_stickers,
                                );
                                let tl = r_rect.left() + 50.0;
                                ui.painter().text(
                                    Pos2::new(tl, r_rect.top() + 9.0),
                                    egui::Align2::LEFT_TOP,
                                    truncate_chars(t_name, 20),
                                    FontId::proportional(12.8),
                                    pal.text_primary,
                                );
                                if !m_time.is_empty() {
                                    ui.painter().text(
                                        Pos2::new(r_rect.right() - 10.0, r_rect.top() + 10.0),
                                        egui::Align2::RIGHT_TOP,
                                        m_time,
                                        FontId::proportional(10.5),
                                        pal.text_muted,
                                    );
                                }
                                ui.painter().text(
                                    Pos2::new(tl, r_rect.top() + 28.0),
                                    egui::Align2::LEFT_TOP,
                                    truncate_chars(m_text, 30),
                                    FontId::proportional(11.8),
                                    pal.primary_hover,
                                );
                                if r_resp
                                    .on_hover_cursor(CursorIcon::PointingHand)
                                    .on_hover_text(m_text)
                                    .clicked()
                                {
                                    clicked_msg_hit = Some((*t_idx, query.clone()));
                                }
                            }
                        }

                        if !query.is_empty() && filtered_indices.is_empty() && msg_hits.is_empty() {
                            ui.vertical_centered(|ui| {
                                ui.add_space(32.0);
                                ui.label(
                                    RichText::new(
                                        "Không tìm thấy hội thoại hoặc tin nhắn phù hợp.",
                                    )
                                    .size(12.5)
                                    .color(pal.text_muted),
                                );
                            });
                        }
                    });

                if let Some((idx, q_str)) = clicked_msg_hit {
                    self.workspace_select_conversation(ctx, idx);
                    self.in_chat_search_open = true;
                    self.in_chat_search_focus_pending = true;
                    self.in_chat_search_query = q_str;
                    if self.threads.get(idx).is_some_and(|thread| {
                        !thread.is_group && !thread.name.contains("Cloud của tôi")
                    }) {
                        if let Some(tx) = WORKER_TX.get() {
                            let _ = tx.send(WorkerCmd::SyncDirectHistory);
                        }
                    }
                }
                if let Some((idx, tid, is_group)) = clicked_thread {
                    if self.selected_thread_index != idx {
                        self.filter_starred_only = false;
                    }
                    self.workspace_select_conversation(ctx, idx);
                    let is_cloud = self
                        .threads
                        .get(idx)
                        .map(|t| t.name.contains("Cloud của tôi"))
                        .unwrap_or(false);
                    if is_group || is_cloud {
                        if let Some(tx) = WORKER_TX.get() {
                            let _ = tx.send(WorkerCmd::LoadGroupHistory {
                                group_id: tid,
                                is_group,
                            });
                        }
                    } else if let Some(tx) = WORKER_TX.get() {
                        let _ = tx.send(WorkerCmd::SyncDirectHistory);
                    }
                }
            });

        // Extract lightweight metadata for Header, Right Info Drawer, and Central Chat Stream
        let thread_meta = self.threads.get(self.selected_thread_index).map(|t| {
            (
                t.id.clone(),
                t.name.clone(),
                t.avatar.clone(),
                t.is_group,
                t.messages.len(),
            )
        });

        // -------------------------------------------------------------------
        // 3. COLLAPSIBLE RIGHT CONVERSATION INFO DRAWER (responsive width)
        // -------------------------------------------------------------------
        let show_info_drawer = self.info_drawer_open && !is_compact_w;
        let drawer_w = (screen_w * 0.26).clamp(240.0, 310.0);

        if show_info_drawer {
            if let Some((ref d_id, ref d_name, ref _d_ava, d_is_group, d_msg_count)) = thread_meta {
                let shared_photos: Vec<(String, String, String, String)> = self
                    .threads
                    .get(self.selected_thread_index)
                    .map(|t| {
                        t.messages
                            .iter()
                            .rev()
                            .filter_map(|m| {
                                let is_photo = m.link_src.as_deref() == Some("Hình ảnh Zalo")
                                    || m.text.trim() == "[Hình ảnh]"
                                    || m.text.trim().starts_with("[Hình ảnh] ");
                                if !is_photo {
                                    return None;
                                }
                                let thumb = m
                                    .link_thumb
                                    .as_deref()
                                    .or(m.link_url.as_deref())
                                    .filter(|u| !u.is_empty())?;
                                let full = m.link_url.as_deref().unwrap_or(thumb).to_string();
                                let title = m
                                    .link_title
                                    .clone()
                                    .unwrap_or_else(|| "Hình ảnh đã chia sẻ".into());
                                Some((
                                    format!("thumb_{}", crate::crypto::md5_hex(thumb)),
                                    thumb.to_string(),
                                    full,
                                    title,
                                ))
                            })
                            .take(9)
                            .collect()
                    })
                    .unwrap_or_default();

                egui::SidePanel::right("conversation_info_drawer")
                    .resizable(true)
                    .default_width(drawer_w)
                    .width_range(220.0..=340.0)
                    .frame(
                        egui::Frame::none()
                            .fill(pal.bg_sidebar)
                            .stroke(Stroke::new(1.0_f32, pal.border)),
                    )
                    .show(ctx, |ui| {
                        // Profile Card Header
                        egui::Frame::none()
                            .inner_margin(Margin::symmetric(16.0, 18.0))
                            .show(ui, |ui| {
                                ui.vertical_centered(|ui| {
                                    let (av_rect, _) = ui
                                        .allocate_exact_size(Vec2::new(64.0, 64.0), Sense::hover());
                                    if let Some(thread) =
                                        self.threads.get(self.selected_thread_index)
                                    {
                                        paint_sidebar_avatar(
                                            ui.painter(),
                                            av_rect.center(),
                                            32.0,
                                            thread,
                                            &self.sticker_textures,
                                            &mut self.loading_stickers,
                                            pal,
                                        );
                                    }
                                    ui.add_space(10.0);
                                    ui.label(
                                        RichText::new(d_name)
                                            .size(14.5)
                                            .color(pal.text_primary)
                                            .strong(),
                                    );
                                    ui.add_space(2.0);
                                    let d_is_cloud = d_name.contains("Cloud của tôi");
                                    let d_fr_status = if d_is_group || d_is_cloud {
                                        crate::chat::FriendshipStatus::Friend
                                    } else {
                                        self.friend_status_map
                                            .get(d_id)
                                            .copied()
                                            .unwrap_or(crate::chat::FriendshipStatus::Unknown)
                                    };
                                    let presence_now = crate::login::unix_now() * 1000;
                                    let direct_presence =
                                        self.presence_map.get(d_id).cloned().unwrap_or_default();
                                    let friend_presence_label = format!(
                                        "✓ Đã kết bạn · {}",
                                        direct_presence.label(presence_now)
                                    );
                                    let (sub_txt, sub_col) = if d_is_group {
                                        ("Nhóm đang hoạt động", pal.accent_online)
                                    } else if d_is_cloud {
                                        ("Lưu trữ đám mây cá nhân", pal.primary_hover)
                                    } else {
                                        match d_fr_status {
                                            crate::chat::FriendshipStatus::Friend => (
                                                friend_presence_label.as_str(),
                                                if direct_presence.is_recent(presence_now) {
                                                    pal.accent_online
                                                } else {
                                                    pal.text_secondary
                                                },
                                            ),
                                            crate::chat::FriendshipStatus::IncomingPending => (
                                                "📩 Đã gửi lời mời kết bạn cho bạn",
                                                pal.primary_hover,
                                            ),
                                            crate::chat::FriendshipStatus::OutgoingPending => {
                                                ("⏳ Đã gửi lời mời kết bạn", pal.warning)
                                            }
                                            crate::chat::FriendshipStatus::Stranger => (
                                                "Người lạ · Chưa kết bạn",
                                                Color32::from_rgb(249, 115, 22),
                                            ),
                                            crate::chat::FriendshipStatus::Unknown => {
                                                ("Đang kiểm tra kết bạn…", pal.text_muted)
                                            }
                                        }
                                    };
                                    ui.label(RichText::new(sub_txt).size(12.0).color(sub_col));

                                    ui.add_space(14.0);
                                    // Quick Action Buttons (Sync & Pin/Star Filter)
                                    ui.horizontal(|ui| {
                                        ui.add_space(34.0);
                                        let sync_btn = egui::Button::new(
                                            RichText::new("Đồng bộ")
                                                .size(11.5)
                                                .color(pal.text_secondary),
                                        )
                                        .fill(pal.bg_elevated)
                                        .stroke(Stroke::new(1.0_f32, pal.border))
                                        .rounding(Rounding::same(8.0))
                                        .min_size(Vec2::new(92.0, 30.0));
                                        if ui
                                            .add(sync_btn)
                                            .on_hover_cursor(CursorIcon::PointingHand)
                                            .clicked()
                                        {
                                            if let Some(tx) = WORKER_TX.get() {
                                                let _ = tx.send(WorkerCmd::RefreshThreads);
                                                if d_is_group || d_name.contains("Cloud của tôi")
                                                {
                                                    let _ = tx.send(WorkerCmd::LoadGroupHistory {
                                                        group_id: d_id.clone(),
                                                        is_group: d_is_group,
                                                    });
                                                } else {
                                                    let _ = tx.send(WorkerCmd::SyncDirectHistory);
                                                }
                                            }
                                            self.show_toast("Đang đồng bộ lịch sử hội thoại…");
                                        }

                                        let star_active = self.filter_starred_only;
                                        let star_btn =
                                            egui::Button::new(
                                                RichText::new(if star_active {
                                                    "Hiện tất cả"
                                                } else {
                                                    "Tin đánh dấu"
                                                })
                                                .size(11.5)
                                                .color(if star_active {
                                                    Color32::WHITE
                                                } else {
                                                    pal.text_secondary
                                                }),
                                            )
                                            .fill(if star_active {
                                                pal.primary
                                            } else {
                                                pal.bg_elevated
                                            })
                                            .stroke(Stroke::new(1.0_f32, pal.border))
                                            .rounding(Rounding::same(8.0))
                                            .min_size(Vec2::new(92.0, 30.0));
                                        if ui
                                            .add(star_btn)
                                            .on_hover_cursor(CursorIcon::PointingHand)
                                            .clicked()
                                        {
                                            self.filter_starred_only = !self.filter_starred_only;
                                        }
                                    });
                                });
                            });

                        ui.add(egui::Separator::default().spacing(1.0));

                        // Shared Media Grid (`Ảnh / Video đã chia sẻ`)
                        egui::Frame::none()
                            .inner_margin(Margin::symmetric(16.0, 14.0))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.label(
                                        RichText::new("Ảnh / Video đã chia sẻ")
                                            .size(12.5)
                                            .color(pal.text_primary)
                                            .strong(),
                                    );
                                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                        ui.label(
                                            RichText::new(format!("({})", shared_photos.len()))
                                                .size(11.5)
                                                .color(pal.primary_hover),
                                        );
                                    });
                                });
                                ui.add_space(8.0);

                                if shared_photos.is_empty() {
                                    ui.label(
                                        RichText::new("Chưa có ảnh hoặc video chia sẻ.")
                                            .size(11.5)
                                            .color(pal.text_muted),
                                    );
                                } else {
                                    ui.horizontal_wrapped(|ui| {
                                        ui.spacing_mut().item_spacing = Vec2::new(6.0, 6.0);
                                        for (tkey, thumb_u, full_u, ptitle) in &shared_photos {
                                            if !self.sticker_textures.contains_key(tkey)
                                                && self.loading_stickers.insert(tkey.clone())
                                            {
                                                if let Some(tx) = WORKER_TX.get() {
                                                    let _ = tx.send(WorkerCmd::FetchStickerImage {
                                                        key: tkey.clone(),
                                                        url: thumb_u.clone(),
                                                    });
                                                }
                                            }
                                            let (cell_r, cell_resp) = ui.allocate_exact_size(
                                                Vec2::splat(74.0),
                                                Sense::click(),
                                            );
                                            ui.painter().rect_filled(
                                                cell_r,
                                                Rounding::same(8.0),
                                                pal.bg_elevated,
                                            );
                                            if let Some(tex) = self.sticker_textures.get(tkey) {
                                                ui.painter().image(
                                                    tex.id(),
                                                    cell_r,
                                                    Rect::from_min_max(
                                                        Pos2::ZERO,
                                                        Pos2::new(1.0, 1.0),
                                                    ),
                                                    Color32::WHITE,
                                                );
                                            } else {
                                                draw_icon_image(
                                                    ui.painter(),
                                                    cell_r.center(),
                                                    20.0,
                                                    pal.text_muted,
                                                );
                                            }
                                            ui.painter().rect_stroke(
                                                cell_r,
                                                Rounding::same(8.0),
                                                Stroke::new(
                                                    1.0_f32,
                                                    if cell_resp.hovered() {
                                                        pal.primary
                                                    } else {
                                                        pal.border
                                                    },
                                                ),
                                            );
                                            if cell_resp
                                                .on_hover_cursor(CursorIcon::PointingHand)
                                                .on_hover_text("Xem ảnh phóng to")
                                                .clicked()
                                            {
                                                self.image_viewer = Some(ImageViewerModal {
                                                    tex_key: tkey.clone(),
                                                    hd_tex_key: self.request_hd_texture(full_u),
                                                    full_url: full_u.clone(),
                                                    title: ptitle.clone(),
                                                    sender: d_name.clone(),
                                                    time: String::new(),
                                                    zoom: 1.0,
                                                    pan: Vec2::ZERO,
                                                });
                                            }
                                        }
                                    });
                                }
                            });

                        // Push technical metadata to the bottom of the drawer
                        let rem = (ui.available_height() - 88.0).max(8.0);
                        ui.add_space(rem);
                        ui.add(egui::Separator::default().spacing(1.0));

                        egui::Frame::none()
                            .fill(pal.bg_app)
                            .inner_margin(Margin::symmetric(16.0, 12.0))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.label(
                                        RichText::new("Tổng số tin nhắn:")
                                            .size(11.5)
                                            .color(pal.text_muted),
                                    );
                                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                        ui.label(
                                            RichText::new(format!("{d_msg_count} tin nhắn"))
                                                .size(11.5)
                                                .color(pal.text_secondary)
                                                .strong(),
                                        );
                                    });
                                });
                                ui.add_space(4.0);
                                ui.horizontal(|ui| {
                                    ui.label(
                                        RichText::new("Mã định danh (ID):")
                                            .size(11.5)
                                            .color(pal.text_muted),
                                    );
                                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                        let char_count = d_id.chars().count();
                                        let short_id = if char_count > 10 {
                                            let prefix: String = d_id.chars().take(4).collect();
                                            let suffix: String =
                                                d_id.chars().skip(char_count - 4).collect();
                                            format!("{prefix}...{suffix}")
                                        } else {
                                            d_id.clone()
                                        };
                                        let (id_rect, id_resp) = ui.allocate_exact_size(
                                            Vec2::new(98.0, 22.0),
                                            Sense::click(),
                                        );
                                        ui.painter().rect_filled(
                                            id_rect,
                                            Rounding::same(5.0),
                                            if id_resp.hovered() {
                                                pal.bg_hover
                                            } else {
                                                pal.bg_elevated
                                            },
                                        );
                                        ui.painter().text(
                                            Pos2::new(id_rect.left() + 7.0, id_rect.center().y),
                                            egui::Align2::LEFT_CENTER,
                                            short_id,
                                            FontId::monospace(10.5),
                                            pal.text_secondary,
                                        );
                                        draw_icon_copy(
                                            ui.painter(),
                                            Pos2::new(id_rect.right() - 10.0, id_rect.center().y),
                                            11.5,
                                            pal.text_secondary,
                                        );
                                        if id_resp
                                            .on_hover_cursor(CursorIcon::PointingHand)
                                            .on_hover_text(format!("Sao chép ID: {d_id}"))
                                            .clicked()
                                        {
                                            ui.ctx().copy_text(d_id.clone());
                                            self.show_toast(
                                                "Đã sao chép ID hội thoại vào bộ nhớ tạm",
                                            );
                                        }
                                    });
                                });
                            });
                    });
            }
        }

        // -------------------------------------------------------------------
        // 4. CENTRAL CHAT AREA (Clean Header, Messages Stream, Icon Toolbar)
        // -------------------------------------------------------------------
        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(pal.bg_chat_canvas))
            .show(ctx, |ui| {
                for error in self.persistence_errors.values() {
                    egui::Frame::none().inner_margin(Margin::same(10.0)).show(ui, |ui| {
                        ui.label(RichText::new(error).color(Color32::from_rgb(210, 70, 50)));
                    });
                }
                let show_direct_history = thread_meta.as_ref().map(|(_, name, _, is_group, _)| {
                    !is_group && !name.contains("Cloud của tôi")
                }).unwrap_or(true);
                if thread_meta.as_ref().is_some_and(|(_, _, _, is_group, _)| *is_group) {
                    egui::Frame::none().inner_margin(Margin::same(10.0)).show(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(RichText::new(&self.group_history_status).size(12.0).color(pal.text_secondary));
                            if ui.add_enabled(!self.phone_history_active, egui::Button::new("Đồng bộ 1-1 và nhóm từ điện thoại")).clicked() {
                                if let Some(tx) = WORKER_TX.get() {
                                    self.phone_history_generation = self.phone_history_generation.wrapping_add(1);
                                    self.phone_history_active = true;
                                    self.phone_history_status = "Đang chuẩn bị đồng bộ 1-1 và nhóm từ điện thoại".into();
                                    let _ = tx.send(WorkerCmd::SyncPhoneHistory { generation: self.phone_history_generation });
                                }
                            }
                            if self.phone_history_active && ui.button("Gửi thông báo tới điện thoại").clicked() {
                                if let Some(tx) = WORKER_TX.get() {
                                    let _ = tx.send(WorkerCmd::WakeupPhoneHistory { generation: self.phone_history_generation });
                                }
                            }
                            if self.phone_history_active && ui.button("Dừng đồng bộ").clicked() {
                                if let Some(tx) = WORKER_TX.get() {
                                    let _ = tx.send(WorkerCmd::CancelPhoneHistory { generation: self.phone_history_generation });
                                }
                            }
                            ui.label(RichText::new(&self.phone_history_status).size(12.0).color(pal.text_secondary));
                            if ui.button("Tải thêm lịch sử nhóm").clicked() {
                                if let Some(tx) = WORKER_TX.get() {
                                    let _ = tx.send(WorkerCmd::SyncGroupHistory);
                                    if let Some((group_id, _, _, _, _)) = thread_meta.as_ref() {
                                        let _ = tx.send(WorkerCmd::LoadGroupHistory { group_id: group_id.clone(), is_group: true });
                                    }
                                }
                            }
                        });
                    });
                }
                if show_direct_history {
                    egui::Frame::none().inner_margin(Margin::same(10.0)).show(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(RichText::new(&self.direct_history_status).size(12.0).color(pal.text_secondary));
                            if ui.add_enabled(!self.phone_history_active, egui::Button::new("Đồng bộ 1-1 và nhóm từ điện thoại")).clicked() {
                                if let Some(tx) = WORKER_TX.get() {
                                    self.phone_history_generation = self.phone_history_generation.wrapping_add(1);
                                    self.phone_history_active = true;
                                    self.phone_history_status = "Đang chuẩn bị đồng bộ từ điện thoại".into();
                                    let _ = tx.send(WorkerCmd::SyncPhoneHistory { generation: self.phone_history_generation });
                                }
                            }
                            if self.phone_history_active && ui.button("Gửi thông báo tới điện thoại").clicked() {
                                if let Some(tx) = WORKER_TX.get() {
                                    let _ = tx.send(WorkerCmd::WakeupPhoneHistory { generation: self.phone_history_generation });
                                }
                            }
                            if self.phone_history_active && ui.button("Dừng đồng bộ").clicked() {
                                if let Some(tx) = WORKER_TX.get() {
                                    let _ = tx.send(WorkerCmd::CancelPhoneHistory { generation: self.phone_history_generation });
                                }
                            }
                            ui.label(RichText::new(&self.phone_history_status).size(12.0).color(pal.text_secondary));
                            if ui.button("Tải thêm lịch sử 1-1").clicked() {
                                if let Some(tx) = WORKER_TX.get() {
                                    let _ = tx.send(WorkerCmd::SyncDirectHistory);
                                }
                            }
                        });
                    });
                }
                if let Some((thread_id, thread_name, thread_avatar, is_group, _msg_count)) =
                    thread_meta
                {
                    let is_cloud_thread = thread_name.contains("Cloud của tôi");
                    if !is_group
                        && !is_cloud_thread
                        && !self.friend_status_map.contains_key(&thread_id)
                        && self.checking_friend_status.insert(thread_id.clone())
                    {
                        if let Some(tx) = WORKER_TX.get() {
                            let _ = tx.send(WorkerCmd::CheckFriendStatus {
                                user_id: thread_id.clone(),
                            });
                        }
                    }
                    let now = std::time::Instant::now();
                    if !is_group && !is_cloud_thread && self.presence_requested_at.get(&thread_id).is_none_or(|last| now.duration_since(*last) >= Duration::from_secs(60)) {
                        if let Some(tx) = WORKER_TX.get() {
                            if tx.send(WorkerCmd::CheckPresence { user_id: thread_id.clone() }).is_ok() {
                                self.presence_requested_at.insert(thread_id.clone(), now);
                            }
                        }
                    }
                    ctx.request_repaint_after(Duration::from_secs(60));
                    let presence = self.presence_map.get(&thread_id).cloned().unwrap_or_default();
                    let now_ms = crate::login::unix_now()*1000;
                    let cur_friendship = if is_group || is_cloud_thread {
                        crate::chat::FriendshipStatus::Friend
                    } else {
                        self.friend_status_map
                            .get(&thread_id)
                            .copied()
                            .unwrap_or(crate::chat::FriendshipStatus::Unknown)
                    };
                    // 1. Clean Conversation Header Bar (No raw 19-digit ID, no bulky sync button)
                    egui::Frame::none()
                        .fill(pal.bg_sidebar)
                        .inner_margin(Margin::symmetric(18.0, 11.0))
                        .stroke(Stroke::new(1.0_f32, pal.border))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                // Header Avatar (40x40) with Cutout Online Dot
                                let (av_rect, _) =
                                    ui.allocate_exact_size(Vec2::new(40.0, 40.0), Sense::hover());
                                if let Some(thread)=self.threads.get(self.selected_thread_index){paint_sidebar_avatar(ui.painter(),av_rect.center(),20.0,thread,&self.sticker_textures,&mut self.loading_stickers,pal);}
                                if !is_group {
                                    let dot_pos=Pos2::new(av_rect.right()-4.0,av_rect.bottom()-4.0);
                                    ui.painter().circle_filled(dot_pos,6.2,pal.bg_sidebar);
                                    ui.painter().circle_filled(dot_pos,4.2,if presence.is_recent(now_ms){pal.accent_online}else{pal.text_muted});
                                }

                                ui.add_space(6.0);
                                // Reserve room for the right-hand action cluster so the name/tag
                                // block can never be overlapped by the call buttons.
                                let actions_w = if is_compact_w { 126.0 } else { 182.0 };
                                let left_info_w =
                                    (ui.available_width() - actions_w).max(60.0);
                                ui.allocate_ui_with_layout(
                                    Vec2::new(left_info_w, 40.0),
                                    Layout::top_down(Align::Min),
                                    |ui| {
                                        ui.spacing_mut().item_spacing.y = 2.0;
                                        ui.horizontal(|ui| {
                                            let name_max = if is_compact_w { 12 } else { 22 };
                                            ui.add(
                                                egui::Label::new(
                                                    RichText::new(truncate_chars(
                                                        &thread_name,
                                                        name_max,
                                                    ))
                                                    .size(15.0)
                                                    .color(pal.text_primary)
                                                    .strong(),
                                                )
                                                .truncate(),
                                            );
                                            if !is_compact_w && left_info_w > 170.0 {
                                                let (tag_lbl, tag_fg, tag_bg) =
                                                    if is_cloud_thread {
                                                        (
                                                            "Đám mây",
                                                            pal.primary_hover,
                                                            pal.primary.linear_multiply(0.14),
                                                        )
                                                    } else if is_group {
                                                        ("Nhóm", pal.text_secondary, pal.bg_elevated)
                                                    } else {
                                                        match cur_friendship {
                                                            crate::chat::FriendshipStatus::Friend => (
                                                                "✓ Bạn bè",
                                                                pal.accent_online,
                                                                pal.accent_online.linear_multiply(0.14),
                                                            ),
                                                            crate::chat::FriendshipStatus::IncomingPending => (
                                                                "Chờ đồng ý kết bạn",
                                                                pal.primary_hover,
                                                                pal.primary.linear_multiply(0.16),
                                                            ),
                                                            crate::chat::FriendshipStatus::OutgoingPending => (
                                                                "Đã gửi lời mời",
                                                                pal.warning,
                                                                pal.warning.linear_multiply(0.15),
                                                            ),
                                                            crate::chat::FriendshipStatus::Stranger => (
                                                                "Người lạ",
                                                                Color32::from_rgb(249, 115, 22),
                                                                Color32::from_rgb(249, 115, 22)
                                                                    .linear_multiply(0.15),
                                                            ),
                                                            crate::chat::FriendshipStatus::Unknown => (
                                                                "Đang kiểm tra…",
                                                                pal.text_muted,
                                                                pal.bg_elevated,
                                                            ),
                                                        }
                                                    };
                                                egui::Frame::none()
                                                    .fill(tag_bg)
                                                    .rounding(Rounding::same(4.0))
                                                    .inner_margin(Margin::symmetric(6.0, 1.5))
                                                    .show(ui, |ui| {
                                                        ui.label(
                                                            RichText::new(tag_lbl)
                                                                .size(10.2)
                                                                .color(tag_fg)
                                                                .strong(),
                                                        );
                                                    });
                                            }
                                        });
                                        let (sub_status_lbl, sub_status_col) = if is_group {
                                            ("Nhóm trò chuyện".to_string(), pal.text_secondary)
                                        } else if is_cloud_thread {
                                            ("Lưu trữ đám mây cá nhân".to_string(), pal.text_secondary)
                                        } else {
                                            (presence.label(now_ms), if presence.is_recent(now_ms) { pal.accent_online } else { pal.text_secondary })
                                        };
                                        ui.label(
                                            RichText::new(sub_status_lbl)
                                                .size(11.6)
                                                .color(sub_status_col),
                                        );
                                    },
                                );

                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    // Info Drawer Toggle Button (hidden on compact widths: auto-collapsed)
                                    if !is_compact_w {
                                        let (drw_rect, drw_resp) = ui.allocate_exact_size(
                                            Vec2::new(34.0, 34.0),
                                            Sense::click(),
                                        );
                                        ui.painter().rect_filled(
                                            drw_rect,
                                            Rounding::same(8.0),
                                            if self.info_drawer_open {
                                                pal.primary.linear_multiply(0.18)
                                            } else if drw_resp.hovered() {
                                                pal.bg_hover
                                            } else {
                                                Color32::TRANSPARENT
                                            },
                                        );
                                        draw_icon_drawer(
                                            ui.painter(),
                                            drw_rect.center(),
                                            17.0,
                                            if self.info_drawer_open {
                                                pal.primary_hover
                                            } else if drw_resp.hovered() {
                                                pal.text_primary
                                            } else {
                                                pal.text_secondary
                                            },
                                        );
                                        if drw_resp
                                            .on_hover_cursor(CursorIcon::PointingHand)
                                            .on_hover_text("Thông tin hội thoại")
                                            .clicked()
                                        {
                                            self.info_drawer_open = !self.info_drawer_open;
                                        }
                                    }

                                    // Subtle vertical divider
                                    let (div_r, _) = ui.allocate_exact_size(
                                        Vec2::new(8.0, 20.0),
                                        Sense::hover(),
                                    );
                                    ui.painter().line_segment(
                                        [div_r.center_top(), div_r.center_bottom()],
                                        Stroke::new(1.0_f32, pal.border),
                                    );

                                    // Video Call Button
                                    let (vid_rect, vid_resp) = ui.allocate_exact_size(
                                        Vec2::new(34.0, 34.0),
                                        Sense::click(),
                                    );
                                    if vid_resp.hovered() {
                                        ui.painter().rect_filled(
                                            vid_rect,
                                            Rounding::same(8.0),
                                            pal.bg_hover,
                                        );
                                    }
                                    draw_icon_video(
                                        ui.painter(),
                                        vid_rect.center(),
                                        17.5,
                                        if vid_resp.hovered() {
                                            pal.text_primary
                                        } else {
                                            pal.text_secondary
                                        },
                                    );
                                    if vid_resp
                                        .on_hover_cursor(CursorIcon::PointingHand)
                                        .on_hover_text("Gọi Video")
                                        .clicked()
                                    {
                                        self.active_call = Some(ActiveCallOverlay {
                                            partner_id: thread_id.clone(),
                                            partner_name: thread_name.clone(),
                                            is_video: true,
                                            is_group,
                                            state_label: "Đang kết nối cuộc gọi video…".into(),
                                            started_at: std::time::Instant::now(),
                                            mic_muted: false,
                                            cam_enabled: true,
                                            is_incoming_ringing: false,
                                            error_code: None,
                                            error_reason: None,
                                        });
                                        if let Some(tx) = WORKER_TX.get() {
                                            let _ = tx.send(WorkerCmd::StartCall {
                                                partner_id: thread_id.clone(),
                                                partner_name: thread_name.clone(),
                                                partner_avatar: None,
                                                is_video: true,
                                                is_group,
                                            });
                                        }
                                    }

                                    // Voice Call Button
                                    let (aud_rect, aud_resp) = ui.allocate_exact_size(
                                        Vec2::new(34.0, 34.0),
                                        Sense::click(),
                                    );
                                    if aud_resp.hovered() {
                                        ui.painter().rect_filled(
                                            aud_rect,
                                            Rounding::same(8.0),
                                            pal.bg_hover,
                                        );
                                    }
                                    draw_icon_phone(
                                        ui.painter(),
                                        aud_rect.center(),
                                        17.5,
                                        if aud_resp.hovered() {
                                            pal.text_primary
                                        } else {
                                            pal.text_secondary
                                        },
                                    );
                                    if aud_resp
                                        .on_hover_cursor(CursorIcon::PointingHand)
                                        .on_hover_text("Gọi thoại")
                                        .clicked()
                                    {
                                        self.active_call = Some(ActiveCallOverlay {
                                            partner_id: thread_id.clone(),
                                            partner_name: thread_name.clone(),
                                            is_video: false,
                                            is_group,
                                            state_label: "Đang kết nối cuộc gọi thoại…".into(),
                                            started_at: std::time::Instant::now(),
                                            mic_muted: false,
                                            cam_enabled: false,
                                            is_incoming_ringing: false,
                                            error_code: None,
                                            error_reason: None,
                                        });
                                        if let Some(tx) = WORKER_TX.get() {
                                            let _ = tx.send(WorkerCmd::StartCall {
                                                partner_id: thread_id.clone(),
                                                partner_name: thread_name.clone(),
                                                partner_avatar: None,
                                                is_video: false,
                                                is_group,
                                            });
                                        }
                                    }
                                    // Search Messages in Conversation Button (`Ctrl+F`)
                                    let (srch_rect, srch_resp) = ui.allocate_exact_size(
                                        Vec2::new(34.0, 34.0),
                                        Sense::click(),
                                    );
                                    ui.painter().rect_filled(
                                        srch_rect,
                                        Rounding::same(8.0),
                                        if self.in_chat_search_open {
                                            pal.primary.linear_multiply(0.18)
                                        } else if srch_resp.hovered() {
                                            pal.bg_hover
                                        } else {
                                            Color32::TRANSPARENT
                                        },
                                    );
                                    draw_icon_search(
                                        ui.painter(),
                                        srch_rect.center(),
                                        16.5,
                                        if self.in_chat_search_open {
                                            pal.primary_hover
                                        } else if srch_resp.hovered() {
                                            pal.text_primary
                                        } else {
                                            pal.text_secondary
                                        },
                                    );
                                    if srch_resp
                                        .on_hover_cursor(CursorIcon::PointingHand)
                                        .on_hover_text("Tìm kiếm tin nhắn trong hội thoại này (Ctrl+F)")
                                        .clicked()
                                        || ui.input(|i| {
                                            (i.modifiers.ctrl || i.modifiers.command)
                                                && i.key_pressed(Key::F)
                                        })
                                    {
                                        self.in_chat_search_open = !self.in_chat_search_open;
                                        if self.in_chat_search_open {
                                            self.in_chat_search_focus_pending = true;
                                        } else {
                                            self.in_chat_search_query.clear();
                                        }
                                    }
                                });
                            });
                        });
                    // 1a. Stranger / Pending Friend Request Banner (matching Zalo PC's top relationship bar)
                    if !is_group
                        && !is_cloud_thread
                        && matches!(
                            cur_friendship,
                            crate::chat::FriendshipStatus::Stranger
                                | crate::chat::FriendshipStatus::IncomingPending
                                | crate::chat::FriendshipStatus::OutgoingPending
                        )
                    {
                        egui::Frame::none()
                            .fill(pal.bg_elevated)
                            .stroke(Stroke::new(1.0_f32, pal.border))
                            .inner_margin(Margin::symmetric(18.0, 7.0))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    match cur_friendship {
                                        crate::chat::FriendshipStatus::IncomingPending => {
                                            ui.label(
                                                RichText::new(format!(
                                                    "{}  {} đã gửi lời mời kết bạn cho bạn",
                                                    lucide::USER_PLUS,
                                                    truncate_chars(&thread_name, 26)
                                                ))
                                                .size(12.2)
                                                .color(pal.primary_hover)
                                                .strong(),
                                            );
                                            ui.with_layout(
                                                Layout::right_to_left(Align::Center),
                                                |ui| {
                                                    ui.spacing_mut().item_spacing.x = 6.0;
                                                    let rej_btn = egui::Button::new(
                                                        RichText::new("Từ chối")
                                                            .size(11.5)
                                                            .color(pal.text_secondary),
                                                    )
                                                    .fill(pal.bg_input)
                                                    .stroke(Stroke::new(1.0_f32, pal.border))
                                                    .rounding(Rounding::same(6.0))
                                                    .min_size(Vec2::new(68.0, 26.0));
                                                    if ui
                                                        .add(rej_btn)
                                                        .on_hover_cursor(CursorIcon::PointingHand)
                                                        .clicked()
                                                    {
                                                        if let Some(tx) = WORKER_TX.get() {
                                                            let _ = tx.send(
                                                                WorkerCmd::RejectFriend {
                                                                    user_id: thread_id.clone(),
                                                                    user_name: thread_name.clone(),
                                                                },
                                                            );
                                                        }
                                                    }

                                                    let acc_btn = egui::Button::new(
                                                        RichText::new(format!(
                                                            "{} Đồng ý kết bạn",
                                                            lucide::CHECK
                                                        ))
                                                        .size(11.8)
                                                        .color(Color32::WHITE)
                                                        .strong(),
                                                    )
                                                    .fill(pal.accent_online)
                                                    .rounding(Rounding::same(6.0))
                                                    .min_size(Vec2::new(124.0, 26.0));
                                                    if ui
                                                        .add(acc_btn)
                                                        .on_hover_cursor(CursorIcon::PointingHand)
                                                        .clicked()
                                                    {
                                                        if let Some(tx) = WORKER_TX.get() {
                                                            let _ = tx.send(
                                                                WorkerCmd::AcceptFriend {
                                                                    user_id: thread_id.clone(),
                                                                    user_name: thread_name.clone(),
                                                                },
                                                            );
                                                        }
                                                    }
                                                },
                                            );
                                        }
                                        crate::chat::FriendshipStatus::OutgoingPending => {
                                            ui.label(
                                                RichText::new(format!(
                                                    "Đã gửi lời mời kết bạn tới {} · Đang chờ đồng ý",
                                                    truncate_chars(&thread_name, 24)
                                                ))
                                                .size(12.0)
                                                .color(pal.warning),
                                            );
                                            ui.with_layout(
                                                Layout::right_to_left(Align::Center),
                                                |ui| {
                                                    let undo_btn = egui::Button::new(
                                                        RichText::new("Hủy lời mời")
                                                            .size(11.5)
                                                            .color(pal.text_secondary),
                                                    )
                                                    .fill(pal.bg_input)
                                                    .stroke(Stroke::new(1.0_f32, pal.border))
                                                    .rounding(Rounding::same(6.0))
                                                    .min_size(Vec2::new(86.0, 26.0));
                                                    if ui
                                                        .add(undo_btn)
                                                        .on_hover_cursor(CursorIcon::PointingHand)
                                                        .clicked()
                                                    {
                                                        if let Some(tx) = WORKER_TX.get() {
                                                            let _ = tx.send(WorkerCmd::UndoFriend {
                                                                user_id: thread_id.clone(),
                                                                user_name: thread_name.clone(),
                                                            });
                                                        }
                                                    }
                                                },
                                            );
                                        }
                                        crate::chat::FriendshipStatus::Stranger => {
                                            ui.label(
                                                RichText::new(format!(
                                                    "Bạn và {} chưa là bạn bè trên Zalo",
                                                    truncate_chars(&thread_name, 26)
                                                ))
                                                .size(12.0)
                                                .color(pal.text_secondary),
                                            );
                                            ui.with_layout(
                                                Layout::right_to_left(Align::Center),
                                                |ui| {
                                                    let add_btn = egui::Button::new(
                                                        RichText::new(format!(
                                                            "{} Kết bạn",
                                                            lucide::USER_PLUS
                                                        ))
                                                        .size(11.8)
                                                        .color(Color32::WHITE)
                                                        .strong(),
                                                    )
                                                    .fill(pal.primary)
                                                    .rounding(Rounding::same(6.0))
                                                    .min_size(Vec2::new(92.0, 26.0));
                                                    if ui
                                                        .add(add_btn)
                                                        .on_hover_cursor(CursorIcon::PointingHand)
                                                        .clicked()
                                                    {
                                                        if let Some(tx) = WORKER_TX.get() {
                                                            let _ = tx.send(WorkerCmd::AddFriend {
                                                                user_id: thread_id.clone(),
                                                                user_name: thread_name.clone(),
                                                            });
                                                        }
                                                        self.show_toast(format!(
                                                            "Đã gửi lời mời kết bạn tới {}",
                                                            thread_name
                                                        ));
                                                    }
                                                },
                                            );
                                        }
                                        _ => {}
                                    }
                                });
                            });
                    }
                    // 1b. Pinned Message Banner (if any message is pinned in this thread)
                    let pinned_msg_info = self
                        .threads
                        .get(self.selected_thread_index)
                        .and_then(|t| {
                            t.messages
                                .iter()
                                .enumerate()
                                .find(|(_, m)| m.is_pinned)
                                .map(|(idx, m)| {
                                    (
                                        idx,
                                        if m.is_outgoing {
                                            "Tôi".to_string()
                                        } else if !m.sender.is_empty() && m.sender != "0" {
                                            m.sender.clone()
                                        } else {
                                            thread_name.clone()
                                        },
                                        if m.sticker_id.is_some() {
                                            "[Nhãn dán]".to_string()
                                        } else {
                                            truncate_chars(&m.text, 80).into_owned()
                                        },
                                    )
                                })
                        });

                    let mut unpin_idx: Option<usize> = None;
                    if let Some((pidx, psender, ptext)) = pinned_msg_info {
                        egui::Frame::none()
                            .fill(pal.bg_elevated)
                            .stroke(Stroke::new(1.0_f32, pal.primary.linear_multiply(0.4)))
                            .inner_margin(Margin::symmetric(14.0, 7.0))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.label(
                                        RichText::new(format!("{} Tin ghim:", lucide::PIN))
                                            .size(12.5)
                                            .color(pal.primary)
                                            .strong(),
                                    );
                                    ui.label(
                                        RichText::new(format!("{psender}: {ptext}"))
                                            .size(12.5)
                                            .color(pal.text_primary),
                                    );
                                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                        if ui
                                            .small_button(format!("Bỏ ghim {}", lucide::X))
                                            .on_hover_cursor(CursorIcon::PointingHand)
                                            .clicked()
                                        {
                                            unpin_idx = Some(pidx);
                                        }
                                    });
                                });
                            });
                    }
                    if let Some(uidx) = unpin_idx {
                        if let Some(real) = self
                            .threads
                            .get_mut(self.selected_thread_index)
                            .and_then(|t| t.messages.get_mut(uidx))
                        {
                            real.is_pinned = false;
                        }
                        self.persist_threads_async();
                        self.status_msg = "Đã bỏ ghim tin nhắn".into();
                    }

                    // 1c. Starred filter active banner
                    if self.filter_starred_only {
                        egui::Frame::none()
                            .fill(Color32::from_rgb(234, 179, 8).linear_multiply(0.12))
                            .stroke(Stroke::new(1.0_f32, Color32::from_rgb(234, 179, 8)))
                            .inner_margin(Margin::symmetric(14.0, 6.0))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.label(
                                        RichText::new(format!("{} Đang lọc tin nhắn đã đánh dấu sao", lucide::STAR))
                                            .size(12.0)
                                            .color(Color32::from_rgb(234, 179, 8))
                                            .strong(),
                                    );
                                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                        if ui
                                            .small_button(format!("Xem tất cả {}", lucide::X))
                                            .on_hover_cursor(CursorIcon::PointingHand)
                                            .clicked()
                                        {
                                            self.filter_starred_only = false;
                                        }
                                    });
                                });
                            });
                    }

                    // 1d. In-Conversation Message Search Bar (`self.in_chat_search_open`)
                    if self.in_chat_search_open
                        && ui.input(|i| i.key_pressed(Key::Escape)) {
                            self.in_chat_search_open = false;
                            self.in_chat_search_query.clear();
                        }
                    if self.in_chat_search_open {
                        let q_low = self.in_chat_search_query.trim().to_lowercase();
                        let search_text = self.in_chat_search_query.trim().to_owned();
                        self.queue_workspace_search(ctx, &search_text, Some(thread_id.clone()));
                        let match_cnt = if q_low.is_empty() { 0 } else {
                            self.threads.get(self.selected_thread_index).map(|t| t.messages.len()).unwrap_or(0)
                        };
                        egui::Frame::none()
                            .fill(pal.bg_sidebar)
                            .stroke(Stroke::new(1.0_f32, pal.border))
                            .inner_margin(Margin::symmetric(18.0, 8.0))
                            .show(ui, |ui| {
                                let bar_avail_w = ui.available_width();
                                ui.set_width(bar_avail_w);
                                ui.horizontal(|ui| {
                                    ui.spacing_mut().item_spacing.x = 8.0;
                                    let right_reserve = if q_low.is_empty() {
                                        86.0
                                    } else {
                                        174.0
                                    };
                                    let pill_w = (bar_avail_w - right_reserve).max(140.0);
                                    egui::Frame::none()
                                        .fill(pal.bg_input)
                                        .rounding(Rounding::same(8.0))
                                        .stroke(Stroke::new(
                                            1.0_f32,
                                            pal.primary.linear_multiply(0.65),
                                        ))
                                        .inner_margin(Margin::symmetric(10.0, 5.0))
                                        .show(ui, |ui| {
                                            let inner_w = (pill_w - 20.0).max(110.0);
                                            ui.set_width(inner_w);
                                            ui.horizontal(|ui| {
                                                ui.spacing_mut().item_spacing.x = 6.0;
                                                let (ir, _) = ui.allocate_exact_size(
                                                    Vec2::new(15.0, 18.0),
                                                    Sense::hover(),
                                                );
                                                draw_icon_search(
                                                    ui.painter(),
                                                    ir.center(),
                                                    13.5,
                                                    pal.primary_hover,
                                                );
                                                let has_txt =
                                                    !self.in_chat_search_query.is_empty();
                                                let te_w = if has_txt {
                                                    (inner_w - 44.0).max(60.0)
                                                } else {
                                                    (inner_w - 22.0).max(80.0)
                                                };
                                                let te_resp = ui.add(
                                                    egui::TextEdit::singleline(
                                                        &mut self.in_chat_search_query,
                                                    )
                                                    .id(Id::new((
                                                        "in_chat_search_input",
                                                        &thread_id,
                                                    )))
                                                    .frame(false)
                                                    .hint_text(
                                                        "Tìm tin nhắn trong hội thoại này...",
                                                    )
                                                    .desired_width(te_w),
                                                );
                                                if self.in_chat_search_focus_pending {
                                                    te_resp.request_focus();
                                                    self.in_chat_search_focus_pending = false;
                                                }
                                                if has_txt {
                                                    let (clr_r, clr_resp) = ui
                                                        .allocate_exact_size(
                                                            Vec2::splat(16.0),
                                                            Sense::click(),
                                                        );
                                                    if clr_resp.hovered() {
                                                        ui.painter().circle_filled(
                                                            clr_r.center(),
                                                            8.0,
                                                            pal.bg_hover,
                                                        );
                                                    }
                                                    draw_lucide_icon(
                                                        ui.painter(),
                                                        clr_r.center(),
                                                        lucide::X,
                                                        11.5,
                                                        pal.text_muted,
                                                    );
                                                    if clr_resp
                                                        .on_hover_cursor(CursorIcon::PointingHand)
                                                        .on_hover_text("Xóa từ khóa")
                                                        .clicked()
                                                    {
                                                        self.in_chat_search_query.clear();
                                                    }
                                                }
                                            });
                                        });

                                    if !q_low.is_empty() {
                                        let badge_col = if match_cnt > 0 {
                                            pal.accent_online
                                        } else {
                                            pal.danger
                                        };
                                        egui::Frame::none()
                                            .fill(badge_col.linear_multiply(0.14))
                                            .rounding(Rounding::same(6.0))
                                            .inner_margin(Margin::symmetric(8.0, 4.0))
                                            .show(ui, |ui| {
                                                ui.label(
                                                    RichText::new(if match_cnt > 0 {
                                                        format!("{match_cnt} kết quả")
                                                    } else {
                                                        "0 kết quả".to_string()
                                                    })
                                                    .size(11.5)
                                                    .color(badge_col)
                                                    .strong(),
                                                );
                                            });
                                    }

                                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                        let close_btn = egui::Button::new(
                                            RichText::new(format!("{} Đóng", lucide::X))
                                                .size(11.8)
                                                .color(pal.text_secondary)
                                                .strong(),
                                        )
                                        .fill(pal.bg_elevated)
                                        .stroke(Stroke::new(1.0_f32, pal.border))
                                        .rounding(Rounding::same(6.0))
                                        .min_size(Vec2::new(66.0, 28.0));
                                        if ui
                                            .add(close_btn)
                                            .on_hover_cursor(CursorIcon::PointingHand)
                                            .on_hover_text("Đóng thanh tìm kiếm (Esc)")
                                            .clicked()
                                        {
                                            self.in_chat_search_open = false;
                                            self.in_chat_search_query.clear();
                                        }
                                    });
                                });
                            });
                    }

                    // 2. Scrollable Messages Area
                    let contextual_stickers = self.resolve_contextual_sticker_suggestions(ctx);
                    let composer_link_preview = self.resolve_composer_link_preview(ctx);
                    // Do NOT subtract `contextual_stickers` height from `picker_reserve`!
                    // The sticker strip is a true floating overlay (`egui::Area`) above the composer bar
                    // (matching Zalo PC's `PopoverIdentitiesDefine.STICKER_SUGGESTION`), so the message scroll
                    // area and composer input box never shift or lose IME focus while typing.
                    let picker_reserve = (if self.sticker_picker_open { 268.0 } else { 0.0 })
                        + (if self.replying_to.is_some() { 48.0 } else { 0.0 })
                        + (if self.pending_clipboard_image.is_some() {
                            64.0
                        } else {
                            0.0
                        });
                    let sender_avatars = SenderAvatars::new(&self.threads);
                    let mut pending_action: Option<(usize, BubbleAction)> = None;
                    let mut load_older_clicked = false;
                    let mut visible_seen = Vec::new();
                    let jump_target=self.pending_jump.clone().filter(|(tid,_)|tid==&thread_id);
                    let mut jump_applied=false;
                    let msg_area_height =
                        (ui.available_height() - 66.0 - picker_reserve).max(100.0);

                    egui::ScrollArea::vertical()
                        .id_salt(Id::new(("chat_scroll", &thread_id, self.message_window.newer)))
                        .max_height(msg_area_height)
                        .auto_shrink([false, false])
                        .stick_to_bottom(self.message_window.newer==0 && jump_target.is_none())
                        .show(ui, |ui| {
                            egui::Frame::none()
                                .inner_margin(Margin::symmetric(18.0, 12.0))
                                .show(ui, |ui| {
                                    if let Some(thread) =
                                        self.threads.get(self.selected_thread_index)
                                    {
                                        if thread.messages.is_empty() {
                                            ui.vertical_centered(|ui| {
                                                ui.add_space(60.0);
                                                ui.label(
                                                    RichText::new(
                                                        "Chưa có tin nhắn trong cuộc trò chuyện này.",
                                                    )
                                                    .size(14.0)
                                                    .color(pal.text_muted),
                                                );
                                            });
                                        } else {
                                            if !self.in_chat_search_open
                                                && !self.filter_starred_only
                                            {
                                                ui.vertical_centered(|ui| {
                                                    let hist_btn = egui::Button::new(
                                                        RichText::new(format!(
                                                            "{} Tải thêm tin nhắn cũ",
                                                            lucide::REFRESH
                                                        ))
                                                        .size(11.5)
                                                        .color(pal.text_secondary),
                                                    )
                                                    .fill(pal.bg_elevated)
                                                    .stroke(Stroke::new(1.0_f32, pal.border))
                                                    .rounding(Rounding::same(12.0));
                                                    if ui
                                                        .add_enabled(self.local_page_has_more || !ctx.data_mut(|data|data.get_temp::<bool>(Id::new(("workspace-page-before",&thread_id))).unwrap_or(true)),hist_btn)
                                                        .on_hover_cursor(CursorIcon::PointingHand)
                                                        .on_hover_text(
                                                            "Tải trang 200 tin nhắn cũ từ cơ sở dữ liệu local",
                                                        )
                                                        .clicked()
                                                    {
                                                        if let Some(first) = thread.messages.first() {
                                                            if let Some(tx) = WORKER_TX.get() {
                                                                let _ = tx.send(WorkerCmd::LoadLocalPage {request_id:{self.navigation_request=self.navigation_request.wrapping_add(1);self.pending_jump=None;self.navigation_request}, thread_id: thread_id.clone(),
                                                                    cursor: Some(crate::store_v2::HistoryCursor { ts:first.ts, msg_id:first.id.clone() }),
                                                                    direction:crate::store_v2::PageDirection::Before, replace:true });
                                                                load_older_clicked = true;
                                                                self.message_window.newer = self.message_window.newer.saturating_add(200);
                                                                ctx.data_mut(|data|data.insert_temp(Id::new(("workspace-page-before",&thread_id)),true));
                                                            }
                                                        }
                                                    }
                                                    ui.add_space(6.0);
                                                });
                                            }
                                            let only_starred = self.filter_starred_only;
                                            let in_chat_q = if self.in_chat_search_open {
                                                self.in_chat_search_query.trim().to_lowercase()
                                            } else {
                                                String::new()
                                            };
                                            let visible_indices: Vec<usize> = thread.messages.iter().enumerate()
                                                .filter(|(_, m)| !only_starred || m.is_starred)
                                                .map(|(index, _)| index).take(200).collect();
                                            if in_chat_q.is_empty() && !only_starred {
                                                ui.horizontal_wrapped(|ui| {
                                                    ui.label(format!("{} tin nhắn trong trang local", thread.messages.len()));
                                                    if ui.add_enabled(self.message_window.newer > 0 && (self.local_page_has_more || ctx.data_mut(|data|data.get_temp::<bool>(Id::new(("workspace-page-before",&thread_id))).unwrap_or(true))), egui::Button::new("200 tin mới hơn →")).clicked() {
                                                        if let (Some(tx), Some(last)) = (WORKER_TX.get(), thread.messages.last()) {
                                                            let _ = tx.send(WorkerCmd::LoadLocalPage {request_id:{self.navigation_request=self.navigation_request.wrapping_add(1);self.pending_jump=None;self.navigation_request}, thread_id:thread_id.clone(),
                                                                cursor:Some(crate::store_v2::HistoryCursor { ts:last.ts,msg_id:last.id.clone() }),
                                                                direction:crate::store_v2::PageDirection::After,replace:true });
                                                            self.message_window.newer = self.message_window.newer.saturating_sub(200);
                                                            ctx.data_mut(|data|data.insert_temp(Id::new(("workspace-page-before",&thread_id)),false));
                                                        }
                                                    }
                                                    if ui.add_enabled(self.message_window.newer > 0, egui::Button::new("Về tin mới nhất")).clicked() {
                                                        if let Some(tx) = WORKER_TX.get() { let _ = tx.send(WorkerCmd::LoadLocalPage {request_id:{self.navigation_request=self.navigation_request.wrapping_add(1);self.pending_jump=None;self.navigation_request},
                                                            thread_id:thread_id.clone(),cursor:None,direction:crate::store_v2::PageDirection::Before,replace:true }); }
                                                        self.message_window.newer = 0;
                                                        ctx.data_mut(|data|data.insert_temp(Id::new(("workspace-page-before",&thread_id)),true));
                                                    }
                                                });
                                            }
                                            if visible_indices.is_empty() && !in_chat_q.is_empty() {
                                                ui.vertical_centered(|ui| {
                                                    ui.add_space(56.0);
                                                    ui.label(
                                                        RichText::new(format!(
                                                            "Không tìm thấy tin nhắn nào khớp từ khóa \"{}\".",
                                                            self.in_chat_search_query.trim()
                                                        ))
                                                        .size(13.5)
                                                        .color(pal.text_muted),
                                                    );
                                                });
                                            }
                                            let last_outgoing_idx = thread
                                                .messages
                                                .iter()
                                                .enumerate()
                                                .rev()
                                                .find(|(_, m)| {
                                                    m.is_outgoing && !is_system_notice_message(m)
                                                })
                                                .map(|(i, _)| i);

                                            for (pos, &idx) in visible_indices.iter().enumerate() {
                                                let msg = &thread.messages[idx];
                                                let prev_msg = pos
                                                    .checked_sub(1)
                                                    .and_then(|p| visible_indices.get(p))
                                                    .map(|&pi| &thread.messages[pi]);
                                                let next_msg = visible_indices
                                                    .get(pos + 1)
                                                    .map(|&ni| &thread.messages[ni]);

                                                if should_show_time_divider(prev_msg, msg) {
                                                    if pos > 0 {
                                                        ui.add_space(6.0);
                                                    }
                                                    render_date_divider(ui, msg, pal);
                                                    ui.add_space(6.0);
                                                }

                                                if is_system_notice_message(msg) {
                                                    render_system_notice_pill(ui, msg, pal);
                                                    ui.add_space(6.0);
                                                    continue;
                                                }

                                                let prev_same_sender = prev_msg
                                                    .map(|pm| same_message_cluster(pm, msg))
                                                    .unwrap_or(false);
                                                let next_same_sender = next_msg
                                                    .map(|nm| same_message_cluster(msg, nm))
                                                    .unwrap_or(false);
                                                let is_latest_outgoing =
                                                    last_outgoing_idx == Some(idx);

                                                let sender_ava_opt = if !is_group {
                                                    thread_avatar.as_deref()
                                                } else {
                                                    sender_avatars.get(msg.wire_ref.sender.as_ref(), &msg.sender)
                                                };
                                                let message_top=ui.cursor().top();
                                                if let Some(action) = render_chat_bubble(
                                                    ui,
                                                    msg,
                                                    prev_same_sender,
                                                    next_same_sender,
                                                    is_latest_outgoing,
                                                    is_group,
                                                    &thread_name,
                                                    (&thread_id, is_group),
                                                    sender_ava_opt,
                                                    pal,
                                                    &self.sticker_textures,
                                                    &mut self.loading_stickers,
                                                    &self.failed_preview_keys,
                                                    &self.emoji_textures,
                                                    idx,
                                                ) {
                                                    pending_action = Some((idx, action));
                                                }
                                                let highlight_key=Id::new(("reply-jump-highlight",&thread_id,&msg.id));
                                                let message_rect=Rect::from_min_max(Pos2::new(ui.max_rect().left(),message_top),Pos2::new(ui.max_rect().right(),ui.cursor().top()));
                                                if jump_target.as_ref().is_some_and(|(_,id)|id==&msg.id) {
                                                    ui.scroll_to_rect(message_rect,Some(Align::Center));
                                                    ctx.data_mut(|d|d.insert_temp(highlight_key,std::time::Instant::now()));
                                                    jump_applied=true;
                                                }
                                                if ctx.data_mut(|d|d.get_temp::<std::time::Instant>(highlight_key)).is_some_and(|at|at.elapsed()<Duration::from_secs(2)) {
                                                    ui.painter().rect_stroke(message_rect.expand(3.0),Rounding::same(7.0),Stroke::new(2.0,pal.primary));
                                                    ctx.request_repaint_after(Duration::from_millis(100));
                                                }
                                                let clip = ui.clip_rect();
                                                let bottom = ui.cursor().top();
                                                if in_chat_q.is_empty() && !only_starred
                                                    && visible_read_candidate(msg)
                                                    && clip.contains(Pos2::new(clip.center().x, bottom)) {
                                                    visible_seen.push(msg.clone());
                                                }
                                                ui.add_space(if next_same_sender {
                                                    2.5
                                                } else {
                                                    9.0
                                                });
                                            }
                                        }
                                    }
                                });
                        });
                    if jump_applied {self.pending_jump=None;ctx.request_repaint();}
                    self.observe_workspace_seen(ctx, &thread_id, is_group, visible_seen);
                    if load_older_clicked {
                        self.show_toast("Đang tải trang tin nhắn cũ đã lưu…");
                    }

                    if let Some((idx, action)) = pending_action {
                        let target = self
                            .threads
                            .get(self.selected_thread_index)
                            .and_then(|t| t.messages.get(idx))
                            .cloned();
                        if let Some(m) = target {
                            match action {
                                BubbleAction::OpenNativeMedia { url, mime } => {
                                    if let Some(tx) = WORKER_TX.get() { let _ = tx.send(WorkerCmd::OpenNativeMedia {
                                        thread_id:thread_id.clone(), message_id:m.id.clone(), url, mime }); }
                                }
                                BubbleAction::JumpToReply {reference} => self.jump_to_reference(&thread_id,reference),
                                BubbleAction::OpenMention {user_id,display_name} => self.open_mention_profile(user_id,display_name),
                                BubbleAction::Reply => self.replying_to = Some(m),
                                BubbleAction::Forward => {
                                    self.forward_msg = Some(m);
                                    self.forward_filter.clear();
                                }
                                BubbleAction::React(r) => {
                                    self.status_msg = format!("Đang gửi cảm xúc {r}");
                                    if let Some(tx) = WORKER_TX.get() {
                                        let _ = tx.send(WorkerCmd::React {
                                            to_id: thread_id.clone(),
                                            is_group,
                                            msg: m,
                                            reaction: r.to_string(),
                                        });
                                    }
                                }
                                BubbleAction::TogglePin | BubbleAction::ToggleStar => {
                                    if let Some(real) = self
                                        .threads
                                        .get_mut(self.selected_thread_index)
                                        .and_then(|t| t.messages.get_mut(idx))
                                    {
                                        if action == BubbleAction::TogglePin {
                                            real.is_pinned = !real.is_pinned;
                                            self.status_msg = if real.is_pinned {
                                                "Đã ghim tin nhắn lên đầu hội thoại".into()
                                            } else {
                                                "Đã bỏ ghim tin nhắn".into()
                                            };
                                        } else {
                                            real.is_starred = !real.is_starred;
                                            self.status_msg = if real.is_starred {
                                                "Đã đánh dấu sao tin nhắn".into()
                                            } else {
                                                "Đã bỏ đánh dấu sao".into()
                                            };
                                        }
                                    }
                                    self.persist_threads_async();
                                }
                                BubbleAction::CallAgain { is_video } => {
                                    self.active_call = Some(ActiveCallOverlay {
                                        partner_id: thread_id.clone(),
                                        partner_name: thread_name.clone(),
                                        is_video,
                                        is_group,
                                        state_label: if is_video {
                                            "Đang kết nối cuộc gọi video…".into()
                                        } else {
                                            "Đang kết nối cuộc gọi thoại…".into()
                                        },
                                        started_at: std::time::Instant::now(),
                                        mic_muted: false,
                                        cam_enabled: is_video,
                                        is_incoming_ringing: false,
                                        error_code: None,
                                        error_reason: None,
                                    });
                                    if let Some(tx) = WORKER_TX.get() {
                                        let _ = tx.send(WorkerCmd::StartCall {
                                            partner_id: thread_id.clone(),
                                            partner_name: thread_name.clone(),
                                            partner_avatar: None,
                                            is_video,
                                            is_group,
                                        });
                                    }
                                }
                                BubbleAction::DeleteForMe => {
                                    if let Some(tx) = WORKER_TX.get() {
                                        let _ = tx.send(WorkerCmd::DeleteForMe {
                                            to_id: thread_id.clone(),
                                            is_group,
                                            msg: m,
                                        });
                                    }
                                }
                                BubbleAction::RecallForEveryone => {
                                    if let Some(real) = self
                                        .threads
                                        .get_mut(self.selected_thread_index)
                                        .and_then(|t| t.messages.get_mut(idx))
                                    {
                                        real.text = "[Tin nhắn đã thu hồi]".into();
                                        real.sticker_id = None;
                                        real.link_url = None;
                                        real.link_thumb = None;
                                    }
                                    self.persist_threads_async();
                                    if let Some(tx) = WORKER_TX.get() {
                                        let _ = tx.send(WorkerCmd::RecallMessage {
                                            to_id: thread_id.clone(),
                                            is_group,
                                            msg: m,
                                        });
                                    }
                                }

                            }
                        }
                    }

                    // 3. Optional Sticker Picker Drawer
                    if self.sticker_picker_open {
                        self.render_sticker_picker(ui, &thread_id, is_group, pal);
                    }

                    // 3b. Reply banner (quoted message preview above the composer)
                    if self.replying_to.is_some() {
                        let mut cancel = false;
                        if let Some(q) = &self.replying_to {
                            let who = if q.is_outgoing {
                                "chính bạn".to_string()
                            } else if !q.sender.is_empty() && q.sender != "0" {
                                q.sender.clone()
                            } else {
                                thread_name.clone()
                            };
                            egui::Frame::none()
                                .fill(pal.bg_elevated)
                                .inner_margin(Margin::symmetric(14.0, 6.0))
                                .stroke(Stroke::new(1.0_f32, pal.border))
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        ui.vertical(|ui| {
                                            ui.label(
                                                RichText::new(format!("Đang trả lời {who}"))
                                                    .size(12.0)
                                                    .color(pal.primary)
                                                    .strong(),
                                            );
                                            let preview = if q.sticker_id.is_some() {
                                                "[Nhãn dán]".to_string()
                                            } else {
                                                truncate_chars(&q.text, 100).into_owned()
                                            };
                                            ui.label(
                                                RichText::new(preview)
                                                    .size(12.0)
                                                    .color(pal.text_muted),
                                            );
                                        });
                                        ui.with_layout(
                                            Layout::right_to_left(Align::Center),
                                            |ui| {
                                                if ui
                                                    .small_button(lucide::X)
                                                    .on_hover_cursor(CursorIcon::PointingHand)
                                                    .on_hover_text("Hủy trả lời")
                                                    .clicked()
                                                {
                                                    cancel = true;
                                                }
                                            },
                                        );
                                    });
                                });
                        }
                        if cancel {
                            self.replying_to = None;
                        }
                    }
                    // 3c. Clipboard Image Preview Banner (`Ctrl+V` / `Shift+Insert` screenshot or image file paste)
                    if is_clipboard_paste_triggered(ctx) {
                        if let Some((path, w, h, col_img)) = read_clipboard_image_to_png() {
                            // Consume any text Paste event on this frame so the composer doesn't paste a file path
                            ctx.input_mut(|i| {
                                i.events.retain(|e| !matches!(e, egui::Event::Paste(_)));
                            });
                            let tex = ctx.load_texture(
                                format!("clip_preview_{}", crate::login::unix_now()),
                                col_img,
                                TextureOptions::LINEAR,
                            );
                            if !self.composer_attachments.contains(&path) { self.composer_attachments.push(path.clone()); }
                            self.pending_clipboard_image = Some((path, w, h, tex));
                            self.composer_focus_pending = true;
                            self.show_toast(format!(
                                "Đã dán ảnh ({w}×{h} px) từ Clipboard - nhấn Enter để gửi"
                            ));
                        }
                    }
                    if self.pending_clipboard_image.is_some() {
                        let mut clear_clip = false;
                        if ui.input(|i| i.key_pressed(Key::Escape)) {
                            clear_clip = true;
                        }
                        if let Some((ref path, w, h, ref tex)) = self.pending_clipboard_image {
                            egui::Frame::none()
                                .fill(pal.bg_elevated)
                                .inner_margin(Margin::symmetric(14.0, 6.0))
                                .stroke(Stroke::new(
                                    1.0_f32,
                                    pal.primary.linear_multiply(0.55),
                                ))
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        ui.add(
                                            egui::Image::new(tex)
                                                .fit_to_exact_size(Vec2::new(48.0, 48.0))
                                                .rounding(Rounding::same(6.0)),
                                        );
                                        ui.add_space(6.0);
                                        ui.vertical(|ui| {
                                            ui.label(
                                                RichText::new(format!(
                                                    "{} Ảnh chụp từ bộ nhớ tạm (Ctrl+V)",
                                                    lucide::IMAGE
                                                ))
                                                .size(12.2)
                                                .color(pal.primary)
                                                .strong(),
                                            );
                                            let fname = path
                                                .file_name()
                                                .and_then(|n| n.to_str())
                                                .unwrap_or("clipboard.png");
                                            ui.label(
                                                RichText::new(format!(
                                                    "{fname} · {w}×{h} px - Nhập chú thích bên dưới rồi nhấn Enter để gửi"
                                                ))
                                                .size(11.3)
                                                .color(pal.text_secondary),
                                            );
                                        });
                                        ui.with_layout(
                                            Layout::right_to_left(Align::Center),
                                            |ui| {
                                                if ui
                                                    .small_button(format!("{} Hủy", lucide::X))
                                                    .on_hover_cursor(CursorIcon::PointingHand)
                                                    .on_hover_text("Hủy ảnh dán (Esc)")
                                                    .clicked()
                                                {
                                                    clear_clip = true;
                                                }
                                            },
                                        );
                                    });
                                });
                        }
                        if clear_clip {
                            if let Some((path,_,_,_))=self.pending_clipboard_image.take() { self.composer_attachments.retain(|p|p!=&path); }
                        }
                    }

                    // 4. Bottom Message Composer Bar (Compact Icon Actions + Spacious Input + Padded Send Button)
                    let composer_frame = egui::Frame::none()
                        .fill(pal.bg_sidebar)
                        .inner_margin(Margin::symmetric(14.0, 10.0))
                        .stroke(Stroke::new(1.0_f32, pal.border))
                        .show(ui, |ui| {
                            // Handle drag-and-drop files onto the chat window
                            let dropped: Vec<std::path::PathBuf> = ui.ctx().input(|i| {
                                i.raw
                                    .dropped_files
                                    .iter()
                                    .filter_map(|df| df.path.clone())
                                    .collect()
                            });
                            for path in dropped {
                                if !self.composer_attachments.contains(&path) { self.composer_attachments.push(path); }
                            }
                            if !self.composer_attachments.is_empty() {
                                let mut remove = None;
                                ui.horizontal_wrapped(|ui| {
                                    for (index,path) in self.composer_attachments.iter().enumerate() {
                                        let filename = path.file_name().and_then(|name| name.to_str()).unwrap_or("attachment");
                                        ui.label(RichText::new(filename).size(11.0));
                                        if ui.small_button("×").on_hover_text("Bỏ tệp đính kèm").clicked() { remove=Some(index); }
                                    }
                                });
                                if let Some(index)=remove { let path=self.composer_attachments.remove(index);
                                    if self.pending_clipboard_image.as_ref().is_some_and(|(p,_,_,_)|p==&path) {self.pending_clipboard_image=None;}
                                }
                            }
                            if let Some(store)=self.composer_store.as_ref() {
                                let key=Id::new(("workspace-outbox-cache",&self.user_uin));
                                let now=std::time::Instant::now();
                                let cached=ctx.data_mut(|data|data.get_temp::<(std::time::Instant,Vec<crate::runtime_state::OutboxEntry>)>(key));
                                let entries=match cached {Some((at,entries)) if now.duration_since(at)<Duration::from_millis(500)=>entries,
                                    _=>{let entries=store.pending();ctx.data_mut(|data|data.insert_temp(key,(now,entries.clone())));entries}};
                                let entries=entries.into_iter().filter(|entry|entry.thread_id==thread_id).collect::<Vec<_>>();
                                for entry in entries.iter().take(3) {
                                    let status=match entry.state {crate::message_model::DeliveryState::Pending=>"Đang chờ kết quả gửi",crate::message_model::DeliveryState::Failed=>"Gửi thất bại; không tự gửi lại",_=>"Chưa xác định kết quả; không tự gửi lại"};
                                    ui.horizontal(|ui|{ui.label(RichText::new(format!("{status}: {}",truncate_chars(&entry.draft.text,48))).size(10.5).color(pal.text_secondary));
                                        if !matches!(entry.state,crate::message_model::DeliveryState::Pending) && ui.add_enabled(self.input_message.is_empty()&&self.composer_attachments.is_empty(),egui::Button::new("Khôi phục bản nháp").small()).clicked(){self.input_message=entry.draft.text.clone();self.replying_to=entry.draft.reply.clone();self.composer_attachments=entry.draft.attachments.clone();}
                                    });
                                }
                            }
                            egui::Frame::none()
                                .fill(pal.bg_input)
                                .rounding(Rounding::same(11.0))
                                .stroke(Stroke::new(1.0_f32, pal.border))
                                .inner_margin(Margin::symmetric(8.0, 5.0))
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        ui.spacing_mut().item_spacing.x = 4.0;

                                        // 1) Compact Sticker Icon Button
                                        let (stk_rect, stk_resp) = ui.allocate_exact_size(
                                            Vec2::new(34.0, 34.0),
                                            Sense::click(),
                                        );
                                        if self.sticker_picker_open || stk_resp.hovered() {
                                            ui.painter().rect_filled(
                                                stk_rect,
                                                Rounding::same(8.0),
                                                pal.bg_hover,
                                            );
                                        }
                                        draw_icon_sticker(
                                            ui.painter(),
                                            stk_rect.center(),
                                            18.0,
                                            if self.sticker_picker_open || stk_resp.hovered() {
                                                Color32::from_rgb(251, 191, 36)
                                            } else {
                                                pal.text_secondary
                                            },
                                        );
                                        if stk_resp
                                            .on_hover_cursor(CursorIcon::PointingHand)
                                            .on_hover_text("Nhãn dán (Sticker)")
                                            .clicked()
                                        {
                                            self.sticker_picker_open = !self.sticker_picker_open;
                                        }

                                        // 2) Compact Photo Upload Icon Button
                                        let (img_rect, img_resp) = ui.allocate_exact_size(
                                            Vec2::new(34.0, 34.0),
                                            Sense::click(),
                                        );
                                        if img_resp.hovered() {
                                            ui.painter().rect_filled(
                                                img_rect,
                                                Rounding::same(8.0),
                                                pal.bg_hover,
                                            );
                                        }
                                        draw_icon_image(
                                            ui.painter(),
                                            img_rect.center(),
                                            18.0,
                                            if img_resp.hovered() {
                                                pal.accent_online
                                            } else {
                                                pal.text_secondary
                                            },
                                        );
                                        if img_resp
                                            .on_hover_cursor(CursorIcon::PointingHand)
                                            .on_hover_text("Gửi hình ảnh")
                                            .clicked()
                                        {
                                            if let Some(tx) = WORKER_TX.get() {
                                                let _ = tx.send(WorkerCmd::PickAttachments { thread_id:thread_id.clone(), images_only:true });
                                            }
                                        }

                                        // 3) Compact File Upload Icon Button
                                        let (att_rect, att_resp) = ui.allocate_exact_size(
                                            Vec2::new(34.0, 34.0),
                                            Sense::click(),
                                        );
                                        if att_resp.hovered() {
                                            ui.painter().rect_filled(
                                                att_rect,
                                                Rounding::same(8.0),
                                                pal.bg_hover,
                                            );
                                        }
                                        draw_icon_attachment(
                                            ui.painter(),
                                            att_rect.center(),
                                            17.0,
                                            if att_resp.hovered() {
                                                pal.primary_hover
                                            } else {
                                                pal.text_secondary
                                            },
                                        );
                                        if att_resp
                                            .on_hover_cursor(CursorIcon::PointingHand)
                                            .on_hover_text("Đính kèm File / Video")
                                            .clicked()
                                        {
                                            if let Some(tx) = WORKER_TX.get() {
                                                let _ = tx.send(WorkerCmd::PickAttachments { thread_id:thread_id.clone(), images_only:false });
                                            }
                                        }

                                        // Vertical hairline separator
                                        let (sep_r, _) = ui.allocate_exact_size(
                                            Vec2::new(8.0, 20.0),
                                            Sense::hover(),
                                        );
                                        ui.painter().line_segment(
                                            [sep_r.center_top(), sep_r.center_bottom()],
                                            Stroke::new(1.0_f32, pal.border),
                                        );
                                        // Responsive Borderless Input Field (shrinks gracefully down to 50px)
                                        let send_btn_w = if is_narrow_w { 52.0 } else { 76.0 };
                                        let input_w =
                                            (ui.available_width() - send_btn_w - 12.0).max(50.0);
                                        let text_edit =
                                            egui::TextEdit::multiline(&mut self.input_message)
                                                .desired_rows(1)
                                                .return_key(egui::KeyboardShortcut::new(egui::Modifiers::SHIFT, Key::Enter))
                                                .id(Id::new(("chat_composer_input", &thread_id)))
                                                .frame(false)
                                                .hint_text(format!(
                                                    "Nhập tin nhắn tới {}...",
                                                    truncate_chars(&thread_name, 16)
                                                ))
                                                .margin(Margin::symmetric(6.0, 7.0))
                                                .desired_width(input_w);

                                        let response = ui.add(text_edit);
                                        if response.changed() {
                                            ensure_cjk_font(
                                                ui.ctx(),
                                                std::iter::once(self.input_message.as_str()),
                                            );
                                        }
                                        if self.composer_focus_pending {
                                            response.request_focus();
                                            self.composer_focus_pending = false;
                                        }
                                        response.context_menu(|ui| {
                                            if ui
                                                .button(format!(
                                                    "{} Dán hình ảnh từ Clipboard (Ctrl+V)",
                                                    lucide::IMAGE
                                                ))
                                                .clicked()
                                            {
                                                ui.close_menu();
                                                if let Some((path, w, h, col_img)) =
                                                    read_clipboard_image_to_png()
                                                {
                                                    let tex = ctx.load_texture(
                                                        format!(
                                                            "clip_preview_{}",
                                                            crate::login::unix_now()
                                                        ),
                                                        col_img,
                                                        TextureOptions::LINEAR,
                                                    );
                                                    if !self.composer_attachments.contains(&path) { self.composer_attachments.push(path.clone()); }
                                                    self.pending_clipboard_image = Some((path, w, h, tex));
                                                    self.composer_focus_pending = true;
                                                    self.show_toast(format!(
                                                        "Đã dán ảnh ({w}×{h} px) từ Clipboard - nhấn Enter để gửi"
                                                    ));
                                                } else {
                                                    self.show_toast(
                                                        "Không tìm thấy hình ảnh nào trong Clipboard",
                                                    );
                                                }
                                            }
                                        });

                                        // Keyboard navigation for contextual sticker popover (Zalo PC `changeSelectedIndex`):
                                        // - ArrowUp selects the first suggested sticker
                                        // - ArrowLeft / ArrowRight cycles through suggested stickers
                                        // - ArrowDown / Escape deselects or hides the popover
                                        // - Enter sends the selected sticker if one is highlighted, otherwise sends text!
                                        let mut send_selected_sticker: Option<
                                            crate::chat::StickerItem,
                                        > = None;
                                        if !contextual_stickers.is_empty() && response.has_focus() {
                                            let len = contextual_stickers.len();
                                            if ui.input(|i| i.key_pressed(Key::ArrowUp)) {
                                                self.composer_sticker_selected = Some(0);
                                            } else if let Some(sel) = self.composer_sticker_selected
                                            {
                                                if ui.input(|i| i.key_pressed(Key::ArrowRight)) {
                                                    self.composer_sticker_selected =
                                                        Some((sel + 1) % len);
                                                } else if ui
                                                    .input(|i| i.key_pressed(Key::ArrowLeft))
                                                {
                                                    self.composer_sticker_selected =
                                                        Some((sel + len - 1) % len);
                                                } else if ui
                                                    .input(|i| i.key_pressed(Key::ArrowDown))
                                                {
                                                    self.composer_sticker_selected = None;
                                                }
                                            }
                                        } else {
                                            self.composer_sticker_selected = None;
                                        }

                                        let enter_pressed = composer_submit_response(ui, &response, ime_frame);
                                        if enter_pressed {
                                            if let Some(sel) = self.composer_sticker_selected {
                                                if let Some(st) = contextual_stickers.get(sel) {
                                                    send_selected_sticker = Some(st.clone());
                                                }
                                            }
                                        }
                                        // Responsive Padded Send Button (shows icon-only on very tight widths)
                                        let can_send = !self.input_message.trim().is_empty()
                                            || !self.composer_attachments.is_empty();
                                        let (send_rect, send_resp) = ui.allocate_exact_size(
                                            Vec2::new(send_btn_w, 34.0),
                                            Sense::click(),
                                        );
                                        let send_bg = if !can_send {
                                            pal.primary.linear_multiply(0.42)
                                        } else if send_resp.hovered() {
                                            pal.primary_hover
                                        } else {
                                            pal.primary
                                        };
                                        ui.painter().rect_filled(
                                            send_rect,
                                            Rounding::same(8.0),
                                            send_bg,
                                        );
                                        if !is_narrow_w {
                                            ui.painter().text(
                                                Pos2::new(
                                                    send_rect.left() + 15.0,
                                                    send_rect.center().y,
                                                ),
                                                egui::Align2::LEFT_CENTER,
                                                "Gửi",
                                                FontId::proportional(13.0),
                                                Color32::WHITE,
                                            );
                                            draw_icon_send(
                                                ui.painter(),
                                                Pos2::new(
                                                    send_rect.right() - 17.0,
                                                    send_rect.center().y,
                                                ),
                                                14.0,
                                                Color32::WHITE,
                                            );
                                        } else {
                                            draw_icon_send(
                                                ui.painter(),
                                                send_rect.center(),
                                                16.0,
                                                Color32::WHITE,
                                            );
                                        }


                                        let send_clicked = send_resp
                                            .on_hover_cursor(if can_send {
                                                CursorIcon::PointingHand
                                            } else {
                                                CursorIcon::Default
                                            })
                                            .clicked();

                                        if let Some(chosen) = send_selected_sticker {
                                            self.dismissed_sticker_suggest_for.clear();
                                            self.composer_sticker_selected = None;
                                            response.request_focus();
                                            if let Some(tx) = WORKER_TX.get() {
                                                let _ = tx.send(WorkerCmd::SendSticker {
                                                    to_id: thread_id.clone(),
                                                    sticker_id: chosen.sticker_id,
                                                    cate_id: chosen.cate_id,
                                                    sticker_type: chosen.sticker_type,
                                                    is_group,
                                                });
                                            }
                                        } else if (send_clicked || enter_pressed) && can_send {
                                            let draft = crate::runtime_state::Draft { text:self.input_message.trim().to_owned(),
                                                reply:self.replying_to.clone(), attachments:self.composer_attachments.clone() };
                                            let staged = self.composer_store.as_mut().ok_or_else(|| anyhow::anyhow!("Composer store unavailable"))
                                                .and_then(|store| store.stage(&thread_id, draft));
                                            match staged {
                                                Ok(id) => {
                                                    let entry = self.composer_store.as_ref().and_then(|store| store.entry(&id));
                                                    if let Some(entry) = entry {
                                                        let now = crate::login::unix_now()*1000;
                                                        if let Some(thread) = self.threads.iter_mut().find(|thread| thread.id == thread_id) {
                                                            thread.messages.push(ChatMessage { id:format!("out_{id}"),client_message_id:id.clone(),
                                                                sender:"Tôi".into(),text:entry.draft.text.clone(),time:crate::clock::message_time(now),ts:now,
                                                                is_outgoing:true,delivery:crate::message_model::DeliveryState::Pending,
                                                                kind:crate::message_model::MessageKind::Text,..Default::default() });
                                                        }
                                                        let dispatched=WORKER_TX.get().is_some_and(|tx|tx.send(WorkerCmd::SendOutbox { entry,is_group }).is_ok());
                                                        if !dispatched {
                                                            if let Some(store)=self.composer_store.as_mut(){let _=store.complete(&id,crate::message_model::DeliveryState::Failed);}
                                                            if let Some(thread)=self.threads.iter_mut().find(|thread|thread.id==thread_id){if let Some(message)=thread.messages.iter_mut().find(|message|message.client_message_id==id){message.delivery=crate::message_model::DeliveryState::Failed;}}
                                                            self.status_msg="Đã lưu outbox nhưng worker chưa sẵn sàng; tin chưa được gửi".into();
                                                        }
                                                    }
                                                    self.input_message.clear();self.replying_to=None;self.composer_attachments.clear();self.pending_clipboard_image=None;
                                                    self.dismissed_sticker_suggest_for.clear();self.composer_sticker_selected=None;
                                                    response.request_focus();
                                                }
                                                Err(_) => self.show_toast("Chưa lưu được outbox; nội dung vẫn được giữ để gửi lại thủ công"),
                                            }
                                        } else if enter_pressed {
                                            response.request_focus();
                                        }
                                    });
                                });
                        });

                    // 4b. Floating Contextual Sticker Suggestion Popover (`egui::Area`, rendered AFTER composer bar)
                    if !contextual_stickers.is_empty() && !self.sticker_picker_open {
                        self.render_contextual_sticker_popover(
                            ctx,
                            composer_frame.response.rect,
                            &contextual_stickers,
                            &thread_id,
                            is_group,
                            pal,
                        );
                    }
                    if let Some(ref link_meta) = composer_link_preview {
                        if contextual_stickers.is_empty() && !self.sticker_picker_open {
                            self.render_composer_link_preview_popover(
                                ctx,
                                composer_frame.response.rect,
                                link_meta,
                                pal,
                            );
                        }
                    }
                } else {
                    ui.vertical_centered(|ui| {
                        ui.add_space(120.0);
                        ui.label(
                            RichText::new("Chọn một hội thoại bên trái để bắt đầu trò chuyện")
                                .size(15.0)
                                .color(pal.text_muted),
                        );
                    });
                }
            });

        self.debounce_workspace_draft(ctx);
        self.render_mention_profile(ctx, pal);
        self.render_profile_popover(ctx, pal);
        self.render_toast_overlay(ctx, pal);
        self.render_forward_dialog(ctx, pal);
        self.render_call_overlay(ctx, pal);
        self.render_image_viewer_modal(ctx, pal);
        self.render_phone_lookup_modal(ctx, pal);
        self.render_settings_modal(ctx, pal);
    }
}

impl ZaloGuiApp {
    fn workspace_select_conversation(&mut self, ctx: &egui::Context, index: usize) {
        if self.selected_thread_index != index {
            if let Some(thread) = self.threads.get(self.selected_thread_index) {
                let draft = crate::runtime_state::Draft {
                    text: self.input_message.clone(),
                    reply: self.replying_to.clone(),
                    attachments: self.composer_attachments.clone(),
                };
                let result = self
                    .composer_store
                    .as_mut()
                    .ok_or_else(|| anyhow::anyhow!("Composer store unavailable"))
                    .and_then(|store| store.set_draft(&thread.id, draft));
                if result.is_err() {
                    self.show_toast("Chưa lưu được bản nháp; giữ nguyên hội thoại hiện tại");
                    return;
                }
            }
            self.pending_clipboard_image = None;
            self.foreground_read_thread = None;
            self.message_window.newer = 0;
            self.in_chat_search_query.clear();
        }
        if let Some(target) = self.threads.get(index) {
            ctx.data_mut(|data| {
                data.remove::<String>(Id::new(("seen-sent-signature", &self.user_uin, &target.id)))
            });
        }
        self.select_conversation(index);
        if let Some(thread) = self.threads.get(index) {
            ctx.data_mut(|data| {
                data.insert_temp(
                    Id::new(("workspace-initial-page", &self.user_uin, &thread.id)),
                    true,
                )
            });
        }
    }

    fn queue_workspace_search(
        &mut self,
        ctx: &egui::Context,
        query: &str,
        thread_id: Option<String>,
    ) {
        let key = Id::new(("db-search-debounce", &self.user_uin, &thread_id));
        let now = std::time::Instant::now();
        let mut state = ctx
            .data_mut(|data| data.get_temp::<SearchDebounce>(key))
            .unwrap_or_else(|| SearchDebounce {
                query: String::new(),
                changed_at: now,
                sent: false,
            });
        if state.query != query {
            state = SearchDebounce {
                query: query.into(),
                changed_at: now,
                sent: false,
            };
            if thread_id.is_none() {
                self.db_search_results.clear();
            }
        }
        if !query.is_empty() && !state.sent {
            if now.duration_since(state.changed_at) >= Duration::from_millis(250) {
                if let Some(tx) = WORKER_TX.get() {
                    state.sent = tx
                        .send(WorkerCmd::SearchLocal {
                            query: query.into(),
                            thread_id,
                            offset: 0,
                        })
                        .is_ok();
                }
            } else {
                ctx.request_repaint_after(Duration::from_millis(250));
            }
        }
        ctx.data_mut(|data| data.insert_temp(key, state));
    }

    fn debounce_workspace_draft(&mut self, ctx: &egui::Context) {
        let Some(thread) = self.threads.get(self.selected_thread_index) else {
            return;
        };
        let thread_id = thread.id.clone();
        let draft = crate::runtime_state::Draft {
            text: self.input_message.clone(),
            reply: self.replying_to.clone(),
            attachments: self.composer_attachments.clone(),
        };
        let signature = serde_json::to_string(&draft).unwrap_or_default();
        let key = Id::new(("draft-debounce", &self.user_uin, &thread_id));
        let now = std::time::Instant::now();
        let mut state = ctx
            .data_mut(|data| data.get_temp::<DraftDebounce>(key))
            .unwrap_or_else(|| DraftDebounce {
                signature: String::new(),
                changed_at: now,
                saved: true,
            });
        if state.signature != signature {
            state = DraftDebounce {
                signature,
                changed_at: now,
                saved: false,
            };
        }
        if !state.saved {
            if now.duration_since(state.changed_at) >= Duration::from_millis(500) {
                if let Some(store) = self.composer_store.as_mut() {
                    match store.set_draft(&thread_id, draft) {
                        Ok(()) => {
                            state.saved = true;
                            self.last_composer_save = now;
                        }
                        Err(_) => {
                            self.status_msg =
                                "Chưa lưu được bản nháp; nội dung vẫn được giữ trong trình soạn"
                                    .into();
                            state.changed_at = now;
                        }
                    }
                }
            }
            ctx.request_repaint_after(Duration::from_millis(500));
        }
        ctx.data_mut(|data| data.insert_temp(key, state));
    }

    fn observe_workspace_seen(
        &mut self,
        ctx: &egui::Context,
        thread_id: &str,
        is_group: bool,
        messages: Vec<ChatMessage>,
    ) {
        if std::env::var_os("DAILY_GUI_SOAK_OUTPUT").is_some() {
            return;
        }
        let signature = messages
            .iter()
            .map(|message| message.id.as_str())
            .collect::<Vec<_>>()
            .join(":");
        let key = Id::new(("seen-visible-signature", &self.user_uin, thread_id));
        let previous = ctx
            .data_mut(|data| data.get_temp::<String>(key))
            .unwrap_or_default();
        let (logical_focused, minimized, now) = ctx.input(|input| {
            (
                input.focused,
                input.viewport().minimized.unwrap_or(false),
                (input.time * 1000.0).max(0.0) as u64,
            )
        });
        let native_foreground = crate::read_receipts::native_visible_window_foreground();
        let active = crate::read_receipts::verified_foreground(logical_focused, native_foreground);
        if previous != signature {
            self.read_gate
                .observe(thread_id, false, minimized, false, now);
            ctx.data_mut(|data| data.insert_temp(key, signature.clone()));
        }
        let eligible =
            self.read_gate
                .observe(thread_id, active, minimized, !messages.is_empty(), now);
        if eligible {
            let changed = self
                .threads
                .iter_mut()
                .find(|thread| thread.id == thread_id)
                .is_some_and(|thread| {
                    crate::read_receipts::read_local_visible(&mut thread.unread, &messages)
                });
            if changed {
                self.unread_counts = self
                    .threads
                    .iter()
                    .filter(|thread| thread.unread.has_badge())
                    .map(|thread| (thread.id.clone(), thread.unread.count().max(1)))
                    .collect();
                if let Some(tx) = WORKER_TX.get() {
                    if tx
                        .send(WorkerCmd::LocalReadDelta {
                            account: self.user_uin.clone(),
                            thread_id: thread_id.into(),
                            messages: messages.clone(),
                        })
                        .is_err()
                    {
                        self.status_msg="Chưa lưu được trạng thái đọc local; trạng thái trong bộ nhớ vẫn được giữ.".into();
                    }
                }
                self.persist_threads_async();
            }
        }
        let has_visible = !messages.is_empty();
        let messages =
            crate::read_receipts::verified_visible_receipts(thread_id, is_group, &messages);
        let signature = messages
            .iter()
            .map(|message| message.id.as_str())
            .collect::<Vec<_>>()
            .join(":");
        let sent_key = Id::new(("seen-sent-signature", &self.user_uin, thread_id));
        let sent = ctx
            .data_mut(|data| data.get_temp::<String>(sent_key))
            .unwrap_or_default();
        let candidates = messages.len();
        let mut enqueued = false;
        if eligible
            && !messages.is_empty()
            && sent != signature
            && self.foreground_read_thread.as_deref() != Some(thread_id)
        {
            if let Some(tx) = WORKER_TX.get() {
                if tx
                    .send(WorkerCmd::SeenVisible {
                        thread_id: thread_id.into(),
                        is_group,
                        messages,
                    })
                    .is_ok()
                {
                    enqueued = true;
                    self.foreground_read_thread = Some(thread_id.into());
                    ctx.data_mut(|data| data.insert_temp(sent_key, signature));
                }
            }
        } else if active && !minimized && has_visible && !eligible {
            ctx.request_repaint_after(Duration::from_millis(500));
        }
        crate::read_receipts::audit_visibility(
            logical_focused,
            native_foreground,
            eligible,
            candidates,
            enqueued,
        );
    }
}

fn composer_submit_response(ui: &egui::Ui, response: &egui::Response, ime: bool) -> bool {
    // Response::has_focus takes a context write lock; never call it inside input().
    let focused = response.has_focus();
    ui.input(|input| {
        composer_should_submit(focused, ime, input.key_pressed(Key::Enter), input.modifiers)
    })
}
fn composer_should_submit(
    focused: bool,
    ime: bool,
    enter: bool,
    modifiers: egui::Modifiers,
) -> bool {
    focused
        && !ime
        && enter
        && !modifiers.shift
        && !modifiers.ctrl
        && !modifiers.alt
        && !modifiers.command
}
fn visible_read_candidate(message: &ChatMessage) -> bool {
    message.kind != crate::message_model::MessageKind::System
}
#[cfg(test)]
fn receipt_has_verified_identity(message: &ChatMessage) -> bool {
    let wire = &message.wire_ref;
    let key = |value: &Option<serde_json::Value>| {
        value
            .as_ref()
            .map(|v| {
                v.as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| v.to_string())
            })
            .unwrap_or_default()
    };
    !crate::read_receipts::verified_visible_receipts(
        &key(&wire.sender),
        false,
        std::slice::from_ref(message),
    )
    .is_empty()
}
#[cfg(test)]
mod workspace_reliability_tests {
    use super::*;
    #[test]
    fn composer_focus_and_input_do_not_reenter_egui_context_lock() {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let context = egui::Context::default();
            let mut text = String::new();
            let _ = context.run(egui::RawInput::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let response = ui.add(egui::TextEdit::multiline(&mut text));
                    assert!(!composer_submit_response(ui, &response, false));
                });
            });
            tx.send(()).unwrap();
        });
        rx.recv_timeout(std::time::Duration::from_secs(3))
            .expect("composer render deadlocked");
    }
    #[test]
    fn enter_send_excludes_shift_shortcuts_and_ime_commit() {
        assert!(composer_should_submit(
            true,
            false,
            true,
            egui::Modifiers::NONE
        ));
        assert!(!composer_should_submit(
            true,
            false,
            true,
            egui::Modifiers::SHIFT
        ));
        assert!(!composer_should_submit(
            true,
            true,
            true,
            egui::Modifiers::NONE
        ));
        assert!(!composer_should_submit(
            false,
            false,
            true,
            egui::Modifiers::NONE
        ));
        assert!(!composer_should_submit(
            true,
            false,
            true,
            egui::Modifiers::CTRL
        ));
    }
    #[test]
    fn foreground_archive_and_metadata_only_messages_are_local_read_candidates() {
        let archive = ChatMessage {
            id: "101".into(),
            history_source: Some(serde_json::json!({"id":"101"})),
            ..Default::default()
        };
        assert!(
            visible_read_candidate(&archive),
            "archive display must be readable locally without a transport identity"
        );
        let live = ChatMessage {
            id: "102".into(),
            wire_ref: crate::message_model::WireMessageRef::from_raw(
                &serde_json::json!({"msgId":"102","cliMsgId":"103","uidFrom":"42","idTo":"self","msgType":"chat.text"}),
            ),
            ..Default::default()
        };
        assert!(
            visible_read_candidate(&live),
            "metadata-only/manual unread must not require prehydrated unread IDs"
        );
    }
    #[test]
    fn receipt_candidates_require_original_complete_wire_mapping() {
        let raw = serde_json::json!({"msgId":"42","cliMsgId":"43","uidFrom":"sender","idTo":"destination","msgType":"webchat"});
        let mut message = ChatMessage {
            wire_ref: crate::message_model::WireMessageRef::from_raw(&raw),
            ..Default::default()
        };
        assert!(receipt_has_verified_identity(&message));
        message.history_source = Some(serde_json::json!({"msgType":0}));
        assert!(!receipt_has_verified_identity(&message));
        message.history_source = None;
        message.wire_ref.client_id = None;
        assert!(!receipt_has_verified_identity(&message));
    }
}

#[cfg(test)]
mod sender_avatar_tests {
    use super::*;

    fn contact(id: &str, name: &str, avatar: Option<&str>) -> Thread {
        Thread {
            id: id.into(),
            name: name.into(),
            avatar: avatar.map(str::to_owned),
            preview: String::new(),
            time: String::new(),
            is_group: false,
            messages: Vec::new(),
            presentation: Default::default(),
            unread: Default::default(),
        }
    }

    #[test]
    fn avatar_uses_uid_before_unique_name_and_rejects_unknown_uid() {
        let threads = vec![
            contact("1", "Same", Some("first")),
            contact("2", "Same", Some("second")),
            contact("3", "Unique", Some("third")),
        ];
        let index = SenderAvatars::new(&threads);
        assert_eq!(
            index.get(Some(&serde_json::json!(2)), "Unique"),
            Some("second")
        );
        assert_eq!(
            index.get(Some(&serde_json::json!("1")), "Unique"),
            Some("first")
        );
        assert_eq!(
            index.get(Some(&serde_json::json!("missing")), "Unique"),
            None
        );
        assert_eq!(index.get(None, "Same"), None);
        assert_eq!(index.get(None, "Unique"), Some("third"));
    }

    #[test]
    fn missing_avatar_still_makes_namesake_fallback_ambiguous() {
        let threads = vec![
            contact("1", "Same", None),
            contact("2", "Same", Some("second")),
        ];
        assert_eq!(SenderAvatars::new(&threads).get(None, "Same"), None);
        let threads = vec![threads[1].clone(), threads[0].clone()];
        assert_eq!(SenderAvatars::new(&threads).get(None, "Same"), None);
    }
}
