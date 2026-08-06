//! fan-curves on-demand UI: a separate process that talks to `fan-daemon` over
//! IPC and fully exits on close — the daemon is the only resident part.
//!
//! Design language ported from the WPF app ("precision instrument"): layered
//! near-black surfaces, monochrome white at graded opacities, ONE warm-amber
//! accent reserved strictly for live thermal state (operating dot, raw-temp
//! dashed line, status-chip dot). Repaints happen only when the poll thread
//! delivers fresh data (`request_repaint` once per engine tick) or on input —
//! no perpetual animation anywhere.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod charts;
mod client;

use eframe::egui::{
    Align2, Button, CentralPanel, Color32, Context, CornerRadius, FontId, Frame, Margin, RichText,
    Sense, Stroke, Vec2, ViewportBuilder,
};
use fan_core::{ChannelStatus, OutputReason};

use client::{Cmd, Link, UiState};

// ---- palette (Ui.cs equivalents): monochrome white at graded opacities.
// NOTE Color32 stores PREMULTIPLIED alpha — white-at-alpha must come from
// from_white_alpha (components > alpha render as additive/solid white).
// (from_white_alpha is not const — equal-component premultiplied IS white-at-alpha.)
pub const CANVAS: Color32 = Color32::from_rgb(0x0a, 0x0a, 0x0d);
pub const CARD: Color32 = Color32::from_rgb(0x11, 0x11, 0x16);
pub const HAIRLINE: Color32 = Color32::from_rgba_premultiplied(20, 20, 20, 20);
pub const TEXT: Color32 = Color32::from_rgba_premultiplied(230, 230, 230, 230);
pub const DIM: Color32 = Color32::from_rgba_premultiplied(140, 140, 140, 140);
pub const FAINT: Color32 = Color32::from_rgba_premultiplied(90, 90, 90, 90);
pub const AMBER: Color32 = Color32::from_rgb(0xff, 0x9e, 0x5e);

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: ViewportBuilder::default()
            .with_inner_size([1010.0, 660.0])
            .with_position([100.0, 100.0])
            .with_resizable(false)
            .with_title("Fan Curves"),
        ..Default::default()
    };
    eframe::run_native(
        "Fan Curves",
        options,
        Box::new(|cc| {
            // Monochrome + amber only: egui's default blue selection/accents are
            // off-palette. Selection becomes a faint white wash.
            let mut visuals = eframe::egui::Visuals::dark();
            visuals.selection.bg_fill = Color32::from_white_alpha(22);
            visuals.selection.stroke = Stroke::new(1.0, HAIRLINE);
            visuals.hyperlink_color = TEXT;
            visuals.panel_fill = CANVAS;
            cc.egui_ctx.set_visuals(visuals);
            let link = client::start(cc.egui_ctx.clone());
            Ok(Box::new(App { link, selected: 0 }))
        }),
    )
}

struct App {
    link: Link,
    selected: usize,
}

impl eframe::App for App {
    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        // Snapshot the shared state; the lock is held only for the copy.
        let snap: Snapshot = {
            let st = self.link.state.lock().unwrap();
            Snapshot::from(&*st, self.selected)
        };
        self.selected = snap.selected;

        CentralPanel::default()
            .frame(Frame::new().fill(CANVAS).inner_margin(Margin::same(14)))
            .show(ctx, |ui| {
                self.top_bar(ui, &snap);
                ui.add_space(10.0);
                ui.horizontal_top(|ui| {
                    self.left_column(ui, &snap);
                    ui.add_space(12.0);
                    ui.vertical(|ui| {
                        let w = ui.available_width();
                        let curve_h = ui.available_height() - 176.0;
                        let (rect, _) =
                            ui.allocate_exact_size(Vec2::new(w, curve_h), Sense::hover());
                        charts::draw_curve_chart(ui.painter(), rect, &snap);
                        ui.add_space(10.0);
                        let (rect, _) =
                            ui.allocate_exact_size(Vec2::new(w, 160.0), Sense::hover());
                        charts::draw_history_strip(ui.painter(), rect, &snap);
                    });
                });
            });
    }
}

/// Everything one frame needs, copied out of the shared state.
pub struct Snapshot {
    pub connected: bool,
    pub daemon_version: String,
    pub applying: bool,
    pub read_only: bool,
    pub profile_name: String,
    pub channels: Vec<ChannelStatus>,
    pub curve: Vec<fan_core::CurvePoint>,
    pub history: Vec<client::HistorySample>,
    pub last_error: Option<String>,
    pub selected: usize,
}

impl Snapshot {
    fn from(st: &UiState, selected: usize) -> Self {
        let selected = if st.channels.is_empty() { 0 } else { selected.min(st.channels.len() - 1) };
        Self {
            connected: st.connected,
            daemon_version: st.daemon_version.clone(),
            applying: st.applying,
            read_only: st.read_only,
            profile_name: st.profile_name.clone(),
            channels: st.channels.clone(),
            curve: st
                .profile
                .as_ref()
                .and_then(|p| p.channels.get(selected))
                .map(|c| c.points.clone())
                .unwrap_or_default(),
            history: st.history.get(selected).map(|r| r.iter().copied().collect()).unwrap_or_default(),
            last_error: st.last_error.clone(),
            selected,
        }
    }
}

