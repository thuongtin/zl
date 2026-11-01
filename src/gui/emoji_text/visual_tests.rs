//! Explicit headless proof of actual egui inline rendering, not a GUI screenshot.
use super::*;
use egui::epaint::{ImageData, Primitive};

struct CpuTexture {
    size: [usize; 2],
    pixels: Vec<Color32>,
    color: bool,
}

fn apply_delta(textures: &mut HashMap<egui::TextureId, CpuTexture>, delta: &egui::TexturesDelta) {
    for (id, update) in &delta.set {
        let pixels: Vec<Color32> = match &update.image {
            ImageData::Color(image) => image.pixels.clone(),
            ImageData::Font(image) => image.srgba_pixels(None).collect(),
        };
        let size = update.image.size();
        if let Some([x, y]) = update.pos {
            let target = textures
                .get_mut(id)
                .expect("Partial update requires existing texture");
            assert!(x + size[0] <= target.size[0] && y + size[1] <= target.size[1]);
            for row in 0..size[1] {
                let start = (y + row) * target.size[0] + x;
                target.pixels[start..start + size[0]]
                    .copy_from_slice(&pixels[row * size[0]..(row + 1) * size[0]]);
            }
        } else {
            textures.insert(
                *id,
                CpuTexture {
                    size,
                    pixels,
                    color: matches!(update.image, ImageData::Color(_)),
                },
            );
        }
    }
}

fn cross(a: Pos2, b: Pos2, p: Pos2) -> f32 {
    (b.x - a.x) * (p.y - a.y) - (b.y - a.y) * (p.x - a.x)
}

fn rasterize(
    primitives: &[egui::ClippedPrimitive],
    textures: &HashMap<egui::TextureId, CpuTexture>,
    size: [usize; 2],
    background: Color32,
) -> (Vec<u8>, usize, usize) {
    // Color32 and atlas pixels are premultiplied sRGBA. Approximate the GPU
    // with nearest sampling and premultiplied source-over in sRGB space.
    let mut canvas = vec![background.to_array(); size[0] * size[1]];
    let (mut color_meshes, mut triangles) = (0, 0);
    for primitive in primitives {
        let Primitive::Mesh(mesh) = &primitive.primitive else {
            panic!("Unexpected callback in inline emoji visual proof");
        };
        let texture = textures
            .get(&mesh.texture_id)
            .expect("Missing primitive texture");
        color_meshes += usize::from(texture.color);
        for indices in mesh.indices.chunks_exact(3) {
            let v = [
                mesh.vertices[indices[0] as usize],
                mesh.vertices[indices[1] as usize],
                mesh.vertices[indices[2] as usize],
            ];
            let area = cross(v[0].pos, v[1].pos, v[2].pos);
            if area.abs() < 0.00001 {
                continue;
            }
            triangles += 1;
            let min_x = v
                .iter()
                .map(|v| v.pos.x)
                .fold(f32::INFINITY, f32::min)
                .max(primitive.clip_rect.min.x)
                .floor()
                .max(0.0) as usize;
            let max_x = v
                .iter()
                .map(|v| v.pos.x)
                .fold(f32::NEG_INFINITY, f32::max)
                .min(primitive.clip_rect.max.x)
                .ceil()
                .clamp(0.0, size[0] as f32) as usize;
            let min_y = v
                .iter()
                .map(|v| v.pos.y)
                .fold(f32::INFINITY, f32::min)
                .max(primitive.clip_rect.min.y)
                .floor()
                .max(0.0) as usize;
            let max_y = v
                .iter()
                .map(|v| v.pos.y)
                .fold(f32::NEG_INFINITY, f32::max)
                .min(primitive.clip_rect.max.y)
                .ceil()
                .clamp(0.0, size[1] as f32) as usize;
            for y in min_y..max_y {
                for x in min_x..max_x {
                    let p = Pos2::new(x as f32 + 0.5, y as f32 + 0.5);
                    if !primitive.clip_rect.contains(p) {
                        continue;
                    }
                    let weights = [
                        cross(v[1].pos, v[2].pos, p) / area,
                        cross(v[2].pos, v[0].pos, p) / area,
                        cross(v[0].pos, v[1].pos, p) / area,
                    ];
                    if weights.iter().any(|w| *w < 0.0) {
                        continue;
                    }
                    // Half-open edge coverage avoids drawing shared quad diagonals twice.
                    let mut owned = true;
                    for (edge, weight) in weights.iter().enumerate() {
                        if weight.abs() < 0.000001 {
                            let a = v[(edge + 1) % 3].pos;
                            let b = v[(edge + 2) % 3].pos;
                            let (a, b) = if area > 0.0 { (a, b) } else { (b, a) };
                            if !(b.y > a.y || (b.y == a.y && b.x < a.x)) {
                                owned = false;
                            }
                        }
                    }
                    if !owned {
                        continue;
                    }
                    let uv = v
                        .iter()
                        .zip(weights)
                        .fold(egui::Vec2::ZERO, |sum, (v, w)| sum + v.uv.to_vec2() * w);
                    let tx = (uv.x * texture.size[0] as f32)
                        .floor()
                        .clamp(0.0, (texture.size[0] - 1) as f32)
                        as usize;
                    let ty = (uv.y * texture.size[1] as f32)
                        .floor()
                        .clamp(0.0, (texture.size[1] - 1) as f32)
                        as usize;
                    let texel = texture.pixels[ty * texture.size[0] + tx].to_array();
                    let mut source = [0.0; 4];
                    for channel in 0..4 {
                        let tint: f32 = v
                            .iter()
                            .zip(weights)
                            .map(|(v, w)| v.color.to_array()[channel] as f32 / 255.0 * w)
                            .sum();
                        source[channel] = texel[channel] as f32 * tint;
                    }
                    let target = &mut canvas[y * size[0] + x];
                    for channel in 0..4 {
                        target[channel] = (source[channel]
                            + target[channel] as f32 * (1.0 - source[3] / 255.0))
                            .round()
                            .clamp(0.0, 255.0) as u8;
                    }
                }
            }
        }
    }
    (
        canvas.into_iter().flatten().collect(),
        color_meshes,
        triangles,
    )
}

