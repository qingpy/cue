use eframe::egui::{Context, FontData, FontDefinitions, FontFamily};

/// Inter (embedded) as the UI font, with a system CJK font appended as
/// fallback so nothing heavy is baked into the exe.
pub fn install(ctx: &Context) {
    let mut fonts = FontDefinitions::default();

    fonts.font_data.insert(
        "inter".into(),
        FontData::from_static(include_bytes!("../assets/Inter-Regular.ttf")).into(),
    );
    fonts.families.entry(FontFamily::Proportional).or_default().insert(0, "inter".into());

    // Consolas for code: egui's bundled monospace renders larger than Inter
    if let Ok(mono) = std::fs::read(r"C:\Windows\Fonts\consola.ttf") {
        fonts.font_data.insert("mono".into(), FontData::from_owned(mono).into());
        fonts.families.entry(FontFamily::Monospace).or_default().insert(0, "mono".into());
    }

    let candidates = [
        r"C:\Windows\Fonts\msyh.ttc",
        r"C:\Windows\Fonts\Deng.ttf",
        r"C:\Windows\Fonts\simhei.ttf",
    ];
    if let Some(bytes) = candidates.iter().find_map(|p| std::fs::read(p).ok()) {
        fonts.font_data.insert("cjk".into(), FontData::from_owned(bytes).into());
        for fam in [FontFamily::Proportional, FontFamily::Monospace] {
            fonts.families.entry(fam).or_default().push("cjk".into());
        }
    }
    ctx.set_fonts(fonts);
}