impl App {
    fn top_bar(&mut self, ui: &mut eframe::egui::Ui, snap: &Snapshot) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("F A N   C U R V E S").font(FontId::proportional(12.0)).color(DIM));
            ui.add_space(14.0);
            // Status chip: the dot is the ONLY amber here, and only when live.
            let (dot, text) = if !snap.connected {
                (FAINT, "OFFLINE".to_string())
            } else if snap.applying {
                (AMBER, format!("DRIVING · {}", snap.profile_name))
            } else {
                (DIM, format!("PAUSED · {}", snap.profile_name))
            };
            let (rect, _) = ui.allocate_exact_size(Vec2::splat(8.0), Sense::hover());
            ui.painter().circle_filled(rect.center(), 3.0, dot);
            ui.label(RichText::new(text).font(FontId::proportional(11.0)).color(TEXT));

            ui.with_layout(eframe::egui::Layout::right_to_left(eframe::egui::Align::Center), |ui| {
                let on = snap.connected;
                if snap.applying {
                    if ui.add_enabled(on, chip_button("PAUSE · BIOS CONTROL")).clicked() {
                        let _ = self.link.tx.send(Cmd::Pause);
                    }
                } else if ui.add_enabled(on, chip_button("APPLY CURVES")).clicked() {
                    let _ = self.link.tx.send(Cmd::Apply);
                }
                ui.add_space(10.0);
                if ui.add_enabled(on, chip_button("PERFORMANCE")).clicked() {
                    let _ = self.link.tx.send(Cmd::Preset("performance"));
                }
                if ui.add_enabled(on, chip_button("QUIET · MACBOOK-LIKE")).clicked() {
                    let _ = self.link.tx.send(Cmd::Preset("quiet"));
                }
                ui.label(RichText::new("PRESETS").font(FontId::proportional(10.0)).color(FAINT));
            });
        });
    }

    fn left_column(&mut self, ui: &mut eframe::egui::Ui, snap: &Snapshot) {
        ui.vertical(|ui| {
            ui.set_width(230.0);

            let ch = snap.channels.get(snap.selected);

            // Hero numeral: the rolling average — the thing that drives the steps.
            let avg = ch.map(|c| c.effective_temp).unwrap_or(f64::NAN);
            let hero = if avg.is_nan() { "--".to_string() } else { format!("{avg:.1}") };
            ui.label(RichText::new(hero).font(FontId::proportional(64.0)).color(TEXT));
            ui.label(RichText::new("AVERAGE  °C").font(FontId::proportional(10.0)).color(FAINT));
            ui.add_space(14.0);

            for (i, c) in snap.channels.iter().enumerate() {
                let selected = i == snap.selected;
                let label = RichText::new(&c.name).font(FontId::proportional(13.0)).color(if selected {
                    TEXT
                } else {
                    DIM
                });
                if ui.selectable_label(selected, label).clicked() {
                    self.selected = i;
                }
            }
            ui.add_space(14.0);

            if let Some(c) = ch {
                readout(ui, "now", &c.raw_temp.map_or("--".into(), |t| format!("{t:.1} °C")));
                readout(ui, "fan", &format!("{:.0} %", c.output_percent));
                readout(ui, "target", &format!("{:.0} %", c.target_percent));
                readout(ui, "rpm", &c.rpm.map_or("--".into(), |r| format!("{r:.0}")));
                ui.add_space(10.0);
                let why = describe(c);
                if !why.is_empty() {
                    ui.label(RichText::new(why).font(FontId::proportional(11.0)).color(DIM));
                }
            }

            ui.with_layout(eframe::egui::Layout::bottom_up(eframe::egui::Align::LEFT), |ui| {
                let line = if snap.connected {
                    format!("daemon v{} · simulation{}", snap.daemon_version, if snap.read_only { " · read-only" } else { "" })
                } else {
                    snap.last_error.clone().unwrap_or_else(|| "daemon unreachable".into())
                };
                ui.label(RichText::new(line).font(FontId::proportional(10.0)).color(FAINT));
            });
        });
    }
}

fn chip_button(text: &str) -> Button<'_> {
    Button::new(RichText::new(text).font(FontId::proportional(10.5)).color(TEXT))
        .fill(CARD)
        .stroke(Stroke::new(1.0, HAIRLINE))
        .corner_radius(CornerRadius::same(4))
}

fn readout(ui: &mut eframe::egui::Ui, label: &str, value: &str) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(format!("{label:<7}")).font(FontId::monospace(11.5)).color(FAINT));
        ui.label(RichText::new(value).font(FontId::monospace(11.5)).color(TEXT));
    });
}

/// Why-chip vocabulary (same phrases as the WPF chip / behavior log).
pub fn describe(s: &ChannelStatus) -> String {
    match s.reason {
        OutputReason::RampUp => format!("ramping up to {:.0}%", s.target_percent),
        OutputReason::RampDown => format!("ramping down to {:.0}%", s.target_percent),
        OutputReason::StepDownHold => {
            format!("step down to {:.0}% in {:.0} s", s.reason_level, s.reason_seconds)
        }
        OutputReason::Hysteresis => "hysteresis hold".into(),
        OutputReason::ZeroSnap => format!("stopped — zero snap (curve asks {:.0}%)", s.reason_level),
        OutputReason::MinFloor => format!("safety floor (curve asks {:.0}%)", s.reason_level),
        OutputReason::IdleKick => "idle kick".into(),
        OutputReason::StopProbe => "trial stop — is this fan needed?".into(),
        OutputReason::None => String::new(),
    }
}

// Re-exported for charts.rs text drawing.
pub use eframe::egui::Painter;
pub fn micro(painter: &Painter, pos: eframe::egui::Pos2, anchor: Align2, text: &str, color: Color32) {
    painter.text(pos, anchor, text, FontId::proportional(10.0), color);
}