#[test]
#[ignore = "Requires explicit ZL_EMOJI_PACK, ZL_EMOJI_VISUAL_DIR and built native EmojiPreview helper"]
fn export_emoji_visual() {
    assert!(
        catalog().validated.iter().all(|valid| *valid),
        "Explicit export requires a complete valid local emoji pack"
    );
    let directory = std::path::PathBuf::from(
        std::env::var_os("ZL_EMOJI_VISUAL_DIR")
            .expect("Set ZL_EMOJI_VISUAL_DIR explicitly; no default output path"),
    );
    std::fs::create_dir_all(&directory).unwrap();
    let fixtures = [
        "Bí danh gốc: :) :)) :D /-heart",
        "Unicode: 😀 😂 ❤️ 🇻🇳 👨‍👩‍👧‍👦",
        "Màu native: 👍🏽 👩‍💻 🫶",
        "Tin nhắn tiếng Việt tự xuống dòng với emoji :) rồi tiếp tục 😀 ❤️ 🇻🇳 👨‍👩‍👧‍👦 và nội dung dài để kiểm tra đường cơ sở, khoảng cách, dấu tiếng Việt cùng 👍🏽 👩‍💻 🫶.",
    ];
    for dark in [true, false] {
        let ctx = egui::Context::default();
        setup_custom_fonts(&ctx);
        ctx.set_visuals(if dark {
            egui::Visuals::dark()
        } else {
            egui::Visuals::light()
        });
        for cluster in ["👍🏽", "👩‍💻", "🫶"] {
            let _ = image(&ctx, &ImageKey::Native(cluster.into()));
        }
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            let ready = ["👍🏽", "👩‍💻", "🫶"].iter().all(|cluster| {
                matches!(
                    image(&ctx, &ImageKey::Native((*cluster).into())),
                    Some(Cached::Ready(_))
                )
            });
            if ready {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "Native emoji textures did not become Ready within 10 seconds"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        let size = [780, 560];
        let mut textures = HashMap::new();
        for frame in 0..2 {
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(
                    Pos2::ZERO,
                    Vec2::new(size[0] as f32, size[1] as f32),
                )),
                ..Default::default()
            };
            let output = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.heading(if dark {
                        "Emoji inline: dark"
                    } else {
                        "Emoji inline: light"
                    });
                    ui.label("Actual egui show() / synthetic message fixtures / CPU approximation");
                    ui.add_space(16.0);
                    for (index, fixture) in fixtures.iter().enumerate() {
                        let width = if index == 3 { 310.0 } else { 700.0 };
                        let text_color = ui.visuals().text_color();
                        let response = show(ui, fixture, 20.0, text_color, width);
                        assert!(response.rect.width() <= width + 1.0);
                        if index == 3 {
                            assert!(response.rect.height() > 60.0, "Fixture must wrap");
                        }
                        ui.add_space(18.0);
                    }
                });
            });
            apply_delta(&mut textures, &output.textures_delta);
            if frame == 1 {
                assert!((output.pixels_per_point - 1.0).abs() < 0.001);
                let primitives = ctx.tessellate(output.shapes, output.pixels_per_point);
                let background = if dark {
                    Color32::from_gray(27)
                } else {
                    Color32::from_gray(250)
                };
                let (rgba, color_meshes, triangles) =
                    rasterize(&primitives, &textures, size, background);
                assert!(
                    color_meshes >= 10,
                    "Actual emoji color-image primitives required"
                );
                let path = directory.join(if dark {
                    "emoji-dark.png"
                } else {
                    "emoji-light.png"
                });
                let image =
                    ::image::RgbaImage::from_raw(size[0] as u32, size[1] as u32, rgba).unwrap();
                image.save(&path).unwrap();
                let reopened = ::image::open(&path).unwrap();
                assert_eq!((reopened.width(), reopened.height()), (780, 560));
                println!("{} color_meshes={} triangles={} textures={} missing_textures=0 dimensions=780x560", path.display(), color_meshes, triangles, textures.len());
            }
            for id in output.textures_delta.free {
                textures.remove(&id);
            }
        }
    }
    println!("Coverage: actual egui show/layout/tessellation, font and emoji texture deltas, clipped textured triangles; synthetic fixtures, nearest sampling and sRGB blending, not a GUI screenshot or GPU/interaction validation.");
}

