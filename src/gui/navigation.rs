use super::*;

impl ZaloGuiApp {
    pub(super) fn jump_to_reference(&mut self, thread_id: &str, reference: String) {
        self.navigation_request = self.navigation_request.wrapping_add(1);
        self.pending_jump = None;
        if let Some(tx) = WORKER_TX.get() {
            let _ = tx.send(WorkerCmd::NavigateMessage {
                thread_id: thread_id.to_owned(),
                reference,
                request_id: self.navigation_request,
            });
        }
    }

    pub(super) fn open_mention_profile(&mut self, user_id: String, display_name: String) {
        // The protocol UID, not a display-name match, selects this profile.
        self.mention_profile = Some((
            user_id.clone(),
            Some(crate::chat::PhoneLookupResult {
                user_id: user_id.clone(),
                display_name,
                zalo_name: String::new(),
                phone: String::new(),
                avatar: String::new(),
                status: String::new(),
                friendship: Default::default(),
            }),
        ));
        if let Some(tx) = WORKER_TX.get() {
            let _ = tx.send(WorkerCmd::LookupMention { user_id });
        }
    }

    pub(super) fn render_mention_profile(&mut self, ctx: &egui::Context, pal: &ThemePalette) {
        let Some((id, profile)) = self.mention_profile.clone() else {
            return;
        };
        let mut open = true;
        egui::Window::new("Thông tin thành viên")
            .id(Id::new("mention-profile"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                if let Some(profile) = profile {
                    let (rect, _) = ui.allocate_exact_size(Vec2::splat(64.0), Sense::hover());
                    paint_user_avatar(
                        ui.painter(),
                        rect.center(),
                        32.0,
                        &profile.display_name,
                        false,
                        (!profile.avatar.is_empty()).then_some(profile.avatar.as_str()),
                        &self.sticker_textures,
                        &mut self.loading_stickers,
                    );
                    ui.label(
                        RichText::new(if profile.display_name.is_empty() {
                            "Thành viên"
                        } else {
                            &profile.display_name
                        })
                        .strong()
                        .size(18.0)
                        .color(pal.text_primary),
                    );
                } else {
                    ui.label("Chưa tải được hồ sơ thành viên này.");
                }
                ui.label(
                    RichText::new(format!("ID: {id}"))
                        .size(11.0)
                        .color(pal.text_muted),
                );
            });
        if !open {
            self.mention_profile = None;
        }
    }
}

pub(super) fn paint_sidebar_avatar(
    painter: &egui::Painter,
    center: Pos2,
    radius: f32,
    thread: &Thread,
    textures: &HashMap<String, TextureHandle>,
    loading: &mut HashSet<String>,
    pal: &ThemePalette,
) {
    if !thread.is_group
        || thread
            .avatar
            .as_deref()
            .is_some_and(|s| !s.trim().is_empty())
        || thread.presentation.members.is_empty()
    {
        paint_user_avatar(
            painter,
            center,
            radius,
            &thread.name,
            thread.is_group,
            thread.avatar.as_deref(),
            textures,
            loading,
        );
        return;
    }
    let member_radius = radius * 0.51;
    let offset = radius * 0.47;
    for (index, member) in thread.presentation.members.iter().take(3).enumerate() {
        let dx = if index == 1 { offset } else { -offset };
        let dy = if index == 2 { offset } else { -offset };
        paint_user_avatar(
            painter,
            center + Vec2::new(dx, dy),
            member_radius,
            &member.name,
            false,
            member.avatar.as_deref(),
            textures,
            loading,
        );
    }
    if let Some(count) = thread.presentation.member_count {
        let pos = center + Vec2::splat(offset);
        painter.circle_filled(pos, member_radius, pal.bg_elevated);
        painter.circle_stroke(pos, member_radius, Stroke::new(1.0, pal.border));
        painter.text(
            pos,
            egui::Align2::CENTER_CENTER,
            if count > 999 {
                "999+".into()
            } else {
                count.to_string()
            },
            FontId::proportional((radius * 0.58).max(9.0)),
            pal.text_primary,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reply_target_scrolls_from_offscreen_into_the_actual_egui_clip() {
        let ctx = egui::Context::default();
        let mut visible = false;
        for frame in 0..90 {
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(500.0, 400.0))),
                time: Some(frame as f64 / 60.0),
                ..Default::default()
            };
            let _ = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("reply-scroll-test")
                        .stick_to_bottom(false)
                        .show(ui, |ui| {
                            for index in 0..200 {
                                let (_, rect) = ui.allocate_space(Vec2::new(250.0, 32.0));
                                if index == 80 {
                                    if frame == 0 {
                                        assert!(!ui.clip_rect().intersects(rect));
                                        ui.scroll_to_rect(rect, Some(Align::Center));
                                    } else if ui.clip_rect().contains_rect(rect) {
                                        visible = true;
                                    }
                                }
                            }
                        });
                });
            });
        }
        assert!(
            visible,
            "the offscreen original message must enter the viewport"
        );
    }
}
