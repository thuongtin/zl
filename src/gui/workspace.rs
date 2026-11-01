use super::*;

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
        // Responsive breakpoints based on live viewport width
        // -------------------------------------------------------------------
        let screen_rect = ctx.screen_rect();
        let screen_w = screen_rect.width();
        let is_compact_w = false;
        let is_narrow_w = false;
        let left_rail_w = 62.0;

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
                            "{} (Đang trực tuyến — Bấm để mở menu tài khoản)",
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

                    // Navigation Icon 2: Danh bạ / Cá nhân (`ThreadFilter::Direct`)
                    let direct_cnt = self.threads.iter().filter(|t| !t.is_group).count();
                    let group_cnt = self.threads.iter().filter(|t| t.is_group).count();
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
                    draw_lucide_icon(
                        ui.painter(),
                        nav2_rect.center(),
                        lucide::USER,
                        19.0,
                        if is_contacts_nav {
                            pal.primary_hover
                        } else {
                            pal.text_secondary
                        },
                    );
                    if nav2_resp
                        .on_hover_cursor(CursorIcon::PointingHand)
                        .on_hover_text(format!("Cá nhân ({direct_cnt} liên hệ)"))
                        .clicked()
                    {
                        self.thread_filter = if self.thread_filter == ThreadFilter::Direct {
                            ThreadFilter::All
                        } else {
                            ThreadFilter::Direct
                        };
                    }

                    ui.add_space(6.0);

                    // Navigation Icon 2b: Nhóm chat (`ThreadFilter::Groups`)
                    let is_groups_nav = self.thread_filter == ThreadFilter::Groups;
                    let (nav2g_rect, nav2g_resp) =
                        ui.allocate_exact_size(Vec2::new(42.0, 42.0), Sense::click());
                    if is_groups_nav {
                        ui.painter().rect_filled(
                            nav2g_rect,
                            Rounding::same(11.0),
                            pal.primary.linear_multiply(0.18),
                        );
                        let pill = Rect::from_min_size(
                            Pos2::new(nav2g_rect.left() - 6.0, nav2g_rect.center().y - 10.0),
                            Vec2::new(3.5, 20.0),
                        );
                        ui.painter()
                            .rect_filled(pill, Rounding::same(2.0), pal.primary);
                    } else if nav2g_resp.hovered() {
                        ui.painter()
                            .rect_filled(nav2g_rect, Rounding::same(11.0), pal.bg_hover);
                    }
                    draw_icon_contacts(
                        ui.painter(),
                        nav2g_rect.center(),
                        19.0,
                        if is_groups_nav {
                            pal.primary_hover
                        } else {
                            pal.text_secondary
                        },
                    );
                    if group_cnt > 0 {
                        let badge_rect = Rect::from_min_size(
                            Pos2::new(nav2g_rect.right() - 16.0, nav2g_rect.top() + 3.0),
                            Vec2::new(15.0, 14.0),
                        );
                        ui.painter()
                            .rect_filled(badge_rect, Rounding::same(7.0), pal.primary);
                        ui.painter().text(
                            badge_rect.center(),
                            egui::Align2::CENTER_CENTER,
                            group_cnt.to_string(),
                            FontId::proportional(9.5),
                            Color32::WHITE,
                        );
                    }
                    if nav2g_resp
                        .on_hover_cursor(CursorIcon::PointingHand)
                        .on_hover_text(format!("Nhóm chat ({group_cnt} nhóm)"))
                        .clicked()
                    {
                        self.thread_filter = if self.thread_filter == ThreadFilter::Groups {
                            ThreadFilter::All
                        } else {
                            ThreadFilter::Groups
                        };
                        if self.thread_filter == ThreadFilter::Groups {
                            if let Some((gidx, gthread)) =
                                self.threads.iter().enumerate().find(|(_, t)| t.is_group)
                            {
                                let gid = gthread.id.clone();
                                self.selected_thread_index = gidx;
                                if let Some(tx) = WORKER_TX.get() {
                                    let _ = tx.send(WorkerCmd::LoadGroupHistory {
                                        group_id: gid,
                                        is_group: true,
                                    });
                                }
                            }
                            if group_cnt == 0
                                || self.threads.iter().any(|t| {
                                    t.is_group && crate::chat::is_generic_thread_name(&t.name)
                                })
                            {
                                if let Some(tx) = WORKER_TX.get() {
                                    let _ = tx.send(WorkerCmd::RefreshThreads);
                                }
                            }
                        }
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
                            self.selected_thread_index = cidx;
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
                        }
                        self.show_toast(format!(
                            "Đang đồng bộ ngầm {} hội thoại…",
                            self.threads.len()
                        ));
                    }

                    ui.add_space(6.0);

                    // Bottom Action 2: Theme / Settings Icon
                    // Bottom Action 2: Settings Modal Button
                    let has_upd_badge = self
                        .update_info
                        .as_ref()
                        .map(|u| u.has_update)
                        .unwrap_or(false);
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
                    if has_upd_badge {
                        ui.painter().circle_filled(
                            Pos2::new(set_rect.right() - 8.0, set_rect.top() + 8.0),
                            4.2,
                            pal.accent_online,
                        );
                    }
                    let set_tip = if let Some(ref u) = self.update_info {
                        if u.has_update {
                            format!(
                                "Cài đặt Zalo (🚀 Có bản cập nhật mới v{})",
                                u.latest_version
                            )
                        } else {
                            format!("Cài đặt Zalo (v{})", env!("CARGO_PKG_VERSION"))
                        }
                    } else {
                        format!("Cài đặt Zalo (v{})", env!("CARGO_PKG_VERSION"))
                    };
                    if set_resp
                        .on_hover_cursor(CursorIcon::PointingHand)
                        .on_hover_text(set_tip)
                        .clicked()
                    {
                        self.settings_modal_open = !self.settings_modal_open;
                        if self.settings_modal_open && has_upd_badge {
                            self.settings_tab = SettingsTab::Update;
                        }
                    }
                });
            });

        // -------------------------------------------------------------------
        // 2. CONVERSATION LIST SIDEBAR (Responsive width, auto-adapts on resize)
        // -------------------------------------------------------------------
        let sidebar_exact_w = 300.0_f32;
        let sidebar_id = "conversation_sidebar_fixed";
        egui::SidePanel::left(sidebar_id)
            .resizable(false)
            .exact_width(sidebar_exact_w)
            .frame(
                egui::Frame::none()
                    .fill(pal.bg_sidebar)
                    .stroke(Stroke::new(1.0_f32, pal.border)),
            )
            .show(ctx, |ui| {
                ui.set_min_width(sidebar_exact_w);
                ui.set_max_width(sidebar_exact_w);
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
                            let avail_row_w = sidebar_exact_w - 28.0;
                            let mut trigger_phone_lookup: Option<String> = None;
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 6.0;
                                let search_box_w = (avail_row_w - 38.0).max(110.0);
                                let has_q = !self.search_query.is_empty();
                                let (sbox_rect, _) = ui.allocate_exact_size(
                                    Vec2::new(search_box_w, 32.0),
                                    Sense::hover(),
                                );
                                ui.painter().rect_filled(
                                    sbox_rect,
                                    Rounding::same(8.0),
                                    pal.bg_input,
                                );
                                ui.painter().rect_stroke(
                                    sbox_rect,
                                    Rounding::same(8.0),
                                    Stroke::new(
                                        1.0_f32,
                                        if has_q {
                                            pal.primary.linear_multiply(0.65)
                                        } else {
                                            pal.border
                                        },
                                    ),
                                );
                                draw_icon_search(
                                    ui.painter(),
                                    Pos2::new(sbox_rect.left() + 16.0, sbox_rect.center().y),
                                    13.5,
                                    if has_q {
                                        pal.primary_hover
                                    } else {
                                        pal.text_muted
                                    },
                                );
                                let edit_rect = Rect::from_min_max(
                                    Pos2::new(sbox_rect.left() + 28.0, sbox_rect.top() + 5.0),
                                    Pos2::new(
                                        sbox_rect.right() - if has_q { 26.0 } else { 8.0 },
                                        sbox_rect.bottom() - 5.0,
                                    ),
                                );
                                let s_resp = ui.put(
                                    edit_rect,
                                    egui::TextEdit::singleline(&mut self.search_query)
                                        .id(Id::new("sidebar_search_input"))
                                        .frame(false)
                                        .hint_text("Tìm hội thoại, tin nhắn, SĐT...")
                                        .desired_width(edit_rect.width()),
                                );
                                let s_enter = (s_resp.lost_focus() || s_resp.has_focus())
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
                                    let clr_rect = Rect::from_center_size(
                                        Pos2::new(sbox_rect.right() - 14.0, sbox_rect.center().y),
                                        Vec2::new(18.0, 18.0),
                                    );
                                    let clr_resp = ui.interact(
                                        clr_rect,
                                        Id::new("sidebar_search_clear_btn"),
                                        Sense::click(),
                                    );
                                    if clr_resp.hovered() {
                                        ui.painter().circle_filled(
                                            clr_rect.center(),
                                            8.5,
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
                                let group_tab_cnt =
                                    self.threads.iter().filter(|t| t.is_group).count();
                                let group_tab_label = if group_tab_cnt > 0 {
                                    format!("Nhóm ({group_tab_cnt})")
                                } else {
                                    "Nhóm".to_string()
                                };
                                for (filter, label) in [
                                    (ThreadFilter::All, "Tất cả".to_string()),
                                    (ThreadFilter::Direct, "Cá nhân".to_string()),
                                    (ThreadFilter::Groups, group_tab_label),
                                ] {
                                    let active = self.thread_filter == filter;
                                    let text_w = ui.fonts(|f| {
                                        f.layout_no_wrap(
                                            label.clone(),
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
                                        &label,
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
                                        if filter == ThreadFilter::Groups {
                                            if let Some((gidx, gthread)) = self
                                                .threads
                                                .iter()
                                                .enumerate()
                                                .find(|(_, t)| t.is_group)
                                            {
                                                let gid = gthread.id.clone();
                                                self.selected_thread_index = gidx;
                                                if let Some(tx) = WORKER_TX.get() {
                                                    let _ = tx.send(WorkerCmd::LoadGroupHistory {
                                                        group_id: gid,
                                                        is_group: true,
                                                    });
                                                }
                                            }
                                            if group_tab_cnt == 0
                                                || self.threads.iter().any(|t| {
                                                    t.is_group
                                                        && crate::chat::is_generic_thread_name(
                                                            &t.name,
                                                        )
                                                })
                                            {
                                                if let Some(tx) = WORKER_TX.get() {
                                                    let _ = tx.send(WorkerCmd::RefreshThreads);
                                                }
                                            }
                                        }
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

                let search_outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
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
                        let ts_a = ta
                            .messages
                            .last()
                            .map(|m| normalize_ts_ms(m.ts))
                            .unwrap_or(0);
                        let ts_b = tb
                            .messages
                            .last()
                            .map(|m| normalize_ts_ms(m.ts))
                            .unwrap_or(0);
                        ts_b.cmp(&ts_a)
                    });

                    let msg_hits: Vec<(usize, String, String, String, bool)> =
                        if query.chars().count() >= 2 {
                            let mut hits = Vec::new();
                            for (t_idx, t) in self.threads.iter().enumerate() {
                                for m in t.messages.iter().rev() {
                                    if matches_vn_query(&m.text, &query, &query_folded) {
                                        hits.push((
                                            t_idx,
                                            t.name.clone(),
                                            m.text.clone(),
                                            m.time.clone(),
                                            t.is_group,
                                        ));
                                        if hits.len() >= 25 {
                                            break;
                                        }
                                    }
                                }
                                if hits.len() >= 25 {
                                    break;
                                }
                            }
                            hits
                        } else {
                            Vec::new()
                        };
                    (filtered_indices, msg_hits)
                }));
                let (filtered_indices, msg_hits) = match search_outcome {
                    Ok(pair) => pair,
                    Err(_) => {
                        self.search_query.clear();
                        self.show_toast("⚠️ Đã đặt lại bộ lọc tìm kiếm");
                        ((0..self.threads.len()).collect(), Vec::new())
                    }
                };

                let mut clicked_thread: Option<(usize, String, bool)> = None;
                let mut clicked_msg_hit: Option<(usize, String)> = None;

                let row_target_w = (sidebar_exact_w - 14.0).max(44.0);
                egui::ScrollArea::vertical()
                    .id_salt("threads_list_scroll")
                    .max_width(sidebar_exact_w)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.set_min_width(sidebar_exact_w - 6.0);
                        ui.set_max_width(sidebar_exact_w - 6.0);
                        ui.add_space(4.0);
                        for &i in &filtered_indices {
                            let t = &self.threads[i];
                            let is_selected = i == self.selected_thread_index;
                            let is_pinned_cloud = t.name.contains("Cloud của tôi");

                            let row_height = if is_narrow_w { 50.0 } else { 58.0 };
                            let (alloc_rect, row_resp) = ui.allocate_exact_size(
                                Vec2::new(row_target_w, row_height),
                                Sense::click(),
                            );
                            let row_rect = alloc_rect.translate(Vec2::new(4.0, 0.0));
                            // Space is still allocated so the scrollbar is right, but off-screen rows
                            // skip painting, and with it the avatar download/texture for every thread.
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
                            paint_user_avatar(
                                ui.painter(),
                                avatar_center,
                                av_radius,
                                &t.name,
                                t.is_group,
                                t.avatar.as_deref(),
                                &self.sticker_textures,
                                &mut self.loading_stickers,
                            );
                            // Cutout status dot (`2.5px` ring matching row background) only when user is actively online
                            let is_user_online = !t.is_group
                                && self
                                    .user_online_map
                                    .get(&t.id)
                                    .map(|(onl, _)| *onl)
                                    .unwrap_or(false);
                            if is_user_online {
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
                                ui.painter()
                                    .circle_filled(badge_pos, 4.0, pal.accent_online);
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
                                let name_x = if t.is_group {
                                    draw_icon_contacts(
                                        ui.painter(),
                                        Pos2::new(text_left + 7.0, row_rect.top() + 19.5),
                                        12.5,
                                        pal.primary,
                                    );
                                    text_left + 18.0
                                } else {
                                    text_left
                                };
                                let display_name = truncate_chars(&t.name, max_name_chars);
                                ui.painter().text(
                                    Pos2::new(name_x, row_rect.top() + 12.0),
                                    egui::Align2::LEFT_TOP,
                                    display_name,
                                    FontId::proportional(13.6),
                                    name_color,
                                );

                                // Pin icon + Timestamp (top-right)
                                let time_right = row_rect.right() - 9.0;
                                if !t.time.is_empty() {
                                    let tw = ui.fonts(|f| {
                                        f.layout_no_wrap(
                                            t.time.clone(),
                                            FontId::proportional(11.0),
                                            pal.text_muted,
                                        )
                                        .size()
                                        .x
                                    });
                                    ui.painter().text(
                                        Pos2::new(time_right, row_rect.top() + 13.5),
                                        egui::Align2::RIGHT_TOP,
                                        &t.time,
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

                                // Snippet with `Bạn:` prefix when latest message is outgoing
                                let last_outgoing =
                                    t.messages.last().map(|m| m.is_outgoing).unwrap_or(false);
                                let raw_preview = if t.preview.is_empty() {
                                    if t.is_group {
                                        "Nhóm trò chuyện".to_string()
                                    } else {
                                        "Chưa có tin nhắn".to_string()
                                    }
                                } else if last_outgoing && !t.preview.starts_with("Bạn:") {
                                    format!("Bạn: {}", t.preview)
                                } else {
                                    t.preview.clone()
                                };
                                let unread_n = self.unread_counts.get(&t.id).copied().unwrap_or(0);
                                let max_snip_chars = if is_compact_w {
                                    16
                                } else if unread_n > 0 {
                                    23
                                } else {
                                    28
                                };
                                let snippet = truncate_chars(&raw_preview, max_snip_chars);
                                ui.painter().text(
                                    Pos2::new(text_left, row_rect.top() + 33.5),
                                    egui::Align2::LEFT_TOP,
                                    snippet,
                                    FontId::proportional(12.0),
                                    if unread_n > 0 {
                                        pal.text_primary
                                    } else {
                                        pal.text_secondary
                                    },
                                );
                                if unread_n > 0 {
                                    let badge_txt = if unread_n > 99 {
                                        "99+".to_string()
                                    } else {
                                        unread_n.to_string()
                                    };
                                    let bw = if unread_n >= 10 { 22.0 } else { 18.0 };
                                    let b_rect = Rect::from_min_size(
                                        Pos2::new(
                                            row_rect.right() - bw - 8.0,
                                            row_rect.top() + 31.5,
                                        ),
                                        Vec2::new(bw, 17.0),
                                    );
                                    ui.painter().rect_filled(
                                        b_rect,
                                        Rounding::same(8.5),
                                        pal.danger,
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
                                let (alloc_r, r_resp) = ui.allocate_exact_size(
                                    Vec2::new(row_target_w, 52.0),
                                    Sense::click(),
                                );
                                let r_rect = alloc_r.translate(Vec2::new(4.0, 0.0));
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
                    self.selected_thread_index = idx;
                    self.in_chat_search_open = true;
                    self.in_chat_search_focus_pending = true;
                    self.in_chat_search_query = q_str;
                }
                if let Some((idx, tid, is_group)) = clicked_thread {
                    if self.selected_thread_index != idx {
                        self.replying_to = None;
                        self.filter_starred_only = false;
                    }
                    self.selected_thread_index = idx;
                    if self.unread_counts.remove(&tid).is_some() {
                        let total_unread: usize = self.unread_counts.values().sum();
                        let title = if total_unread > 0 {
                            format!("({total_unread}) Zalo Lite — Native Rust Client")
                        } else {
                            "Zalo Lite — Native Rust Client".to_string()
                        };
                        ctx.send_viewport_cmd(egui::ViewportCommand::Title(title));
                    }
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
            if let Some((ref d_id, ref d_name, ref d_ava, d_is_group, d_msg_count)) = thread_meta {
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
                    .resizable(false)
                    .exact_width(drawer_w)
                    .frame(
                        egui::Frame::none()
                            .fill(pal.bg_sidebar)
                            .stroke(Stroke::new(1.0_f32, pal.border)),
                    )
                    .show(ctx, |ui| {
                        ui.set_max_width(drawer_w);
                        egui::Frame::none()
                            .inner_margin(Margin::symmetric(16.0, 18.0))
                            .show(ui, |ui| {
                                ui.vertical_centered(|ui| {
                                    let (av_rect, _) = ui
                                        .allocate_exact_size(Vec2::new(64.0, 64.0), Sense::hover());
                                    paint_user_avatar(
                                        ui.painter(),
                                        av_rect.center(),
                                        32.0,
                                        d_name,
                                        d_is_group,
                                        d_ava.as_deref(),
                                        &self.sticker_textures,
                                        &mut self.loading_stickers,
                                    );
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
                                    let d_onl = self.user_online_map.get(d_id);
                                    let (sub_txt, sub_col): (String, Color32) = if d_is_group {
                                        ("Nhóm đang hoạt động".into(), pal.accent_online)
                                    } else if d_is_cloud {
                                        ("Lưu trữ đám mây cá nhân".into(), pal.primary_hover)
                                    } else {
                                        match d_fr_status {
                                            crate::chat::FriendshipStatus::Friend => match d_onl {
                                                Some((true, lbl)) if !lbl.is_empty() => (
                                                    format!("✓ Đã kết bạn · {lbl}"),
                                                    pal.accent_online,
                                                ),
                                                Some((false, lbl)) if !lbl.is_empty() => (
                                                    format!("✓ Đã kết bạn · {lbl}"),
                                                    pal.text_secondary,
                                                ),
                                                _ => ("✓ Đã kết bạn".into(), pal.text_secondary),
                                            },
                                            crate::chat::FriendshipStatus::IncomingPending => (
                                                "📩 Đã gửi lời mời kết bạn cho bạn".into(),
                                                pal.primary_hover,
                                            ),
                                            crate::chat::FriendshipStatus::OutgoingPending => {
                                                ("⏳ Đã gửi lời mời kết bạn".into(), pal.warning)
                                            }
                                            crate::chat::FriendshipStatus::Stranger => (
                                                "Người lạ · Chưa kết bạn".into(),
                                                Color32::from_rgb(249, 115, 22),
                                            ),
                                            crate::chat::FriendshipStatus::Unknown => {
                                                ("Đang kiểm tra kết bạn…".into(), pal.text_muted)
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
                                                let _ = tx.send(WorkerCmd::LoadGroupHistory {
                                                    group_id: d_id.clone(),
                                                    is_group: d_is_group,
                                                });
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
                                        let char_cnt = d_id.chars().count();
                                        let short_id = if char_cnt > 10 {
                                            let prefix: String = d_id.chars().take(4).collect();
                                            let suffix: String =
                                                d_id.chars().skip(char_cnt - 4).collect();
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
                                paint_user_avatar(
                                    ui.painter(),
                                    av_rect.center(),
                                    20.0,
                                    &thread_name,
                                    is_group,
                                    thread_avatar.as_deref(),
                                    &self.sticker_textures,
                                    &mut self.loading_stickers,
                                );
                                let cur_onl = self.user_online_map.get(&thread_id);
                                if !is_group
                                    && cur_onl.map(|(onl, _)| *onl).unwrap_or(false)
                                {
                                    let dot_pos =
                                        Pos2::new(av_rect.right() - 4.0, av_rect.bottom() - 4.0);
                                    ui.painter().circle_filled(dot_pos, 6.2, pal.bg_sidebar);
                                    ui.painter().circle_filled(dot_pos, 4.2, pal.accent_online);
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
                                        let (sub_status_lbl, sub_status_col): (String, Color32) =
                                            if is_group {
                                                ("Nhóm trò chuyện".into(), pal.text_secondary)
                                            } else if is_cloud_thread {
                                                ("Lưu trữ đám mây cá nhân".into(), pal.text_secondary)
                                            } else {
                                                match cur_friendship {
                                                    crate::chat::FriendshipStatus::Friend => {
                                                        match cur_onl {
                                                            Some((true, lbl)) if !lbl.is_empty() => (
                                                                format!("Đã kết bạn · {lbl}"),
                                                                pal.accent_online,
                                                            ),
                                                            Some((false, lbl)) if !lbl.is_empty() => (
                                                                format!("Đã kết bạn · {lbl}"),
                                                                pal.text_secondary,
                                                            ),
                                                            _ => (
                                                                "Đã kết bạn".into(),
                                                                pal.text_secondary,
                                                            ),
                                                        }
                                                    }
                                                    crate::chat::FriendshipStatus::IncomingPending => (
                                                        "Đã gửi lời mời kết bạn cho bạn".into(),
                                                        pal.primary_hover,
                                                    ),
                                                    crate::chat::FriendshipStatus::OutgoingPending => (
                                                        "Đã gửi lời mời kết bạn · Chờ đồng ý".into(),
                                                        pal.warning,
                                                    ),
                                                    crate::chat::FriendshipStatus::Stranger => (
                                                        "Người lạ · Chưa kết bạn trên Zalo".into(),
                                                        Color32::from_rgb(249, 115, 22),
                                                    ),
                                                    crate::chat::FriendshipStatus::Unknown => {
                                                        match cur_onl {
                                                            Some((true, lbl)) if !lbl.is_empty() => (
                                                                lbl.clone(),
                                                                pal.accent_online,
                                                            ),
                                                            Some((false, lbl)) if !lbl.is_empty() => (
                                                                lbl.clone(),
                                                                pal.text_secondary,
                                                            ),
                                                            _ => (
                                                                "Trò chuyện cá nhân".into(),
                                                                pal.text_secondary,
                                                            ),
                                                        }
                                                    }
                                                }
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
                                        let ff_ok = crate::media::is_ffmpeg_available();
                                        let (_, cams) = crate::media::list_media_devices(false);
                                        let can_cam = ff_ok && !cams.is_empty();
                                        if !ff_ok {
                                            self.show_toast(
                                                "⚠️ Không tìm thấy ffmpeg trong PATH để bật Camera (cài bằng: winget install Gyan.FFmpeg)",
                                            );
                                        } else if cams.is_empty() {
                                            self.show_toast(
                                                "⚠️ Không phát hiện thiết bị Camera trên máy — đang gọi không hình",
                                            );
                                        }
                                        self.active_call = Some(ActiveCallOverlay {
                                            partner_id: thread_id.clone(),
                                            partner_name: thread_name.clone(),
                                            is_video: true,
                                            is_group,
                                            state_label: "Đang kết nối cuộc gọi video…".into(),
                                            started_at: std::time::Instant::now(),
                                            mic_muted: false,
                                            cam_enabled: can_cam,
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
                    if self.in_chat_search_open {
                        if ui.input(|i| i.key_pressed(Key::Escape)) {
                            self.in_chat_search_open = false;
                            self.in_chat_search_query.clear();
                        }
                    }
                    if self.in_chat_search_open {
                        let q_low = self.in_chat_search_query.trim().to_lowercase();
                        let q_folded = fold_vn_search(&q_low);
                        let match_cnt = if q_low.is_empty() {
                            0
                        } else {
                            self.threads
                                .get(self.selected_thread_index)
                                .map(|t| {
                                    t.messages
                                        .iter()
                                        .filter(|m| {
                                            matches_vn_query(&m.text, &q_low, &q_folded)
                                                || matches_vn_query(&m.sender, &q_low, &q_folded)
                                        })
                                        .count()
                                })
                                .unwrap_or(0)
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
                    let mut pending_action: Option<(usize, BubbleAction)> = None;
                    let mut load_older_clicked = false;
                    // Render only the newest `render_limit` bubbles: egui lays out every bubble
                    // each frame, so a 2 000-message group re-laid out 2 000 galleys per repaint.
                    // "Tải thêm tin nhắn cũ" widens the window before asking the server.
                    if self.chat_render_window.0 != thread_id {
                        self.chat_render_window = (thread_id.clone(), CHAT_RENDER_STEP);
                    }
                    let render_limit = self.chat_render_window.1;
                    let mut widen_window = false;
                    // Group sender -> avatar, built once per frame instead of a scan per bubble.
                    let avatar_by_name: HashMap<&str, &str> = if is_group {
                        self.threads
                            .iter()
                            .filter(|th| !th.is_group)
                            .filter_map(|th| Some((th.name.as_str(), th.avatar.as_deref()?)))
                            .collect()
                    } else {
                        HashMap::new()
                    };
                    let msg_area_height =
                        (ui.available_height() - 66.0 - picker_reserve).max(100.0);

                    egui::ScrollArea::vertical()
                        .id_salt(Id::new(("chat_scroll", &thread_id)))
                        .max_height(msg_area_height)
                        .auto_shrink([false, false])
                        .stick_to_bottom(true)
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
                                                        .add(hist_btn)
                                                        .on_hover_cursor(CursorIcon::PointingHand)
                                                        .on_hover_text(
                                                            "Đồng bộ thêm lịch sử tin nhắn cũ từ máy chủ Zalo",
                                                        )
                                                        .clicked()
                                                    {
                                                        if thread.messages.len() > render_limit {
                                                            widen_window = true;
                                                        } else {
                                                            if let Some(tx) = WORKER_TX.get() {
                                                                let _ = tx.send(
                                                                    WorkerCmd::LoadGroupHistory {
                                                                        group_id: thread_id.clone(),
                                                                        is_group,
                                                                    },
                                                                );
                                                            }
                                                            load_older_clicked = true;
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
                                            let in_chat_folded = fold_vn_search(&in_chat_q);
                                            let visible_indices: Vec<usize> = thread
                                                .messages
                                                .iter()
                                                .enumerate()
                                                .filter(|(_, m)| {
                                                    (!only_starred || m.is_starred)
                                                        && (in_chat_q.is_empty()
                                                            || matches_vn_query(
                                                                &m.text,
                                                                &in_chat_q,
                                                                &in_chat_folded,
                                                            )
                                                            || matches_vn_query(
                                                                &m.sender,
                                                                &in_chat_q,
                                                                &in_chat_folded,
                                                            ))
                                                })
                                                .map(|(i, _)| i)
                                                .collect();
                                            // Search and the starred filter still scan everything.
                                            let visible_indices: Vec<usize> =
                                                if only_starred || !in_chat_q.is_empty() {
                                                    visible_indices
                                                } else {
                                                    let skip = visible_indices
                                                        .len()
                                                        .saturating_sub(render_limit);
                                                    visible_indices[skip..].to_vec()
                                                };
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
                                                    avatar_by_name.get(msg.sender.as_str()).copied()
                                                };
                                                if let Some(action) = render_chat_bubble(
                                                    ui,
                                                    msg,
                                                    prev_same_sender,
                                                    next_same_sender,
                                                    is_latest_outgoing,
                                                    is_group,
                                                    &thread_name,
                                                    sender_ava_opt,
                                                    pal,
                                                    &self.sticker_textures,
                                                    &mut self.loading_stickers,
                                                    &self.emoji_textures,
                                                    idx,
                                                ) {
                                                    pending_action = Some((idx, action));
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
                    if widen_window {
                        self.chat_render_window.1 += CHAT_RENDER_STEP;
                    }
                    if load_older_clicked {
                        self.show_toast("Đang tải thêm tin nhắn cũ từ máy chủ Zalo…");
                    }

                    if let Some((idx, action)) = pending_action {
                        let target = self
                            .threads
                            .get(self.selected_thread_index)
                            .and_then(|t| t.messages.get(idx))
                            .cloned();
                        if let Some(m) = target {
                            match action {
                                BubbleAction::Reply => self.replying_to = Some(m),
                                BubbleAction::Forward => {
                                    self.forward_msg = Some(m);
                                    self.forward_filter.clear();
                                }
                                BubbleAction::React(r) => {
                                    if let Some(real) = self
                                        .threads
                                        .get_mut(self.selected_thread_index)
                                        .and_then(|t| t.messages.get_mut(idx))
                                    {
                                        if real.reactions.contains(&r.to_string()) {
                                            real.reactions.clear();
                                            self.status_msg = "Đã bỏ cảm xúc".into();
                                        } else {
                                            real.reactions = vec![r.to_string()];
                                            self.status_msg = format!("Đã thả cảm xúc {r}");
                                        }
                                    }
                                    self.persist_threads_async();
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
                                BubbleAction::OpenImagePopup {
                                    tex_key,
                                    full_url,
                                    title,
                                    sender,
                                    time,
                                } => {
                                    let hd_tex_key = self.request_hd_texture(&full_url);
                                    self.image_viewer = Some(ImageViewerModal {
                                        tex_key,
                                        hd_tex_key,
                                        full_url,
                                        title,
                                        sender,
                                        time,
                                        zoom: 1.0,
                                        pan: Vec2::ZERO,
                                    });
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
                            self.pending_clipboard_image = Some((path, w, h, tex));
                            self.composer_focus_pending = true;
                            self.show_toast(format!(
                                "Đã dán ảnh ({w}×{h} px) từ Clipboard — nhấn Enter để gửi"
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
                                                    "{fname} · {w}×{h} px — Nhập chú thích bên dưới rồi nhấn Enter để gửi"
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
                            self.pending_clipboard_image = None;
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
                            for dpath in dropped {
                                if let Some(tx) = WORKER_TX.get() {
                                    let _ = tx.send(WorkerCmd::PickAndSendAttachment {
                                        to_id: thread_id.clone(),
                                        is_group,
                                        images_only: false,
                                        caption: String::new(),
                                        preset_path: Some(dpath),
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
                                            let cap = self.input_message.trim().to_string();
                                            self.input_message.clear();
                                            if let Some(tx) = WORKER_TX.get() {
                                                let _ = tx.send(WorkerCmd::PickAndSendAttachment {
                                                    to_id: thread_id.clone(),
                                                    is_group,
                                                    images_only: true,
                                                    caption: cap,
                                                    preset_path: None,
                                                });
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
                                            let cap = self.input_message.trim().to_string();
                                            self.input_message.clear();
                                            if let Some(tx) = WORKER_TX.get() {
                                                let _ = tx.send(WorkerCmd::PickAndSendAttachment {
                                                    to_id: thread_id.clone(),
                                                    is_group,
                                                    images_only: false,
                                                    caption: cap,
                                                    preset_path: None,
                                                });
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
                                            egui::TextEdit::singleline(&mut self.input_message)
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
                                                    self.pending_clipboard_image =
                                                        Some((path, w, h, tex));
                                                    self.composer_focus_pending = true;
                                                    self.show_toast(format!(
                                                        "Đã dán ảnh ({w}×{h} px) từ Clipboard — nhấn Enter để gửi"
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

                                        let enter_pressed =
                                            (response.lost_focus() || response.has_focus())
                                                && ui.input(|i| i.key_pressed(Key::Enter));
                                        if enter_pressed {
                                            if let Some(sel) = self.composer_sticker_selected {
                                                if let Some(st) = contextual_stickers.get(sel) {
                                                    send_selected_sticker = Some(st.clone());
                                                }
                                            }
                                        }
                                        // Responsive Padded Send Button (shows icon-only on very tight widths)
                                        let can_send = !self.input_message.trim().is_empty()
                                            || self.pending_clipboard_image.is_some();
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
                                            self.input_message.clear();
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
                                        } else if (send_clicked || enter_pressed)
                                            && self.pending_clipboard_image.is_some()
                                        {
                                            if let Some((clip_path, _, _, _)) =
                                                self.pending_clipboard_image.take()
                                            {
                                                let cap =
                                                    self.input_message.trim().to_string();
                                                self.input_message.clear();
                                                self.dismissed_sticker_suggest_for.clear();
                                                self.composer_sticker_selected = None;
                                                response.request_focus();
                                                if let Some(tx) = WORKER_TX.get() {
                                                    let _ = tx.send(
                                                        WorkerCmd::PickAndSendAttachment {
                                                            to_id: thread_id.clone(),
                                                            is_group,
                                                            images_only: true,
                                                            caption: cap,
                                                            preset_path: Some(clip_path),
                                                        },
                                                    );
                                                }
                                            }
                                        } else if (send_clicked || enter_pressed) && can_send {
                                            let text = self.input_message.trim().to_string();
                                            self.input_message.clear();
                                            self.dismissed_sticker_suggest_for.clear();
                                            self.composer_sticker_selected = None;
                                            response.request_focus();
                                            let quoted = self.replying_to.take();
                                            if let Some(tx) = WORKER_TX.get() {
                                                if let Some(quoted) = quoted {
                                                    let _ = tx.send(WorkerCmd::SendQuote {
                                                        to_id: thread_id.clone(),
                                                        text,
                                                        quoted,
                                                        is_group,
                                                    });
                                                } else {
                                                    let _ = tx.send(WorkerCmd::SendMessage {
                                                        to_id: thread_id.clone(),
                                                        text,
                                                        is_group,
                                                    });
                                                }
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

        self.render_profile_popover(ctx, pal);
        self.render_wipe_confirm(ctx, pal);
        self.render_toast_overlay(ctx, pal);
        self.render_forward_dialog(ctx, pal);
        self.render_call_overlay(ctx, pal);
        self.render_image_viewer_modal(ctx, pal);
        self.render_phone_lookup_modal(ctx, pal);
        self.render_settings_modal(ctx, pal);
    }
}
