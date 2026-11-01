use super::*;

// ---------------------------------------------------------------------------
// Sticker Presets & State Types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum StickerTab {
    Popular,
    Onion,
    Pack(u64),
    Search,
    Store,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ThreadFilter {
    All,
    Direct,
    Groups,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct PresetSticker {
    pub(super) sticker_id: u64,
    pub(super) cate_id: u64,
    pub(super) sticker_type: u32,
    pub(super) label: &'static str,
}

/// Built-in featured Zalo sticker packs (`(cate_id, title)`).
pub(super) const FEATURED_PACK_TABS: &[(u64, &str)] = &[
    (61900, "Zapy Dô Tri"),
    (12815, "Bé Hai"),
    (12748, "Cò Lõ Đi Làm"),
    (12705, "Mèo Meon"),
    (12718, "Gấu Cay"),
    (12045, "Zameo"),
    (11150, "Bé Dúi"),
    (10186, "Đậu Đỏ"),
    (10177, "Heo Luci"),
    (10117, "Chó Toto"),
];

pub(super) fn builtin_pack_stickers(cate_id: u64) -> Option<Vec<crate::chat::StickerItem>> {
    let (name, ids): (&str, Vec<u64>) = match cate_id {
        61900 => ("Zapy Dô Tri", (98930..=98941).collect()),
        12815 => ("Bé Hai", (101001..=101030).collect()),
        12748 => ("Cò Lõ Đi Làm", (99978..=99997).collect()),
        12705 => ("Mèo Meon", (99278..=99293).collect()),
        12718 => ("Gấu Cay", (99486..=99501).collect()),
        12045 => (
            "Zameo Rốn Lồi",
            (47614..=47624).chain(47626..=47633).collect(),
        ),
        11150 => ("Bé Dúi", (34107..=34110).chain(34827..=34837).collect()),
        10186 => ("Đậu Đỏ", (18974..=18995).collect()),
        10177 => ("Heo Luci", (18847..=18871).collect()),
        10117 => ("Chó Toto", (17745..=17764).collect()),
        _ => return None,
    };
    Some(
        ids.into_iter()
            .enumerate()
            .map(|(i, sid)| crate::chat::StickerItem {
                sticker_id: sid,
                cate_id,
                sticker_type: 7,
                label: format!("{name} #{}", i + 1),
            })
            .collect(),
    )
}

pub(super) const POPULAR_STICKERS: &[PresetSticker] = &[
    PresetSticker {
        sticker_id: 18009,
        cate_id: 10130,
        sticker_type: 7,
        label: "Xin chào",
    },
    PresetSticker {
        sticker_id: 21458,
        cate_id: 10302,
        sticker_type: 7,
        label: "Vẫy tay",
    },
    PresetSticker {
        sticker_id: 21459,
        cate_id: 10302,
        sticker_type: 7,
        label: "Vui vẻ",
    },
    PresetSticker {
        sticker_id: 21460,
        cate_id: 10302,
        sticker_type: 7,
        label: "Thả tim",
    },
    PresetSticker {
        sticker_id: 21461,
        cate_id: 10302,
        sticker_type: 7,
        label: "Cười lớn",
    },
    PresetSticker {
        sticker_id: 21462,
        cate_id: 10302,
        sticker_type: 7,
        label: "Hóng",
    },
    PresetSticker {
        sticker_id: 21463,
        cate_id: 10302,
        sticker_type: 7,
        label: "Ngạc nhiên",
    },
    PresetSticker {
        sticker_id: 21464,
        cate_id: 10302,
        sticker_type: 7,
        label: "Cảm ơn",
    },
    PresetSticker {
        sticker_id: 16713,
        cate_id: 10047,
        sticker_type: 7,
        label: "Hello",
    },
    PresetSticker {
        sticker_id: 18869,
        cate_id: 10177,
        sticker_type: 7,
        label: "OK",
    },
    PresetSticker {
        sticker_id: 19773,
        cate_id: 10233,
        sticker_type: 7,
        label: "Tuyệt",
    },
    PresetSticker {
        sticker_id: 17751,
        cate_id: 10117,
        sticker_type: 7,
        label: "Yêu thương",
    },
    PresetSticker {
        sticker_id: 18910,
        cate_id: 10183,
        sticker_type: 7,
        label: "Haha",
    },
    PresetSticker {
        sticker_id: 20695,
        cate_id: 10275,
        sticker_type: 7,
        label: "Chơi game",
    },
    PresetSticker {
        sticker_id: 19949,
        cate_id: 10242,
        sticker_type: 7,
        label: "Mèo con",
    },
    PresetSticker {
        sticker_id: 394,
        cate_id: 14,
        sticker_type: 7,
        label: "Chào bạn",
    },
    PresetSticker {
        sticker_id: 98930,
        cate_id: 61900,
        sticker_type: 7,
        label: "Zapy Xin chào",
    },
    PresetSticker {
        sticker_id: 101001,
        cate_id: 12815,
        sticker_type: 7,
        label: "Bé Hai #1",
    },
    PresetSticker {
        sticker_id: 99978,
        cate_id: 12748,
        sticker_type: 7,
        label: "Cò Lõ #1",
    },
    PresetSticker {
        sticker_id: 99278,
        cate_id: 12705,
        sticker_type: 7,
        label: "Mèo Meon #1",
    },
    PresetSticker {
        sticker_id: 99486,
        cate_id: 12718,
        sticker_type: 7,
        label: "Gấu Cay #1",
    },
    PresetSticker {
        sticker_id: 47614,
        cate_id: 12045,
        sticker_type: 7,
        label: "Zameo #1",
    },
    PresetSticker {
        sticker_id: 34107,
        cate_id: 11150,
        sticker_type: 7,
        label: "Bé Dúi #1",
    },
    PresetSticker {
        sticker_id: 18974,
        cate_id: 10186,
        sticker_type: 7,
        label: "Đậu Đỏ #1",
    },
];

pub(super) const ONION_STICKERS: &[PresetSticker] = &[
    PresetSticker {
        sticker_id: 1,
        cate_id: 0,
        sticker_type: 7,
        label: "Củ hành #1",
    },
    PresetSticker {
        sticker_id: 2,
        cate_id: 0,
        sticker_type: 7,
        label: "Củ hành #2",
    },
    PresetSticker {
        sticker_id: 3,
        cate_id: 0,
        sticker_type: 7,
        label: "Củ hành #3",
    },
    PresetSticker {
        sticker_id: 4,
        cate_id: 0,
        sticker_type: 7,
        label: "Củ hành #4",
    },
    PresetSticker {
        sticker_id: 5,
        cate_id: 0,
        sticker_type: 7,
        label: "Củ hành #5",
    },
    PresetSticker {
        sticker_id: 6,
        cate_id: 0,
        sticker_type: 7,
        label: "Củ hành #6",
    },
    PresetSticker {
        sticker_id: 7,
        cate_id: 0,
        sticker_type: 7,
        label: "Củ hành #7",
    },
    PresetSticker {
        sticker_id: 8,
        cate_id: 0,
        sticker_type: 7,
        label: "Củ hành #8",
    },
    PresetSticker {
        sticker_id: 9,
        cate_id: 0,
        sticker_type: 7,
        label: "Củ hành #9",
    },
    PresetSticker {
        sticker_id: 10,
        cate_id: 0,
        sticker_type: 7,
        label: "Củ hành #10",
    },
    PresetSticker {
        sticker_id: 11,
        cate_id: 0,
        sticker_type: 7,
        label: "Củ hành #11",
    },
    PresetSticker {
        sticker_id: 12,
        cate_id: 0,
        sticker_type: 7,
        label: "Củ hành #12",
    },
    PresetSticker {
        sticker_id: 13,
        cate_id: 0,
        sticker_type: 7,
        label: "Củ hành #13",
    },
    PresetSticker {
        sticker_id: 14,
        cate_id: 0,
        sticker_type: 7,
        label: "Củ hành #14",
    },
    PresetSticker {
        sticker_id: 15,
        cate_id: 0,
        sticker_type: 7,
        label: "Củ hành #15",
    },
    PresetSticker {
        sticker_id: 16,
        cate_id: 0,
        sticker_type: 7,
        label: "Củ hành #16",
    },
    PresetSticker {
        sticker_id: 17,
        cate_id: 0,
        sticker_type: 7,
        label: "Củ hành #17",
    },
    PresetSticker {
        sticker_id: 18,
        cate_id: 0,
        sticker_type: 7,
        label: "Củ hành #18",
    },
    PresetSticker {
        sticker_id: 19,
        cate_id: 0,
        sticker_type: 7,
        label: "Củ hành #19",
    },
    PresetSticker {
        sticker_id: 20,
        cate_id: 0,
        sticker_type: 7,
        label: "Củ hành #20",
    },
    PresetSticker {
        sticker_id: 21,
        cate_id: 0,
        sticker_type: 7,
        label: "Củ hành #21",
    },
    PresetSticker {
        sticker_id: 22,
        cate_id: 0,
        sticker_type: 7,
        label: "Củ hành #22",
    },
    PresetSticker {
        sticker_id: 23,
        cate_id: 0,
        sticker_type: 7,
        label: "Củ hành #23",
    },
    PresetSticker {
        sticker_id: 24,
        cate_id: 0,
        sticker_type: 7,
        label: "Củ hành #24",
    },
    PresetSticker {
        sticker_id: 25,
        cate_id: 0,
        sticker_type: 7,
        label: "Củ hành #25",
    },
    PresetSticker {
        sticker_id: 26,
        cate_id: 0,
        sticker_type: 7,
        label: "Củ hành #26",
    },
    PresetSticker {
        sticker_id: 27,
        cate_id: 0,
        sticker_type: 7,
        label: "Củ hành #27",
    },
    PresetSticker {
        sticker_id: 28,
        cate_id: 0,
        sticker_type: 7,
        label: "Củ hành #28",
    },
    PresetSticker {
        sticker_id: 29,
        cate_id: 0,
        sticker_type: 7,
        label: "Củ hành #29",
    },
    PresetSticker {
        sticker_id: 30,
        cate_id: 0,
        sticker_type: 7,
        label: "Củ hành #30",
    },
];

/// Instant contextual sticker suggestions mapped from Vietnamese/English keywords, emoticons, and emojis.
/// Returns `(canonical_api_keyword, instant_preset_stickers)`.
pub(super) fn instant_contextual_stickers(
    raw_input: &str,
) -> Option<(&'static str, Vec<crate::chat::StickerItem>)> {
    let trimmed = raw_input.trim();
    if trimmed.is_empty() || trimmed.contains('\n') {
        return None;
    }
    if let Some(hit) = instant_contextual_stickers_exact(trimmed) {
        return Some(hit);
    }
    let words: Vec<&str> = trimmed.split_whitespace().collect();
    if words.len() >= 2 && words.len() <= 6 {
        let tail2 = format!("{} {}", words[words.len() - 2], words[words.len() - 1]);
        if let Some(hit) = instant_contextual_stickers_exact(&tail2) {
            return Some(hit);
        }
        if let Some(hit) = instant_contextual_stickers_exact(words[words.len() - 1]) {
            return Some(hit);
        }
        let head2 = format!("{} {}", words[0], words[1]);
        if let Some(hit) = instant_contextual_stickers_exact(&head2) {
            return Some(hit);
        }
        if let Some(hit) = instant_contextual_stickers_exact(words[0]) {
            return Some(hit);
        }
    }
    None
}

pub(super) fn instant_contextual_stickers_exact(
    trimmed: &str,
) -> Option<(&'static str, Vec<crate::chat::StickerItem>)> {
    let low = trimmed.to_lowercase();
    let folded = fold_vn_search(trimmed);
    let has_vn_diacritics = low != folded;

    // Match Zalo PC's `getSuggestionFromSentencesV2`:
    // - If input contains Vietnamese diacritics (`has_vn_diacritics == true`), only match exact accented
    //   Vietnamese phrases (`low`) so unfinished Telex states like `"buôn"` (before pressing `f` -> `"buồn"`)
    //   never prematurely trigger `"buon"`.
    // - If input is plain ASCII (`has_vn_diacritics == false`), match unaccented ASCII keywords (`low`).
    let (api_kw, label_prefix, pairs): (&'static str, &'static str, &[(u64, u64)]) =
        match low.as_str() {
            "🙂" | "😊" | "☺️" | "🙃" | ":)" | ":-)" | ":3" | "^^" | ":>" => (
                ":)",
                "Mỉm cười",
                &[
                    (12685, 98981),
                    (12815, 101002),
                    (12748, 99980),
                    (12705, 99279),
                    (11978, 46575),
                    (11954, 46182),
                    (11934, 45835),
                    (11870, 44809),
                    (11807, 43863),
                    (11659, 41251),
                    (11150, 34827),
                    (10302, 21459),
                ],
            ),
            "🙁" | "☹️" | "☹" | "😢" | "😭" | "😞" | "😔" | "🥺" | "🥹" | ":(" | ":(("
            | ":-(( " | ":-(" | "t_t" => (
                "buon",
                "Buồn",
                &[
                    (12815, 101012),
                    (12748, 99985),
                    (12556, 50348),
                    (12334, 49778),
                    (12238, 49500),
                    (12013, 47156),
                    (11978, 46576),
                    (11955, 46210),
                    (11938, 45902),
                    (11917, 45594),
                    (10302, 21456),
                ],
            ),
            "😂" | "🤣" | "😆" | "😄" | "😁" | "=))" | "=)))" | ":))" | ":)))" | ":v" => {
                (
                    "haha",
                    "Haha",
                    &[
                        (12686, 99001),
                        (12685, 98981),
                        (12334, 49787),
                        (12244, 49561),
                        (12059, 47856),
                        (12048, 47668),
                        (12045, 47617),
                        (12022, 47275),
                        (12017, 47228),
                        (12013, 47154),
                        (10302, 21461),
                        (10183, 18910),
                    ],
                )
            }
            "❤️" | "❤" | "💕" | "💖" | "💘" | "💗" | "💓" | "😍" | "🥰" | "😘" | "<3" | ":*"
            | ":-*" => (
                "yêu",
                "Yêu thương",
                &[
                    (12556, 50344),
                    (12334, 49774),
                    (12017, 47227),
                    (12013, 47147),
                    (11986, 46709),
                    (10939, 30474),
                    (10751, 27695),
                    (10302, 21460),
                    (10117, 17751),
                ],
            ),
            "👍" | "👌" | "✅" | "💯" | "🤝" | "/-strong" => (
                "ok",
                "OK",
                &[
                    (61900, 98936),
                    (12778, 100425),
                    (12748, 99981),
                    (12676, 50745),
                    (12646, 50589),
                    (12556, 50345),
                    (12334, 49789),
                    (12238, 49497),
                    (10233, 19773),
                    (10177, 18869),
                ],
            ),
            "😠" | "😡" | "🤬" | "😤" | ":-h" => (
                "giận",
                "Giận",
                &[
                    (61900, 98933),
                    (12686, 98993),
                    (12052, 47748),
                    (11917, 45599),
                    (11906, 45422),
                    (11454, 38359),
                ],
            ),
            "..." | "...." | "....." | "......" | "-_-" | "==" | "=.=" | "😐" | "😑" | "🙄"
            | "😒" => (
                "....",
                "Cạn lời",
                &[
                    (12740, 99857),
                    (12334, 49777),
                    (12013, 47155),
                    (11917, 45601),
                    (11788, 43550),
                    (10398, 23044),
                    (10313, 21646),
                ],
            ),
            "?" | "??" | "???" | "🤔" | "😮" | "😲" | "😯" | "@@" | "👀" => (
                "??",
                "Thắc mắc",
                &[
                    (12748, 99990),
                    (12738, 99830),
                    (12686, 98999),
                    (12676, 50751),
                    (12616, 50519),
                    (12334, 49780),
                    (12238, 49515),
                    (12013, 47158),
                    (10302, 21463),
                ],
            ),
            "😴" | "😪" | "🥱" | "💤" => (
                "ngu ngon",
                "Ngủ ngon",
                &[
                    (12778, 100431),
                    (11986, 46713),
                    (11962, 46332),
                    (11956, 46226),
                    (11949, 46092),
                    (11819, 44048),
                    (11659, 41263),
                    (10233, 19792),
                ],
            ),
            "🙏" => (
                "cam on",
                "Cảm ơn",
                &[
                    (12778, 100433),
                    (12694, 99120),
                    (61900, 98932),
                    (12238, 49507),
                    (12013, 47153),
                    (11986, 46706),
                    (10302, 21464),
                ],
            ),
            "👋" => (
                "hi",
                "Xin chào",
                &[
                    (61900, 98930),
                    (12778, 100421),
                    (12713, 99407),
                    (12676, 50746),
                    (12238, 49510),
                    (12045, 47621),
                    (12013, 47160),
                    (10302, 21458),
                ],
            ),
            "🥳" | "🎉" | "🎊" | "🎂" => (
                "chuc mung",
                "Chúc mừng",
                &[
                    (12713, 99406),
                    (12694, 99113),
                    (12238, 49511),
                    (12048, 47666),
                    (12013, 47161),
                    (11978, 46570),
                    (11954, 46193),
                    (10233, 19787),
                ],
            ),
            _ if has_vn_diacritics => match low.as_str() {
                "chào" | "xin chào" | "chào bạn" | "chào anh" | "chào chị" | "chào em"
                | "hế lô" => (
                    "hi",
                    "Xin chào",
                    &[
                        (61900, 98930),
                        (12778, 100421),
                        (12713, 99407),
                        (12676, 50746),
                        (12646, 50594),
                        (12238, 49510),
                        (12045, 47621),
                        (12013, 47160),
                        (10302, 21458),
                        (10130, 18009),
                    ],
                ),
                "cười" | "vui quá" | "mắc cười" | "buồn cười" => (
                    "haha",
                    "Haha",
                    &[
                        (12686, 99001),
                        (12685, 98981),
                        (12334, 49787),
                        (12244, 49561),
                        (12059, 47856),
                        (12048, 47668),
                        (12045, 47617),
                        (12017, 47228),
                        (12013, 47154),
                        (10302, 21461),
                        (10183, 18910),
                    ],
                ),
                "ừ" | "ờ" | "vâng" | "dạ" | "ok chị" | "oke chị" | "tuyệt" | "chuẩn" | "được" => {
                    (
                        "ok",
                        "OK",
                        &[
                            (61900, 98936),
                            (12778, 100425),
                            (12748, 99981),
                            (12676, 50745),
                            (12646, 50589),
                            (12556, 50345),
                            (12334, 49789),
                            (12238, 49497),
                            (10233, 19773),
                            (10177, 18869),
                        ],
                    )
                }
                "cảm ơn" | "cám ơn" | "cảm ơn bạn" | "cảm ơn anh" | "cảm ơn chị" | "cảm ơn em"
                | "cảm ơn nhiều" => (
                    "cam on",
                    "Cảm ơn",
                    &[
                        (12778, 100433),
                        (12694, 99120),
                        (61900, 98932),
                        (12238, 49507),
                        (12017, 47226),
                        (12013, 47153),
                        (11986, 46706),
                        (10302, 21464),
                    ],
                ),
                "yêu" | "thương" | "nhớ" | "nhớ quá" | "thả tim" => (
                    "yêu",
                    "Yêu thương",
                    &[
                        (12556, 50344),
                        (12334, 49774),
                        (12017, 47227),
                        (12013, 47147),
                        (11986, 46709),
                        (10939, 30474),
                        (10751, 27695),
                        (10302, 21460),
                        (10117, 17751),
                    ],
                ),
                "buồn" | "buồn quá" | "khóc" | "chán" | "chán quá" | "tủi thân" => (
                    "buon",
                    "Buồn",
                    &[
                        (12815, 101012),
                        (12748, 99985),
                        (12556, 50348),
                        (12334, 49778),
                        (12238, 49500),
                        (12013, 47156),
                        (11978, 46576),
                        (11955, 46210),
                        (11938, 45902),
                        (10302, 21456),
                    ],
                ),
                "xin lỗi" => (
                    "xin loi",
                    "Xin lỗi",
                    &[
                        (61900, 98931),
                        (10601, 25586),
                        (10398, 23046),
                        (10233, 19779),
                    ],
                ),
                "giận" | "tức" | "bực" | "bực mình" | "giận dỗi" => (
                    "giận",
                    "Giận",
                    &[
                        (61900, 98933),
                        (12686, 98993),
                        (12052, 47748),
                        (11917, 45599),
                        (11454, 38359),
                    ],
                ),
                "ngủ" | "ngủ ngon" | "ngủ ngon nha" | "ngủ ngon nhé" | "buồn ngủ" | "đi ngủ" => {
                    (
                        "ngu ngon",
                        "Ngủ ngon",
                        &[
                            (12778, 100431),
                            (11986, 46713),
                            (11962, 46332),
                            (11956, 46226),
                            (11949, 46092),
                            (11819, 44048),
                            (11659, 41263),
                            (10233, 19792),
                        ],
                    )
                }
                "tạm biệt" | "tạm biệt nhé" => (
                    "tam biet",
                    "Tạm biệt",
                    &[
                        (12045, 47622),
                        (11986, 46710),
                        (11969, 46452),
                        (11960, 46294),
                        (11956, 46225),
                        (11949, 46100),
                        (11901, 45345),
                        (10233, 19791),
                    ],
                ),
                "ngạc nhiên" | "hóng" | "ảo thật" => (
                    "wow",
                    "Ngạc nhiên",
                    &[
                        (11786, 43523),
                        (11482, 38578),
                        (11150, 34109),
                        (10939, 30474),
                        (10606, 25682),
                        (10302, 21462),
                        (10302, 21463),
                        (10117, 17757),
                    ],
                ),
                "chúc mừng" | "sinh nhật" | "tết" => (
                    "chuc mung",
                    "Chúc mừng",
                    &[
                        (12713, 99406),
                        (12694, 99113),
                        (12238, 49511),
                        (12048, 47666),
                        (12013, 47161),
                        (11978, 46570),
                        (11954, 46193),
                        (10233, 19787),
                    ],
                ),
                "đói" | "đói bụng" | "đói quá" | "ăn" | "ăn cơm" | "nhậu" | "đi nhậu"
                | "cà phê" => (
                    "đói",
                    "Ăn uống",
                    &[
                        (12778, 100422),
                        (11819, 44039),
                        (11454, 38361),
                        (10939, 30489),
                        (10799, 28642),
                        (10786, 28362),
                        (10655, 26307),
                        (10653, 26271),
                        (10242, 20065),
                    ],
                ),
                _ => return None,
            },
            _ => match low.as_str() {
                "hi" | "hii" | "hiii" | "hello" | "helo" | "he lo" | "chao" | "xin chao"
                | "chao ban" | "hi ban" | "hi anh" | "hi chi" | "hi em" | "hello anh"
                | "hello chi" | "hello em" | "alo" => (
                    "hi",
                    "Xin chào",
                    &[
                        (61900, 98930),
                        (12778, 100421),
                        (12713, 99407),
                        (12676, 50746),
                        (12646, 50594),
                        (12238, 49510),
                        (12045, 47621),
                        (12013, 47160),
                        (10302, 21458),
                        (10130, 18009),
                    ],
                ),
                "haha" | "hahaha" | "hihi" | "hihihi" | "hehe" | "hoho" | "kaka" | "cuoi"
                | "vui" | "vui qua" | "mac cuoi" | "buon cuoi" => (
                    "haha",
                    "Haha",
                    &[
                        (12686, 99001),
                        (12685, 98981),
                        (12334, 49787),
                        (12244, 49561),
                        (12059, 47856),
                        (12048, 47668),
                        (12045, 47617),
                        (12017, 47228),
                        (12013, 47154),
                        (10302, 21461),
                        (10183, 18910),
                    ],
                ),
                "ok" | "oke" | "okay" | "oki" | "okie" | "uk" | "uh" | "uhm" | "um" | "vang"
                | "da" | "ok anh" | "ok chi" | "ok em" | "oke anh" | "oke em" | "tuyet"
                | "chuan" | "duoc" => (
                    "ok",
                    "OK",
                    &[
                        (61900, 98936),
                        (12778, 100425),
                        (12748, 99981),
                        (12676, 50745),
                        (12646, 50589),
                        (12556, 50345),
                        (12334, 49789),
                        (12238, 49497),
                        (10233, 19773),
                        (10177, 18869),
                    ],
                ),
                "cam on" | "cam on ban" | "cam on anh" | "cam on chi" | "cam on em"
                | "cam on nhieu" | "thank" | "thanks" | "thank you" | "tks" | "tkss"
                | "thank kiu" => (
                    "cam on",
                    "Cảm ơn",
                    &[
                        (12778, 100433),
                        (12694, 99120),
                        (61900, 98932),
                        (12238, 49507),
                        (12017, 47226),
                        (12013, 47153),
                        (11986, 46706),
                        (10302, 21464),
                    ],
                ),
                "yeu" | "iu" | "love" | "love u" | "love you" | "thuong" | "nho" | "nho qua"
                | "tha tim" | "tim" => (
                    "yêu",
                    "Yêu thương",
                    &[
                        (12556, 50344),
                        (12334, 49774),
                        (12017, 47227),
                        (12013, 47147),
                        (11986, 46709),
                        (10939, 30474),
                        (10751, 27695),
                        (10302, 21460),
                        (10117, 17751),
                    ],
                ),
                "buon" | "buon qua" | "khoc" | "huhu" | "hic" | "chan" | "chan qua"
                | "tui than" => (
                    "buon",
                    "Buồn",
                    &[
                        (12815, 101012),
                        (12748, 99985),
                        (12556, 50348),
                        (12334, 49778),
                        (12238, 49500),
                        (12013, 47156),
                        (11978, 46576),
                        (11955, 46210),
                        (11938, 45902),
                        (10302, 21456),
                    ],
                ),
                "xin loi" | "sorry" | "sr" | "so ry" => (
                    "xin loi",
                    "Xin lỗi",
                    &[
                        (61900, 98931),
                        (10601, 25586),
                        (10398, 23046),
                        (10233, 19779),
                    ],
                ),
                "gian" | "tuc" | "buc" | "buc minh" | "gian doi" | "cay" => (
                    "giận",
                    "Giận",
                    &[
                        (61900, 98933),
                        (12686, 98993),
                        (12052, 47748),
                        (11917, 45599),
                        (11454, 38359),
                    ],
                ),
                "ngu" | "ngu ngon" | "ngu ngon nha" | "ngu ngon nhe" | "good night" | "gn"
                | "buon ngu" | "di ngu" => (
                    "ngu ngon",
                    "Ngủ ngon",
                    &[
                        (12778, 100431),
                        (11986, 46713),
                        (11962, 46332),
                        (11956, 46226),
                        (11949, 46092),
                        (11819, 44048),
                        (11659, 41263),
                        (10233, 19792),
                    ],
                ),
                "bye" | "byebye" | "good bye" | "goodbye" | "tam biet" | "tam biet nhe"
                | "bibi" | "pp" => (
                    "tam biet",
                    "Tạm biệt",
                    &[
                        (12045, 47622),
                        (11986, 46710),
                        (11969, 46452),
                        (11960, 46294),
                        (11956, 46225),
                        (11949, 46100),
                        (11901, 45345),
                        (10233, 19791),
                    ],
                ),
                "wow" | "ngac nhien" | "hong" | "ao that" => (
                    "wow",
                    "Ngạc nhiên",
                    &[
                        (11786, 43523),
                        (11482, 38578),
                        (11150, 34109),
                        (10939, 30474),
                        (10606, 25682),
                        (10302, 21462),
                        (10302, 21463),
                        (10117, 17757),
                    ],
                ),
                "chuc mung" | "hpbd" | "sinh nhat" | "happy birthday" | "tet" => (
                    "chuc mung",
                    "Chúc mừng",
                    &[
                        (12713, 99406),
                        (12694, 99113),
                        (12238, 49511),
                        (12048, 47666),
                        (12013, 47161),
                        (11978, 46570),
                        (11954, 46193),
                        (10233, 19787),
                    ],
                ),
                "doi" | "doi bung" | "doi qua" | "an" | "an com" | "nhau" | "di nhau" | "cf"
                | "cafe" | "ca phe" => (
                    "đói",
                    "Ăn uống",
                    &[
                        (12778, 100422),
                        (11819, 44039),
                        (11454, 38361),
                        (10939, 30489),
                        (10799, 28642),
                        (10786, 28362),
                        (10655, 26307),
                        (10653, 26271),
                        (10242, 20065),
                    ],
                ),
                _ => return None,
            },
        };
    let items = pairs
        .iter()
        .enumerate()
        .map(|(idx, &(cate_id, sticker_id))| crate::chat::StickerItem {
            sticker_id,
            cate_id,
            sticker_type: 7,
            label: format!("{label_prefix} #{}", idx + 1),
        })
        .collect();
    Some((api_kw, items))
}

impl ZaloGuiApp {
    pub(super) fn render_sticker_picker(
        &mut self,
        ui: &mut egui::Ui,
        thread_id: &str,
        is_group: bool,
        pal: &ThemePalette,
    ) {
        // Trigger background store catalog load once when drawer opens
        if self.sticker_store_packs.is_empty() && !self.sticker_store_loading {
            self.sticker_store_loading = true;
            if let Some(tx) = WORKER_TX.get() {
                let _ = tx.send(WorkerCmd::FetchStickerStore);
            }
        }

        egui::Frame::none()
            .fill(pal.bg_sidebar)
            .stroke(Stroke::new(1.0_f32, pal.border))
            .inner_margin(Margin::symmetric(14.0, 8.0))
            .show(ui, |ui| {
                ui.set_height(248.0);

                // 1. Top Pack & Store Navigation Bar
                ui.horizontal(|ui| {
                    let store_count_lbl = if !self.sticker_store_packs.is_empty() {
                        format!("{} Kho Nhãn Dán ({})", lucide::STORE, self.sticker_store_packs.len())
                    } else {
                        format!("{} Kho Nhãn Dán", lucide::STORE)
                    };

                    for (tab, title) in [
                        (StickerTab::Popular, format!("{} Phổ biến", lucide::FLAME)),
                        (StickerTab::Onion, "Củ hành (30)".to_string()),
                        (StickerTab::Search, format!("{} Tìm nhãn", lucide::SEARCH)),
                        (StickerTab::Store, store_count_lbl),
                    ] {
                        let active = self.sticker_tab == tab;
                        let btn = egui::Button::new(
                            RichText::new(title)
                                .size(12.2)
                                .color(if active {
                                    Color32::WHITE
                                } else {
                                    pal.text_primary
                                })
                                .strong(),
                        )
                        .fill(if active { pal.primary } else { pal.bg_input })
                        .stroke(Stroke::new(
                            1.0_f32,
                            if active { pal.primary } else { pal.border },
                        ))
                        .rounding(Rounding::same(6.0));
                        if ui
                            .add(btn)
                            .on_hover_cursor(CursorIcon::PointingHand)
                            .clicked()
                        {
                            self.sticker_tab = tab;
                        }
                    }

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui
                            .small_button(format!("{} Đóng", lucide::X))
                            .on_hover_cursor(CursorIcon::PointingHand)
                            .clicked()
                        {
                            self.sticker_picker_open = false;
                        }
                    });
                });

                ui.add_space(3.0);

                // 1b. Pinned / Featured Pack Strip (10+ packs + any pack opened from the Store)
                egui::ScrollArea::horizontal()
                    .id_salt("sticker_pack_tabs_scroll")
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 5.0;
                            let pinned_snapshot = self.pinned_sticker_packs.clone();
                            for (cid, pname) in pinned_snapshot {
                                let tab = StickerTab::Pack(cid);
                                let active = self.sticker_tab == tab;
                                let btn = egui::Button::new(
                                    RichText::new(&pname)
                                        .size(11.8)
                                        .color(if active {
                                            Color32::WHITE
                                        } else {
                                            pal.text_secondary
                                        })
                                        .strong(),
                                )
                                .fill(if active {
                                    pal.primary
                                } else {
                                    pal.bg_elevated
                                })
                                .stroke(Stroke::new(
                                    1.0_f32,
                                    if active { pal.primary } else { pal.border },
                                ))
                                .rounding(Rounding::same(6.0));
                                if ui
                                    .add(btn)
                                    .on_hover_cursor(CursorIcon::PointingHand)
                                    .clicked()
                                {
                                    self.sticker_tab = tab;
                                    if !self.sticker_pack_items.contains_key(&cid)
                                        && self.loading_sticker_packs.insert(cid)
                                    {
                                        if let Some(tx) = WORKER_TX.get() {
                                            let _ = tx.send(WorkerCmd::FetchStickerPack {
                                                cate_id: cid,
                                                name: pname,
                                            });
                                        }
                                    }
                                }
                            }
                        });
                    });

                ui.add_space(4.0);

                // 2. Tab Body
                match self.sticker_tab {
                    StickerTab::Store => {
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new("Lọc bộ nhãn:")
                                    .size(12.0)
                                    .color(pal.text_secondary),
                            );
                            ui.add(
                                egui::TextEdit::singleline(&mut self.sticker_store_filter)
                                    .hint_text("Tìm trong 390+ bộ nhãn Zalo (VD: Zapy, Mèo, Gấu, Tết, Cò Lõ…)")
                                    .desired_width(340.0),
                            );
                            if !self.sticker_store_filter.is_empty()
                                && ui.small_button("Xóa").clicked()
                            {
                                self.sticker_store_filter.clear();
                            }
                            if ui
                                .small_button(format!("{} Tải lại kho", lucide::REFRESH))
                                .on_hover_cursor(CursorIcon::PointingHand)
                                .clicked()
                            {
                                self.sticker_store_loading = true;
                                if let Some(tx) = WORKER_TX.get() {
                                    let _ = tx.send(WorkerCmd::FetchStickerStore);
                                }
                            }
                        });
                        ui.add_space(4.0);

                        if self.sticker_store_loading && self.sticker_store_packs.is_empty() {
                            ui.vertical_centered(|ui| {
                                ui.add_space(36.0);
                                ui.spinner();
                                ui.label("Đang tải danh mục Kho Nhãn Dán Zalo…");
                            });
                            return;
                        }

                        let q = self.sticker_store_filter.trim().to_lowercase();
                        let packs_snapshot: Vec<crate::chat::StickerPackInfo> = self
                            .sticker_store_packs
                            .iter()
                            .filter(|p| q.is_empty() || p.name.to_lowercase().contains(&q))
                            .take(120)
                            .cloned()
                            .collect();

                        let mut open_pack: Option<(u64, String)> = None;
                        egui::ScrollArea::vertical()
                            .id_salt("sticker_store_scroll")
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                ui.horizontal_wrapped(|ui| {
                                    ui.spacing_mut().item_spacing = Vec2::new(8.0, 8.0);
                                    for pack in &packs_snapshot {
                                        let icon_key = format!("pack_ic_{}", pack.id);
                                        if !pack.icon_url.is_empty()
                                            && !self.sticker_textures.contains_key(&icon_key)
                                            && self.loading_stickers.insert(icon_key.clone())
                                        {
                                            if let Some(tx) = WORKER_TX.get() {
                                                let _ = tx.send(WorkerCmd::FetchStickerImage {
                                                    key: icon_key.clone(),
                                                    url: pack.icon_url.clone(),
                                                });
                                            }
                                        }

                                        let (card_rect, card_resp) = ui.allocate_exact_size(
                                            Vec2::new(195.0, 56.0),
                                            Sense::click(),
                                        );
                                        let card_bg = if card_resp.hovered() {
                                            pal.bg_hover
                                        } else {
                                            pal.bg_elevated
                                        };
                                        let card_border = if card_resp.hovered() {
                                            pal.primary
                                        } else {
                                            pal.border
                                        };
                                        ui.painter().rect_filled(
                                            card_rect,
                                            Rounding::same(8.0),
                                            card_bg,
                                        );
                                        ui.painter().rect_stroke(
                                            card_rect,
                                            Rounding::same(8.0),
                                            Stroke::new(1.0_f32, card_border),
                                        );

                                        let ic_rect = Rect::from_min_size(
                                            Pos2::new(card_rect.left() + 6.0, card_rect.top() + 6.0),
                                            Vec2::splat(44.0),
                                        );
                                        if let Some(tex) = self.sticker_textures.get(&icon_key) {
                                            ui.painter().image(
                                                tex.id(),
                                                ic_rect,
                                                Rect::from_min_max(
                                                    Pos2::ZERO,
                                                    Pos2::new(1.0, 1.0),
                                                ),
                                                Color32::WHITE,
                                            );
                                        } else {
                                            ui.painter().rect_filled(
                                                ic_rect,
                                                Rounding::same(6.0),
                                                pal.primary.linear_multiply(0.15),
                                            );
                                            draw_icon_sticker(
                                                ui.painter(),
                                                ic_rect.center(),
                                                18.0,
                                                pal.primary,
                                            );
                                        }

                                        ui.painter().text(
                                            Pos2::new(
                                                card_rect.left() + 56.0,
                                                card_rect.top() + 19.0,
                                            ),
                                            egui::Align2::LEFT_CENTER,
                                            truncate_chars(&pack.name, 18),
                                            FontId::proportional(12.5),
                                            pal.text_primary,
                                        );
                                        ui.painter().text(
                                            Pos2::new(
                                                card_rect.left() + 56.0,
                                                card_rect.top() + 38.0,
                                            ),
                                            egui::Align2::LEFT_CENTER,
                                            format!("{} nhãn · Bấm để mở", pack.total_image),
                                            FontId::proportional(11.0),
                                            pal.primary,
                                        );

                                        if card_resp
                                            .on_hover_cursor(CursorIcon::PointingHand)
                                            .on_hover_text(format!(
                                                "Mở bộ nhãn \"{}\" (ID: {})",
                                                pack.name, pack.id
                                            ))
                                            .clicked()
                                        {
                                            open_pack = Some((pack.id, pack.name.clone()));
                                        }
                                    }
                                });
                            });

                        if let Some((cid, pname)) = open_pack {
                            if !self.pinned_sticker_packs.iter().any(|(id, _)| *id == cid) {
                                self.pinned_sticker_packs.insert(0, (cid, pname.clone()));
                            }
                            self.sticker_tab = StickerTab::Pack(cid);
                            if !self.sticker_pack_items.contains_key(&cid)
                                && self.loading_sticker_packs.insert(cid)
                            {
                                if let Some(tx) = WORKER_TX.get() {
                                    let _ = tx.send(WorkerCmd::FetchStickerPack {
                                        cate_id: cid,
                                        name: pname,
                                    });
                                }
                            }
                        }
                    }
                    StickerTab::Search => {
                        let mut trigger_kw: Option<String> = None;
                        ui.horizontal(|ui| {
                            let te = egui::TextEdit::singleline(&mut self.sticker_search_query)
                                .hint_text("Gõ để tự động tìm nhãn dán (xin chào, cảm ơn, khóc, nhớ, cố lên, mệt quá…)")
                                .desired_width(250.0);
                            let resp = ui.add(te);
                            if resp.changed() {
                                self.last_sticker_search_edit_at = std::time::Instant::now();
                            }
                            let enter = (resp.lost_focus() || resp.has_focus())
                                && ui.input(|i| i.key_pressed(Key::Enter));
                            if (ui
                                .button(format!("{} Tìm", lucide::SEARCH))
                                .on_hover_cursor(CursorIcon::PointingHand)
                                .clicked()
                                || enter)
                                && !self.sticker_search_query.trim().is_empty()
                            {
                                trigger_kw = Some(self.sticker_search_query.trim().to_string());
                            }

                            for chip in [
                                "xin chào",
                                "cảm ơn",
                                "haha",
                                "ok",
                                "yêu",
                                "buồn",
                                "khóc",
                                "nhớ",
                                "cố lên",
                                "mệt quá",
                                "ngủ ngon",
                                "chúc mừng",
                            ] {
                                if ui
                                    .small_button(chip)
                                    .on_hover_cursor(CursorIcon::PointingHand)
                                    .clicked()
                                {
                                    self.sticker_search_query = chip.to_string();
                                    trigger_kw = Some(chip.to_string());
                                }
                            }
                        });

                        let q_trimmed = self.sticker_search_query.trim();
                        if trigger_kw.is_none()
                            && !q_trimmed.is_empty()
                            && q_trimmed != self.last_sticker_search_dispatched
                        {
                            let elapsed = self.last_sticker_search_edit_at.elapsed();
                            let debounce = Duration::from_millis(200);
                            if elapsed >= debounce {
                                trigger_kw = Some(q_trimmed.to_string());
                            } else {
                                ui.ctx().request_repaint_after(
                                    debounce - elapsed + Duration::from_millis(10),
                                );
                            }
                        }

                        if let Some(kw) = trigger_kw {
                            self.last_sticker_search_dispatched = kw.clone();
                            self.sticker_search_loading = true;
                            if let Some(tx) = WORKER_TX.get() {
                                let _ = tx.send(WorkerCmd::SearchStickers { keyword: kw });
                            }
                        }
                        ui.add_space(4.0);
                        let items = self.sticker_search_results.clone();
                        if self.sticker_search_loading {
                            ui.vertical_centered(|ui| {
                                ui.add_space(28.0);
                                ui.spinner();
                                ui.label("Đang tìm nhãn dán phù hợp…");
                            });
                        } else if items.is_empty() {
                            ui.vertical_centered(|ui| {
                                ui.add_space(28.0);
                                ui.label(
                                    RichText::new(
                                        "Chọn một từ khóa gợi ý bên trên hoặc nhập từ khóa để tìm nhãn dán Zalo.",
                                    )
                                    .size(12.5)
                                    .color(pal.text_muted),
                                );
                            });
                        } else {
                            self.render_sticker_items_grid(ui, &items, thread_id, is_group, pal);
                        }
                    }
                    StickerTab::Popular | StickerTab::Onion => {
                        let presets = if self.sticker_tab == StickerTab::Popular {
                            POPULAR_STICKERS
                        } else {
                            ONION_STICKERS
                        };
                        let items: Vec<crate::chat::StickerItem> = presets
                            .iter()
                            .map(|p| crate::chat::StickerItem {
                                sticker_id: p.sticker_id,
                                cate_id: p.cate_id,
                                sticker_type: p.sticker_type,
                                label: p.label.to_string(),
                            })
                            .collect();
                        self.render_sticker_items_grid(ui, &items, thread_id, is_group, pal);
                    }
                    StickerTab::Pack(cid) => {
                        if let Some(items) = self.sticker_pack_items.get(&cid).cloned() {
                            self.render_sticker_items_grid(ui, &items, thread_id, is_group, pal);
                        } else {
                            ui.vertical_centered(|ui| {
                                ui.add_space(36.0);
                                ui.spinner();
                                ui.label("Đang tải bộ nhãn dán từ máy chủ Zalo…");
                            });
                        }
                    }
                }
            });
    }

    pub(super) fn render_sticker_items_grid(
        &mut self,
        ui: &mut egui::Ui,
        items: &[crate::chat::StickerItem],
        thread_id: &str,
        is_group: bool,
        pal: &ThemePalette,
    ) {
        egui::ScrollArea::vertical()
            .id_salt(("sticker_items_grid", self.sticker_tab_salt()))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    for item in items {
                        let key = format!("stk_{}_{}", item.cate_id, item.sticker_id);
                        let url = if item.cate_id == 0 && (1..=30).contains(&item.sticker_id) {
                            format!(
                                "https://stc-chat.zdn.vn/images/stickers/default/thumb/{}.png",
                                item.sticker_id
                            )
                        } else {
                            format!(
                                "https://api.zaloapp.com/api/emoticon/sticker/webpc?eid={}&size=130",
                                item.sticker_id
                            )
                        };

                        if !self.sticker_textures.contains_key(&key)
                            && self.loading_stickers.insert(key.clone())
                        {
                            if let Some(tx) = WORKER_TX.get() {
                                let _ = tx.send(WorkerCmd::FetchStickerImage {
                                    key: key.clone(),
                                    url,
                                });
                            }
                        }

                        let (cell_rect, cell_resp) =
                            ui.allocate_exact_size(Vec2::new(68.0, 68.0), Sense::click());
                        if cell_resp.hovered() {
                            ui.painter().rect_filled(
                                cell_rect,
                                Rounding::same(8.0),
                                pal.bg_hover,
                            );
                            ui.painter().rect_stroke(
                                cell_rect,
                                Rounding::same(8.0),
                                Stroke::new(1.0_f32, pal.primary),
                            );
                        }

                        if let Some(tex) = self.sticker_textures.get(&key) {
                            let img_rect = cell_rect.shrink(4.0);
                            ui.painter().image(
                                tex.id(),
                                img_rect,
                                Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                                Color32::WHITE,
                            );
                        } else {
                            ui.painter().text(
                                cell_rect.center(),
                                egui::Align2::CENTER_CENTER,
                                truncate_chars(&item.label, 10),
                                FontId::proportional(10.5),
                                pal.text_secondary,
                            );
                        }

                        if cell_resp
                            .on_hover_cursor(CursorIcon::PointingHand)
                            .on_hover_text(format!(
                                "Gửi nhãn dán: {} (ID: {})",
                                item.label, item.sticker_id
                            ))
                            .clicked()
                        {
                            self.sticker_picker_open = false;
                            if let Some(tx) = WORKER_TX.get() {
                                let _ = tx.send(WorkerCmd::SendSticker {
                                    to_id: thread_id.to_string(),
                                    sticker_id: item.sticker_id,
                                    cate_id: item.cate_id,
                                    sticker_type: item.sticker_type,
                                    is_group,
                                });
                            }
                        }
                    }
                });
            });
    }

    /// Resolve contextual sticker suggestions for the current `self.input_message` (matching Zalo Web App's
    /// live auto-call sticker suggestion bar above the composer when typing keywords, phrases, or emoticons).
    pub(super) fn resolve_contextual_sticker_suggestions(
        &mut self,
        ctx: &egui::Context,
    ) -> Vec<crate::chat::StickerItem> {
        if !self.settings.suggest_stickers_while_typing
            || !self.settings.enable_sticker_keyword_search
        {
            self.composer_sticker_selected = None;
            return Vec::new();
        }
        if self.input_message != self.last_composer_text {
            self.last_composer_text = self.input_message.clone();
            self.last_composer_edit_at = std::time::Instant::now();
            self.composer_sticker_selected = None;
        }

        let trimmed = self.input_message.trim();
        if trimmed.is_empty() {
            self.dismissed_sticker_suggest_for.clear();
            self.composer_sticker_selected = None;
            return Vec::new();
        }
        if trimmed.contains('\n')
            || trimmed.starts_with('#')
            || trimmed.starts_with('@')
            || trimmed.starts_with('/')
            || trimmed.split_whitespace().count() > 5
            || trimmed.chars().count() > 36
        {
            self.composer_sticker_selected = None;
            return Vec::new();
        }
        if self.dismissed_sticker_suggest_for == trimmed {
            return Vec::new();
        }

        let has_alpha = trimmed.chars().any(|c| c.is_alphabetic());
        let low = crate::chat::normalize_sticker_keyword(trimmed);
        let folded = fold_vn_search(&low);
        let instant_match = instant_contextual_stickers(trimmed);
        let word_count = low.split_whitespace().count();
        let char_count = low.chars().count();
        let digit_count = low.chars().filter(|c| c.is_ascii_digit()).count();
        let is_url_like = low.contains("://")
            || low.starts_with("www.")
            || low.contains(".com")
            || low.contains(".vn")
            || low.contains(".net");

        let query_kw = if !has_alpha {
            // Pure emoji / emoticon (`:)`, `🙂`, `😂`, `....`, `???`) -> map to canonical emotion keyword
            instant_match
                .as_ref()
                .map(|(api_kw, _)| (*api_kw).to_string())
        } else if !is_url_like
            && digit_count <= 2
            && (1..=5).contains(&word_count)
            && (2..=32).contains(&char_count)
        {
            // Alphabetic Vietnamese/English chat input: query the exact normalized phrase
            // so Zalo's live `/suggest/stickers` + `/search` APIs return specific stickers
            // for `"khóc"`, `"nhớ"`, `"cố lên"`, `"mệt quá"`, `"đi chơi"`, `"trà sữa"`, etc.
            Some(low.clone())
        } else if let Some((api_kw, _)) = &instant_match {
            Some((*api_kw).to_string())
        } else if self.sticker_suggest_keywords.contains(&low) {
            Some(low.clone())
        } else if !folded.is_empty() && self.sticker_suggest_keywords.contains(&folded) {
            Some(folded)
        } else {
            None
        };

        let Some(kw) = query_kw else {
            self.composer_sticker_selected = None;
            return Vec::new();
        };

        // Debounce alphabetic typing by 145ms before firing network requests so Vietnamese
        // Telex/VNI composition (`b-u-o-o-n-f` -> `"buồn"`) completes cleanly without API spam,
        // while already-cached keywords and pure emojis/emoticons render at 0ms.
        let already_cached = self.composer_sticker_cache.contains_key(&kw);
        if has_alpha && !already_cached {
            let elapsed = self.last_composer_edit_at.elapsed();
            let debounce = Duration::from_millis(145);
            if elapsed < debounce {
                ctx.request_repaint_after(debounce - elapsed + Duration::from_millis(8));
                return Vec::new();
            }
        }

        if !already_cached && self.composer_sticker_loading.insert(kw.clone()) {
            if let Some(tx) = WORKER_TX.get() {
                let _ = tx.send(WorkerCmd::SuggestComposerStickers {
                    keyword: kw.clone(),
                });
            }
        }

        const EXCLUDED_PACKS: &[u64] = &[
            0, 50, 10788, 11190, 11984, 11998, 12002, 12003, 12024, 12658,
        ];
        let mut out: Vec<crate::chat::StickerItem> = Vec::new();
        let mut seen = HashSet::new();

        // Prioritize live Zalo API stickers first once loaded; use curated presets as instant 0ms
        // fallback while the live query is in flight or to top up sparse results.
        if let Some(api_items) = self.composer_sticker_cache.get(&kw) {
            for it in api_items.iter().take(18) {
                if !EXCLUDED_PACKS.contains(&it.cate_id) && seen.insert(it.sticker_id) {
                    out.push(it.clone());
                }
            }
        }
        if let Some((_, preset_items)) = instant_match {
            for it in preset_items {
                if !EXCLUDED_PACKS.contains(&it.cate_id) && seen.insert(it.sticker_id) {
                    out.push(it);
                }
            }
        }
        out.truncate(16);
        out
    }

    /// Render the floating contextual sticker suggestion popover (`egui::Area`) anchored directly
    /// above `composer_rect` (matching Zalo PC's `PopoverIdentitiesDefine.STICKER_SUGGESTION`).
    pub(super) fn render_contextual_sticker_popover(
        &mut self,
        ctx: &egui::Context,
        composer_rect: Rect,
        items: &[crate::chat::StickerItem],
        thread_id: &str,
        is_group: bool,
        pal: &ThemePalette,
    ) {
        if ctx.input(|i| i.key_pressed(Key::Escape)) {
            self.dismissed_sticker_suggest_for = self.input_message.trim().to_string();
            self.composer_sticker_selected = None;
            self.composer_focus_pending = true;
            return;
        }

        let strip_h = 80.0_f32;
        let strip_w = composer_rect.width().max(200.0);
        let pos = Pos2::new(
            composer_rect.left(),
            (composer_rect.top() - strip_h).max(48.0),
        );

        let mut send_item: Option<crate::chat::StickerItem> = None;
        let mut dismiss_clicked = false;
        let mut open_search_clicked = false;
        let is_loading_live = !self.composer_sticker_loading.is_empty();
        let selected_idx = self.composer_sticker_selected;

        egui::Area::new(Id::new("zalo_contextual_sticker_popover"))
            .order(egui::Order::Foreground)
            .fixed_pos(pos)
            .show(ctx, |ui| {
                egui::Frame::none()
                    .fill(if pal.is_dark {
                        Color32::from_rgba_unmultiplied(33, 37, 43, 248)
                    } else {
                        Color32::from_rgba_unmultiplied(255, 255, 255, 250)
                    })
                    .stroke(Stroke::new(1.0_f32, pal.border))
                    .rounding(Rounding {
                        nw: 12.0,
                        ne: 12.0,
                        sw: 0.0,
                        se: 0.0,
                    })
                    .shadow(egui::epaint::Shadow {
                        offset: Vec2::new(0.0, -4.0),
                        blur: 14.0,
                        spread: 0.0,
                        color: Color32::from_black_alpha(if pal.is_dark { 95 } else { 28 }),
                    })
                    .inner_margin(Margin::symmetric(14.0, 7.0))
                    .show(ui, |ui| {
                        let inner_w = (strip_w - 28.0).max(160.0);
                        ui.set_width(inner_w);
                        ui.set_max_height(64.0);
                        ui.horizontal_top(|ui| {
                            let scroll_w = (inner_w - 64.0).max(120.0);
                            let wheel_y = ui.input(|i| i.smooth_scroll_delta.y);
                            egui::ScrollArea::horizontal()
                                .id_salt("contextual_sticker_suggest_scroll")
                                .max_width(scroll_w)
                                .max_height(64.0)
                                .scroll_bar_visibility(
                                    egui::scroll_area::ScrollBarVisibility::AlwaysHidden,
                                )
                                .auto_shrink([false, true])
                                .show(ui, |ui| {
                                    if wheel_y.abs() > 0.1
                                        && ui.rect_contains_pointer(ui.max_rect())
                                    {
                                        ui.scroll_with_delta(Vec2::new(wheel_y, 0.0));
                                    }
                                    ui.horizontal(|ui| {
                                        ui.spacing_mut().item_spacing.x = 10.0;
                                        for (idx, item) in items.iter().enumerate() {
                                            let key = format!(
                                                "stk_{}_{}",
                                                item.cate_id, item.sticker_id
                                            );
                                            let url = if item.cate_id == 0
                                                && (1..=30).contains(&item.sticker_id)
                                            {
                                                format!(
                                                    "https://stc-chat.zdn.vn/images/stickers/default/thumb/{}.png",
                                                    item.sticker_id
                                                )
                                            } else {
                                                format!(
                                                    "https://api.zaloapp.com/api/emoticon/sticker/webpc?eid={}&size=130",
                                                    item.sticker_id
                                                )
                                            };

                                            if !self.sticker_textures.contains_key(&key)
                                                && self.loading_stickers.insert(key.clone())
                                            {
                                                if let Some(tx) = WORKER_TX.get() {
                                                    let _ = tx.send(WorkerCmd::FetchStickerImage {
                                                        key: key.clone(),
                                                        url,
                                                    });
                                                }
                                            }

                                            let (cell_rect, cell_resp) = ui.allocate_exact_size(
                                                Vec2::new(62.0, 62.0),
                                                Sense::click(),
                                            );
                                            let hovered = cell_resp.hovered();
                                            let is_kb_sel = selected_idx == Some(idx);
                                            if hovered || is_kb_sel {
                                                ui.painter().rect_filled(
                                                    cell_rect,
                                                    Rounding::same(10.0),
                                                    if is_kb_sel {
                                                        pal.primary.linear_multiply(0.18)
                                                    } else {
                                                        pal.bg_hover
                                                    },
                                                );
                                                ui.painter().rect_stroke(
                                                    cell_rect,
                                                    Rounding::same(10.0),
                                                    Stroke::new(1.3_f32, pal.primary),
                                                );
                                            }

                                            if let Some(tex) = self.sticker_textures.get(&key) {
                                                let [tw, th] = tex.size();
                                                let tw_f = (tw.max(1)) as f32;
                                                let th_f = (th.max(1)) as f32;
                                                let box_side = if hovered || is_kb_sel {
                                                    58.0_f32
                                                } else {
                                                    54.0_f32
                                                };
                                                let scale =
                                                    (box_side / tw_f).min(box_side / th_f);
                                                let draw_size =
                                                    Vec2::new(tw_f * scale, th_f * scale);
                                                let img_rect = Rect::from_center_size(
                                                    cell_rect.center(),
                                                    draw_size,
                                                );
                                                ui.painter().image(
                                                    tex.id(),
                                                    img_rect,
                                                    Rect::from_min_max(
                                                        Pos2::ZERO,
                                                        Pos2::new(1.0, 1.0),
                                                    ),
                                                    Color32::WHITE,
                                                );
                                            } else {
                                                ui.painter().rect_filled(
                                                    cell_rect.shrink(6.0),
                                                    Rounding::same(8.0),
                                                    pal.bg_elevated,
                                                );
                                                ui.painter().text(
                                                    cell_rect.center(),
                                                    egui::Align2::CENTER_CENTER,
                                                    truncate_chars(&item.label, 8),
                                                    FontId::proportional(9.5),
                                                    pal.text_muted,
                                                );
                                            }

                                            if cell_resp
                                                .on_hover_cursor(CursorIcon::PointingHand)
                                                .on_hover_text(
                                                    "Bấm để gửi nhãn dán (hoặc dùng phím ↑ ← → Enter)",
                                                )
                                                .clicked()
                                            {
                                                send_item = Some(item.clone());
                                            }
                                        }
                                    });
                                });

                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                let (close_slot, close_resp) =
                                    ui.allocate_exact_size(Vec2::new(26.0, 62.0), Sense::click());
                                let close_center = close_slot.center();
                                if close_resp.hovered() {
                                    ui.painter()
                                        .circle_filled(close_center, 12.0, pal.bg_hover);
                                }
                                draw_lucide_icon(
                                    ui.painter(),
                                    close_center,
                                    lucide::X,
                                    13.5,
                                    if close_resp.hovered() {
                                        pal.text_primary
                                    } else {
                                        pal.text_muted
                                    },
                                );
                                if close_resp
                                    .on_hover_cursor(CursorIcon::PointingHand)
                                    .on_hover_text("Ẩn thanh gợi ý nhãn dán (Esc)")
                                    .clicked()
                                {
                                    dismiss_clicked = true;
                                }

                                let (more_slot, more_resp) =
                                    ui.allocate_exact_size(Vec2::new(26.0, 62.0), Sense::click());
                                let more_center = more_slot.center();
                                if more_resp.hovered() {
                                    ui.painter()
                                        .circle_filled(more_center, 12.0, pal.bg_hover);
                                }
                                if is_loading_live {
                                    ui.put(
                                        Rect::from_center_size(more_center, Vec2::new(14.0, 14.0)),
                                        egui::Spinner::new().size(13.0).color(pal.primary),
                                    );
                                } else {
                                    draw_lucide_icon(
                                        ui.painter(),
                                        more_center,
                                        lucide::SEARCH,
                                        13.0,
                                        if more_resp.hovered() {
                                            pal.primary
                                        } else {
                                            pal.text_muted
                                        },
                                    );
                                }
                                if more_resp
                                    .on_hover_cursor(CursorIcon::PointingHand)
                                    .on_hover_text("Xem thêm nhãn dán cho từ khóa này")
                                    .clicked()
                                {
                                    open_search_clicked = true;
                                }
                            });
                        });
                    });
            });

        if open_search_clicked {
            let q = crate::chat::normalize_sticker_keyword(self.input_message.trim());
            if !q.is_empty() {
                self.sticker_picker_open = true;
                self.sticker_tab = StickerTab::Search;
                self.sticker_search_query = q.clone();
                self.last_sticker_search_dispatched = q.clone();
                self.sticker_search_loading = true;
                if let Some(tx) = WORKER_TX.get() {
                    let _ = tx.send(WorkerCmd::SearchStickers { keyword: q });
                }
            }
        }
        if dismiss_clicked {
            self.dismissed_sticker_suggest_for = self.input_message.trim().to_string();
            self.composer_sticker_selected = None;
            self.composer_focus_pending = true;
        }
        if let Some(chosen) = send_item {
            self.input_message.clear();
            self.dismissed_sticker_suggest_for.clear();
            self.composer_sticker_selected = None;
            self.composer_focus_pending = true;
            if let Some(tx) = WORKER_TX.get() {
                let _ = tx.send(WorkerCmd::SendSticker {
                    to_id: thread_id.to_string(),
                    sticker_id: chosen.sticker_id,
                    cate_id: chosen.cate_id,
                    sticker_type: chosen.sticker_type,
                    is_group,
                });
            }
        }
    }

    pub(super) fn sticker_tab_salt(&self) -> u64 {
        match self.sticker_tab {
            StickerTab::Popular => 1,
            StickerTab::Onion => 2,
            StickerTab::Search => 3,
            StickerTab::Store => 4,
            StickerTab::Pack(cid) => 1000 + cid,
        }
    }
}