#[test]
#[ignore = "Requires explicit ZL_EMOJI_PACK and ZL_EMOJI_VISUAL_DIR"]
fn export_sidebar_emoji_visual() {
    assert!(
        catalog().validated.iter().all(|valid| *valid),
        "Explicit export requires a complete valid local emoji pack"
    );
    let directory = std::path::PathBuf::from(std::env::var_os("ZL_EMOJI_VISUAL_DIR").unwrap());
    std::fs::create_dir_all(&directory).unwrap();
    for dark in [true, false] {
        let ctx = egui::Context::default();
        setup_custom_fonts(&ctx);
        ctx.set_visuals(if dark {
            egui::Visuals::dark()
        } else {
            egui::Visuals::light()
        });
        let mut textures = HashMap::new();
        for frame in 0..3 {
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(480.0, 260.0))),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        ui.label("Sidebar preview / synthetic fixtures / actual painter");
                        for (index, (text, width)) in [
                            ("🎃 HotSale Halloween chỉ từ 79.000Đ", 240.0),
                            (":)) :D 🇻🇳 nội dung dài để kiểm tra dấu ba chấm", 140.0),
                            ("🎃🎃🎃🎃🎃🎃🎃🎃", 64.0),
                            ("Bạn: Xin chào ❤️", 240.0),
                            ("Không có emoji, tiếng Việt bình thường", 240.0),
                            ("🎃 rất hẹp", 24.0),
                        ]
                        .iter()
                        .enumerate()
                        {
                            let rect = Rect::from_min_size(
                                Pos2::new(16.0, 45.0 + index as f32 * 32.0),
                                Vec2::new(*width, 20.0),
                            );
                            ui.painter().rect_filled(
                                rect.expand(3.0),
                                3.0,
                                if dark {
                                    Color32::from_gray(45)
                                } else {
                                    Color32::from_gray(230)
                                },
                            );
                            paint_preview(ui, text, rect, 12.0, ui.visuals().text_color());
                        }
                    });
                },
            );
            apply_delta(&mut textures, &output.textures_delta);
            if frame == 2 {
                let primitives = ctx.tessellate(output.shapes, output.pixels_per_point);
                let (rgba, color_meshes, _) =
                    rasterize(&primitives, &textures, [480, 260], Color32::TRANSPARENT);
                assert!(color_meshes >= 6, "Sidebar must paint original color textures; adjacent pumpkins share one mesh: {color_meshes}");
                let path = directory.join(if dark {
                    "sidebar-dark.png"
                } else {
                    "sidebar-light.png"
                });
                ::image::RgbaImage::from_raw(480, 260, rgba)
                    .unwrap()
                    .save(&path)
                    .unwrap();
                let reopened = ::image::open(&path).unwrap();
                assert_eq!((reopened.width(), reopened.height()), (480, 260));
                println!(
                    "SIDEBAR VISUAL PASS: {} color_meshes={color_meshes} missing_textures=0",
                    path.display()
                );
            }
        }
    }
}

