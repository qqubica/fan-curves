//! The window/taskbar icon. The in-app title-bar glyph left with the title
//! row (2026-08-07) — the drawn geometry lives on in `fan_core::icon`.

/// The 32×32 window/taskbar icon. The pixels come from `fan_core::icon`, so
/// the UI and the tray launcher cannot drift apart.
pub fn icon_data() -> eframe::egui::IconData {
    eframe::egui::IconData {
        rgba: fan_core::icon::rgba(),
        width: fan_core::icon::SIZE as u32,
        height: fan_core::icon::SIZE as u32,
    }
}