#[test]
#[ignore = "Requires explicit ZL_EMOJI_PACK and ZL_EMOJI_VISUAL_DIR"]
fn export_reaction_badge_visual() {
    assert!(
        catalog().validated.iter().all(|valid| *valid),
        "Explicit export requires a complete valid local emoji pack"
    );
    let directory = std::path::PathBuf::from(std::env::var_os("ZL_EMOJI_VISUAL_DIR").unwrap());
    std::fs::create_dir_all(&directory).unwrap();
    for dark in [true, false] {
        let ctx = egui::Context::default();
        setup_custom_fonts(&ctx);
        let pal = if dark {
            crate::gui::theme::ThemePalette::dark()
        } else {
            crate::gui::theme::ThemePalette::light()
        };
        pal.apply_to_ctx(&ctx);
        let msg = crate::chat::ChatMessage {
            text: "Ok".into(),
            reaction_summaries: crate::reactions::parse_reactions(
                &serde_json::json!({"reactions":{"fixture-user":{"5":2}}}),
            ),
            ..Default::default()
        };
        assert_eq!(msg.reaction_summaries[0].count, 2);
        let mut textures = HashMap::new();
        for frame in 0..3 {
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(320.0, 160.0))),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        ui.label("Synthetic badge / actual app renderer");
                        ui.add_space(10.0);
                        ui.label(egui::RichText::new("Ok").size(22.0));
                        crate::gui::messages::render_reactions(ui, &msg, &pal, &HashMap::new());
                    });
                },
            );
            apply_delta(&mut textures, &output.textures_delta);
            if frame == 2 {
                let primitives = ctx.tessellate(output.shapes, output.pixels_per_point);
                let (rgba, color_meshes, _) =
                    rasterize(&primitives, &textures, [320, 160], Color32::TRANSPARENT);
                assert!(color_meshes >= 1);
                let path = directory.join(if dark {
                    "reaction-dark.png"
                } else {
                    "reaction-light.png"
                });
                ::image::RgbaImage::from_raw(320, 160, rgba)
                    .unwrap()
                    .save(&path)
                    .unwrap();
                let reopened = ::image::open(&path).unwrap();
                assert_eq!((reopened.width(), reopened.height()), (320, 160));
                println!(
                    "REACTION VISUAL PASS: count=2 color_meshes={color_meshes} {}",
                    path.display()
                );
            }
        }
    }
}
