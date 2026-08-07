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
mod devpanel;
mod icon;

use fan_core::history;

use eframe::egui::{
    Align2, Button, CentralPanel, Checkbox, Color32, Context, CornerRadius, FontId, Frame, Margin,
    RichText, Sense, Stroke, Vec2, ViewportBuilder,
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

/// The two fixed window sizes, ported from the WPF app: simple and developer.
const SIMPLE_SIZE: [f32; 2] = [1010.0, 660.0];
const DEV_SIZE: [f32; 2] = [1336.0, 830.0];
/// Sidebar column width, from the WPF grid (`252 | * | Auto`).
const SIDEBAR_WIDTH: f32 = 252.0;

fn main() -> eframe::Result {
    let dev = std::env::args().any(|a| a == "--dev");
    let options = eframe::NativeOptions {
        viewport: ViewportBuilder::default()
            .with_inner_size(if dev { DEV_SIZE } else { SIMPLE_SIZE })
            .with_position([100.0, 100.0])
            .with_resizable(false)
            .with_title("Fan Curves")
            // Same mark as the WPF app's window/taskbar/tray icon.
            .with_icon(icon::icon_data()),
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
            Ok(Box::new(App {
                link,
                selected: 0,
                dev,
                draft: None,
                draft_epoch: 0,
                push_pending: false,
                drag: None,
                baseline: None,
                undo: Vec::new(),
                redo: Vec::new(),
                // Timezone lookup is only safe before other threads exist.
                utc_offset_secs: time::UtcOffset::current_local_offset()
                    .map(|o| o.whole_seconds() as i64)
                    .unwrap_or(0),
                size_mode: SizeMode::Fixed,
                viewport: history::Viewport::default(),
                drag_remainder: 0.0,
            }))
        }),
    )
}

struct App {
    link: Link,
    selected: usize,
    dev: bool,
    /// The profile the UI edits. Seeded from the daemon and re-seeded whenever
    /// the daemon's copy is replaced (preset switch, reconnect) — otherwise a
    /// half-finished local edit would be clobbered every poll.
    draft: Option<fan_core::Profile>,
    draft_epoch: u64,
    push_pending: bool,
    /// Index of the curve point being dragged, across frames.
    drag: Option<usize>,
    /// Point list as it was when the current gesture started — one drag has to
    /// produce ONE undo entry, not one per mouse-move.
    baseline: Option<(usize, Vec<fan_core::CurvePoint>, String)>,
    undo: Vec<Edit>,
    redo: Vec<Edit>,
    utc_offset_secs: i64,
    size_mode: SizeMode,
    viewport: history::Viewport,
    /// Sub-sample drag leftover, so a slow drag still pans.
    drag_remainder: f32,
}

/// The app has exactly three window sizes and no drag-resize, cycled in this
/// order by the size button — whose glyph previews the NEXT one.
#[derive(Clone, Copy, PartialEq)]
enum SizeMode {
    Fixed,
    Quarter,
    Max,
}

/// Undo covers curve edits AND preset switches — a preset click overwrites
/// every curve and every response knob, so leaving it unrecoverable was the
/// thing that made the WPF app add `TuningEdit` in the first place.
enum Edit {
    Curve {
        channel: usize,
        before: Vec<fan_core::CurvePoint>,
        after: Vec<fan_core::CurvePoint>,
        before_name: String,
    },
    Tuning {
        before: fan_core::TuningSnapshot,
        after: fan_core::TuningSnapshot,
    },
}

const UNDO_CAP: usize = 100;

impl eframe::App for App {
    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        // Snapshot the shared state; the lock is held only for the copy.
        let (snap, remote_profile, remote_epoch, inventory, version) = {
            let st = self.link.state.lock().unwrap();
            (
                Snapshot::from(&st, self.selected),
                st.profile.clone(),
                st.profile_epoch,
                st.inventory.clone(),
                st.daemon_version.clone(),
            )
        };
        self.selected = snap.selected;

        // Re-seed the draft when the daemon's profile was replaced under us
        // (first connect, preset switch) — never on an ordinary status poll,
        // which would fight the user's in-flight edit.
        if self.draft.is_none() || self.draft_epoch != remote_epoch {
            if let Some(p) = remote_profile {
                self.draft = Some(p);
                self.draft_epoch = remote_epoch;
            }
        }

        self.handle_shortcuts(ctx);

        CentralPanel::default()
            .frame(Frame::new().fill(CANVAS).inner_margin(Margin::same(14)))
            .show(ctx, |ui| {
                // Layout mirrors the WPF grid: sidebar 252 | charts * | dev 300.
                self.title_bar(ui);
                ui.add_space(10.0);
                ui.horizontal_top(|ui| {
                    let full_h = ui.available_height();

                    // --- sidebar
                    ui.allocate_ui_with_layout(
                        Vec2::new(SIDEBAR_WIDTH, full_h),
                        eframe::egui::Layout::top_down(eframe::egui::Align::Min),
                        |ui| self.sidebar(ui, &snap),
                    );
                    ui.add_space(26.0);

                    // --- dev panel LAST in the row, so reserve its width now
                    let dev_w = if self.dev { devpanel::PANEL_WIDTH + 26.0 } else { 0.0 };
                    let charts_w = (ui.available_width() - dev_w).max(200.0);

                    // --- charts column: ONE card holding the channel header,
                    // the curve, the hint and the history strip (WPF layout).
                    let inner_w = charts_w - 36.0;
                    // Allocate the exact region FIRST: a right-to-left layout
                    // inside the card would otherwise claim the whole row,
                    // including the space reserved for the dev panel.
                    ui.allocate_ui_with_layout(
                        Vec2::new(charts_w, full_h),
                        eframe::egui::Layout::top_down(eframe::egui::Align::Min),
                        |ui| {
                    card_frame().show(ui, |ui| {
                        ui.set_width(inner_w);
                        ui.set_height(full_h - 34.0);
                        let strip_h = if self.dev { 156.0 } else { 0.0 };
                        let hint_h = if self.dev { 22.0 } else { 0.0 };
                        let curve_h = (full_h - strip_h - hint_h - 74.0).max(120.0);

                        let snap = self.chart_card(ui, snap, inner_w, curve_h);

                        if self.dev {
                            ui.add_space(2.0);
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new(
                                        "Drag points · double-click adds · right-click removes · \
                                         Ctrl+Z undo, Ctrl+Y redo.",
                                    )
                                    .font(FontId::proportional(10.0))
                                    .color(FAINT),
                                );
                                ui.with_layout(
                                    eframe::egui::Layout::right_to_left(eframe::egui::Align::Center),
                                    |ui| {
                                        // LIVE only appears while scrolled back.
                                        if !self.viewport.is_live()
                                            && ui
                                                .add(text_button("LIVE"))
                                                .on_hover_text("Back to now")
                                                .clicked()
                                        {
                                            self.viewport.to_live();
                                        }
                                        if ui
                                            .add(text_button("CLEAR"))
                                            .on_hover_text(
                                                "Clear the recorded history on every channel — the \
                                                 strip starts fresh from the right edge.",
                                            )
                                            .clicked()
                                        {
                                            // Daemon-side too: the daemon is
                                            // where history lives, and a
                                            // mirror-only clear would refill
                                            // on the next launch.
                                            let _ = self.link.tx.send(Cmd::ClearHistory);
                                            self.viewport.to_live();
                                        }
                                    },
                                );
                            });
                            let (rect, response) = ui.allocate_exact_size(
                                Vec2::new(inner_w, strip_h),
                                Sense::click_and_drag(),
                            );
                            let mut snap = snap;
                            snap.hover_x = response
                                .hovered()
                                .then(|| ui.input(|i| i.pointer.hover_pos()))
                                .flatten()
                                .map(|p| p.x);
                            snap.utc_offset_secs = self.utc_offset_secs;
                            self.scroll_strip(ui, &response, rect, &mut snap);
                            charts::draw_history_strip(ui.painter(), rect, &snap);
                        }
                    });
                        },
                    );

                    // --- developer panel, on the RIGHT like the WPF one, and
                    // ONE card rather than eleven stacked boxes.
                    if self.dev {
                        ui.add_space(26.0);
                        let mut edited = devpanel::Edited::default();
                        if let Some(draft) = self.draft.as_mut() {
                            ui.allocate_ui_with_layout(
                                Vec2::new(devpanel::PANEL_WIDTH, full_h),
                                eframe::egui::Layout::top_down(eframe::egui::Align::Min),
                                |ui| {
                                    card_frame().show(ui, |ui| {
                                        ui.set_width(devpanel::PANEL_WIDTH - 36.0);
                                        ui.set_height(full_h - 34.0);
                                        edited = devpanel::draw(
                                            ui,
                                            draft,
                                            self.selected,
                                            &inventory,
                                            &version,
                                        );
                                    });
                                },
                            );
                        }
                        if edited.any {
                            // A TUNING edit renames the profile "Custom" and
                            // clears the preset highlight; app-level switches
                            // deliberately do not.
                            if edited.tuning {
                                if let Some(d) = self.draft.as_mut() {
                                    d.name = "Custom".into();
                                }
                            }
                            self.push_pending = true;
                        }
                    }
                });
            });

        // One push per frame at most, and only when something actually moved.
        if self.push_pending {
            if let Some(draft) = &self.draft {
                let _ = self.link.tx.send(Cmd::Update(Box::new(draft.clone())));
            }
            self.push_pending = false;
        }
    }
}

/// Everything one frame needs, copied out of the shared state.
pub struct Snapshot {
    pub connected: bool,
    pub daemon_version: String,
    pub applying: bool,
    pub read_only: bool,
    pub simulated: bool,
    pub profile_name: String,
    pub channels: Vec<ChannelStatus>,
    pub curve: Vec<fan_core::CurvePoint>,
    pub history: Vec<client::HistorySample>,
    pub last_error: Option<String>,
    pub selected: usize,
    // Tuning the why-chip quotes back at the user, for the selected channel.
    pub slew_up: f64,
    pub slew_down: f64,
    pub hysteresis_c: f64,
    pub zero_snap_percent: f64,
    pub averaging_seconds: f64,
    /// Pointer x over the history strip, when hovering it.
    pub hover_x: Option<f32>,
    /// Local UTC offset, captured once at startup (std has no timezone data).
    pub utc_offset_secs: i64,
    pub autostart: bool,
    pub autostart_conflict: bool,
    /// False while the strip is scrolled back — the live amber dots and the
    /// counting-up stopped span are hidden then.
    pub is_live: bool,
}

impl Snapshot {
    fn from(st: &UiState, selected: usize) -> Self {
        let selected = if st.channels.is_empty() { 0 } else { selected.min(st.channels.len() - 1) };
        let ch = st.profile.as_ref().and_then(|p| p.channels.get(selected));
        Self {
            slew_up: ch.map_or(8.0, |c| c.slew_up_percent_per_sec),
            slew_down: ch.map_or(8.0, |c| c.slew_down_percent_per_sec),
            hysteresis_c: ch.map_or(3.0, |c| c.hysteresis_c),
            averaging_seconds: ch.map_or(20.0, |c| c.averaging_seconds),
            hover_x: None,
            utc_offset_secs: 0,
            autostart: st.inventory.autostart,
            autostart_conflict: st.inventory.autostart_conflict,
            zero_snap_percent: st.profile.as_ref().map_or(20.0, |p| {
                if p.zero_snap_enabled { p.zero_snap_percent } else { 0.0 }
            }),
            connected: st.connected,
            daemon_version: st.daemon_version.clone(),
            applying: st.applying,
            read_only: st.read_only,
            simulated: st.inventory.simulated,
            profile_name: st.profile_name.clone(),
            channels: st.channels.clone(),
            curve: st
                .profile
                .as_ref()
                .and_then(|p| p.channels.get(selected))
                .map(|c| c.points.clone())
                .unwrap_or_default(),
            // Filled by the caller from the viewport (needs &mut for spill reads).
            history: Vec::new(),
            is_live: true,
            last_error: st.last_error.clone(),
            selected,
        }
    }
}

impl App {
    /// Mouse editing of the selected channel's curve, plus the undo bookkeeping
    /// that turns one gesture into one entry.
    fn edit_curve(
        &mut self,
        ui: &eframe::egui::Ui,
        response: &eframe::egui::Response,
        rect: eframe::egui::Rect,
        channel: usize,
    ) {
        let Some(draft) = self.draft.as_mut() else { return };
        let Some(ch) = draft.channels.get_mut(channel) else { return };

        // Remember the pre-gesture state before the first mutation of it.
        if self.baseline.is_none() {
            self.baseline = Some((channel, ch.points.clone(), draft.name.clone()));
        }

        let edit = charts::edit_curve(ui, response, rect, &mut ch.points, &mut self.drag);
        if edit.changed {
            self.push_pending = true;
        }
        if edit.committed {
            let after = ch.points.clone();
            if let Some((base_ch, before, before_name)) = self.baseline.take() {
                if base_ch == channel && before != after {
                    // A curve edit renames the profile "Custom" and clears the
                    // redo branch, exactly like the WPF editor.
                    draft.name = "Custom".into();
                    self.undo.push(Edit::Curve { channel, before, after, before_name });
                    if self.undo.len() > UNDO_CAP {
                        self.undo.remove(0);
                    }
                    self.redo.clear();
                    self.push_pending = true;
                }
            }
        }
        // Between gestures the baseline follows the live state.
        if self.drag.is_none() && !edit.changed {
            self.baseline = None;
        }
    }

    /// Scrollback: wheel pans ~1 min a notch (10× with Shift), drag follows the
    /// cursor, double-click returns to live. Fills the snapshot's window from
    /// the viewport, which is the only place the spill file is read.
    fn scroll_strip(
        &mut self,
        ui: &eframe::egui::Ui,
        response: &eframe::egui::Response,
        rect: eframe::egui::Rect,
        snap: &mut Snapshot,
    ) {
        const NOTCH: i64 = 60; // ≈ 1 min at the ~1 s tick
        let mut st = self.link.state.lock().unwrap();
        let Some(h) = st.history.get_mut(snap.selected) else { return };

        if response.double_clicked() {
            self.viewport.to_live();
        }
        if response.hovered() {
            let (scroll, shift) = ui.input(|i| (i.raw_scroll_delta.y, i.modifiers.shift));
            if scroll != 0.0 {
                let notches = (scroll / 50.0).round().clamp(-4.0, 4.0) as i64;
                // Wheel UP goes into the past.
                self.viewport.scroll_by(-notches * NOTCH * if shift { 10 } else { 1 }, h);
            }
        }
        if response.dragged() {
            // Dragging right pulls older samples in. Keep the fractional
            // remainder so slow drags still move.
            let px_per_sample = (rect.width() - 60.0).max(1.0) / (history::RING - 1) as f32;
            self.drag_remainder += response.drag_delta().x / px_per_sample;
            let whole = self.drag_remainder.trunc();
            if whole != 0.0 {
                self.drag_remainder -= whole;
                self.viewport.scroll_by(-(whole as i64), h);
            }
        } else {
            self.drag_remainder = 0.0;
        }

        let (samples, live) = self.viewport.samples(h);
        snap.history = samples;
        snap.is_live = live;
    }

    /// The fixed size for the current mode.
    fn apply_fixed_size(&self, ctx: &Context) {
        let s = if self.dev { DEV_SIZE } else { SIMPLE_SIZE };
        ctx.send_viewport_cmd(eframe::egui::ViewportCommand::Maximized(false));
        ctx.send_viewport_cmd(eframe::egui::ViewportCommand::InnerSize(Vec2::new(s[0], s[1])));
    }

    /// Fixed → Quarter → Max → Fixed. Quarter is half the monitor in each
    /// dimension, i.e. a quarter of the screen.
    fn cycle_size(&mut self, ctx: &Context) {
        self.size_mode = match self.size_mode {
            SizeMode::Fixed => SizeMode::Quarter,
            SizeMode::Quarter => SizeMode::Max,
            SizeMode::Max => SizeMode::Fixed,
        };
        match self.size_mode {
            SizeMode::Fixed => self.apply_fixed_size(ctx),
            SizeMode::Quarter => {
                let monitor = ctx.input(|i| i.viewport().monitor_size).unwrap_or(Vec2::new(1920.0, 1080.0));
                ctx.send_viewport_cmd(eframe::egui::ViewportCommand::Maximized(false));
                ctx.send_viewport_cmd(eframe::egui::ViewportCommand::InnerSize(Vec2::new(
                    monitor.x / 2.0,
                    monitor.y / 2.0,
                )));
            }
            SizeMode::Max => ctx.send_viewport_cmd(eframe::egui::ViewportCommand::Maximized(true)),
        }
    }

    /// Adopt a preset locally (so it is undoable) and tell the daemon.
    fn adopt_preset(&mut self, name: &'static str) {
        if let Some(draft) = self.draft.as_mut() {
            let before = draft.capture_tuning();
            let preset = if name == "quiet" {
                fan_core::Profile::mac_book_like()
            } else {
                fan_core::Profile::performance()
            };
            draft.adopt_tuning(&preset);
            let after = draft.capture_tuning();
            // Re-clicking the preset you are already on must push nothing.
            if before != after {
                self.undo.push(Edit::Tuning { before, after });
                if self.undo.len() > UNDO_CAP {
                    self.undo.remove(0);
                }
                self.redo.clear();
            }
            self.baseline = None;
        }
        let _ = self.link.tx.send(Cmd::Preset(name));
    }

    /// Ctrl+Z / Ctrl+Y / Ctrl+Shift+Z.
    fn handle_shortcuts(&mut self, ctx: &Context) {
        use eframe::egui::{Key, Modifiers};
        let (undo, redo) = ctx.input(|i| {
            (
                i.key_pressed(Key::Z) && i.modifiers.matches_logically(Modifiers::CTRL),
                (i.key_pressed(Key::Y) && i.modifiers.matches_logically(Modifiers::CTRL))
                    || (i.key_pressed(Key::Z)
                        && i.modifiers.matches_logically(Modifiers::CTRL | Modifiers::SHIFT)),
            )
        });
        if undo {
            self.step_history(true);
        } else if redo {
            self.step_history(false);
        }
    }

    fn step_history(&mut self, undo: bool) {
        let Some(draft) = self.draft.as_mut() else { return };
        let entry = if undo { self.undo.pop() } else { self.redo.pop() };
        let Some(entry) = entry else { return };
        let opposite = match entry {
            Edit::Curve { channel, before, after, before_name } => {
                let (target, name) = if undo {
                    (before.clone(), before_name.clone())
                } else {
                    (after.clone(), "Custom".to_string())
                };
                if let Some(ch) = draft.channels.get_mut(channel) {
                    ch.points = target;
                }
                draft.name = name;
                self.selected = channel; // make the restore visible
                Edit::Curve { channel, before, after, before_name }
            }
            Edit::Tuning { before, after } => {
                // Re-capture first, so behaviour-slider edits made AFTER the
                // preset switch survive the round trip instead of vanishing.
                let live = draft.capture_tuning();
                draft.apply_tuning(if undo { &before } else { &after });
                if undo {
                    Edit::Tuning { before, after: live }
                } else {
                    Edit::Tuning { before: live, after }
                }
            }
        };
        if undo {
            self.redo.push(opposite);
        } else {
            self.undo.push(opposite);
        }
        self.baseline = None;
        self.push_pending = true;
    }

    /// Title bar: fan glyph, name, and the mode toggle — the WPF caption row.
    fn title_bar(&mut self, ui: &mut eframe::egui::Ui) {
        ui.horizontal(|ui| {
            // The app mark. It spins in the WPF app; here it is static, because
            // the repaint rules forbid a perpetual animation and this UI paints
            // once per engine tick.
            let (rect, _) = ui.allocate_exact_size(Vec2::splat(20.0), Sense::hover());
            icon::draw_glyph(ui.painter(), rect.center(), 20.0, Color32::from_white_alpha(217));
            ui.add_space(6.0);
            ui.label(RichText::new("Fan Curves").font(FontId::proportional(13.0)).color(TEXT));

            ui.with_layout(eframe::egui::Layout::right_to_left(eframe::egui::Align::Center), |ui| {
                // Size cycle: Fixed → Quarter → Max → Fixed, label previews next.
                let next = match self.size_mode {
                    SizeMode::Fixed => "Quarter",
                    SizeMode::Quarter => "Maximize",
                    SizeMode::Max => "Restore",
                };
                if ui
                    .add(chip_button(next))
                    .on_hover_text("Cycle the window size — the label is the next size")
                    .clicked()
                {
                    self.cycle_size(ui.ctx());
                }
                ui.add_space(8.0);
                if ui
                    .add(chip_button("Developer"))
                    .on_hover_text("Show the developer panel, the history strip and curve editing")
                    .clicked()
                {
                    self.dev = !self.dev;
                    if self.size_mode == SizeMode::Fixed {
                        self.apply_fixed_size(ui.ctx());
                    }
                }
            });
        });
    }

    /// Sidebar: hero, status chip, preset cards, pause — the WPF column order.
    fn sidebar(&mut self, ui: &mut eframe::egui::Ui, snap: &Snapshot) {
        let ch = snap.channels.get(snap.selected);

        // Hero label: which channel, and the window that actually drives it.
        let label = match ch {
            Some(c) => format!("{} · {} AVG", c.name, format_avg(snap.averaging_seconds)),
            None => "NO CHANNEL".to_string(),
        };
        ui.label(
            RichText::new(label.to_uppercase())
                .font(FontId::proportional(10.0))
                .color(FAINT),
        );

        // Hero numeral: integer big, fraction small — the rolling average.
        let avg = ch.map(|c| c.effective_temp).unwrap_or(f64::NAN);
        let avg = if avg.is_nan() { ch.and_then(|c| c.raw_temp).unwrap_or(f64::NAN) } else { avg };
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            if avg.is_nan() {
                ui.label(RichText::new("—").font(FontId::proportional(58.0)).color(TEXT));
            } else {
                let whole = avg.trunc();
                let frac = ((avg - whole) * 10.0).round().abs() as i64;
                ui.label(RichText::new(format!("{whole:.0}")).font(FontId::proportional(58.0)).color(TEXT));
                ui.label(
                    RichText::new(format!(".{frac}°"))
                        .font(FontId::proportional(26.0))
                        .color(Color32::from_white_alpha(102)),
                );
            }
        });

        // Status chip: pill + dot, the dot amber only while genuinely live.
        let (dot, text) = if !snap.connected {
            (FAINT, "Daemon not running".to_string())
        } else if !snap.applying {
            (Color32::from_white_alpha(102), "Paused — fans on the BIOS curve".to_string())
        } else if ch.map_or(true, |c| c.raw_temp.is_none()) {
            (AMBER, "No temp sensor — assign in Developer mode".to_string())
        } else if snap.simulated {
            (AMBER, "Simulation — demo data".to_string())
        } else {
            (AMBER, "Curves active".to_string())
        };
        ui.add_space(6.0);
        Frame::new()
            .fill(Color32::from_white_alpha(10))
            .stroke(Stroke::new(1.0, Color32::from_white_alpha(20)))
            .corner_radius(CornerRadius::same(255)) // pill (u8 radius, so max out)
            .inner_margin(Margin::symmetric(10, 6))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(Vec2::splat(7.0), Sense::hover());
                    ui.painter().circle_filled(r.center(), 3.5, dot);
                    ui.label(
                        RichText::new(text).font(FontId::proportional(11.0)).color(Color32::from_white_alpha(179)),
                    );
                });
            });

        ui.add_space(32.0);
        ui.label(RichText::new("P R O F I L E").font(FontId::proportional(10.0)).color(FAINT));
        ui.add_space(10.0);

        let quiet_on = snap.profile_name == "Quiet (MacBook-like)";
        let perf_on = snap.profile_name == "Performance";
        if preset_card(ui, "Quiet", "MacBook-like — silent floor, unhurried ramps", quiet_on) {
            self.adopt_preset("quiet");
        }
        ui.add_space(8.0);
        if preset_card(ui, "Performance", "Cooling first — earlier, steeper steps", perf_on) {
            self.adopt_preset("performance");
        }
        if !quiet_on && !perf_on {
            ui.add_space(8.0);
            ui.label(
                RichText::new("Custom curve — tuned by hand")
                    .font(FontId::proportional(11.0))
                    .color(Color32::from_white_alpha(102)),
            );
        }

        ui.add_space(18.0);
        let pause_label = if snap.applying { "Pause — fans to BIOS" } else { "Resume curves" };
        let button = Button::new(
            RichText::new(pause_label)
                .font(FontId::proportional(12.0))
                .color(if snap.applying { TEXT } else { Color32::from_rgb(0x0b, 0x0b, 0x0e) }),
        )
        .fill(if snap.applying { CARD } else { Color32::from_white_alpha(235) })
        .stroke(Stroke::new(1.0, HAIRLINE))
        .corner_radius(CornerRadius::same(6))
        .min_size(Vec2::new(ui.available_width(), 34.0));
        if ui.add_enabled(snap.connected, button).clicked() {
            let _ = self
                .link
                .tx
                .send(if snap.applying { Cmd::Pause } else { Cmd::Apply });
        }

        // Footer pinned to the bottom of the sidebar, with the residency
        // switch above it — the .NET app's "Start with Windows" seat.
        ui.with_layout(eframe::egui::Layout::bottom_up(eframe::egui::Align::LEFT), |ui| {
            let line = if snap.connected {
                format!(
                    "daemon v{}{}{}",
                    snap.daemon_version,
                    if snap.simulated { " · simulation" } else { "" },
                    if snap.read_only { " · profile read-only" } else { "" }
                )
            } else {
                snap.last_error.clone().unwrap_or_else(|| "daemon unreachable".into())
            };
            ui.label(RichText::new(line).font(FontId::proportional(10.0)).color(FAINT));
            ui.add_space(8.0);

            if snap.autostart_conflict && snap.autostart {
                ui.label(
                    RichText::new(
                        "Both this daemon and the .NET app start at logon — they will fight \
                         over the headers. Disable one.",
                    )
                    .font(FontId::proportional(10.0))
                    .color(AMBER),
                );
            }
            let mut on = snap.autostart;
            if ui
                .add_enabled(
                    snap.connected,
                    Checkbox::new(
                        &mut on,
                        RichText::new("Start with Windows")
                            .font(FontId::proportional(11.5))
                            .color(Color32::from_white_alpha(140)),
                    ),
                )
                .on_hover_text(
                    "Register a logon task that starts the fan daemon elevated. The daemon is \
                     the resident part — this window is on-demand and can be closed freely.",
                )
                .changed()
            {
                let _ = self.link.tx.send(Cmd::SetAutostart(on));
            }
        });
    }

    /// The chart card: channel segments + live detail in the header, then the
    /// curve editor. Returns the snapshot with any same-frame edit applied.
    fn chart_card(
        &mut self,
        ui: &mut eframe::egui::Ui,
        snap: Snapshot,
        width: f32,
        curve_h: f32,
    ) -> Snapshot {
        // Header row: segmented channel control, SIM tag, detail readout.
        ui.horizontal(|ui| {
            Frame::new()
                .fill(Color32::from_white_alpha(10))
                .stroke(Stroke::new(1.0, Color32::from_white_alpha(18)))
                .corner_radius(CornerRadius::same(11))
                .inner_margin(Margin::same(3))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        for (i, c) in snap.channels.iter().enumerate() {
                            let on = i == snap.selected;
                            let segment = match (c.effective_temp.is_nan(), c.raw_temp) {
                                (false, _) => format!("{:.0}° · {:.0}%", c.effective_temp, c.output_percent),
                                (true, Some(r)) => format!("{r:.0}° · {:.0}%", c.output_percent),
                                _ => "—".to_string(),
                            };
                            let button = Button::new(
                                RichText::new(format!("{}   {}", c.name, segment))
                                    .font(FontId::proportional(12.0))
                                    .color(if on { TEXT } else { DIM }),
                            )
                            .fill(if on { Color32::from_white_alpha(26) } else { Color32::TRANSPARENT })
                            .stroke(Stroke::NONE)
                            .corner_radius(CornerRadius::same(8));
                            if ui.add(button).clicked() {
                                self.selected = i;
                            }
                        }
                    });
                });

            if snap.simulated {
                ui.add_space(8.0);
                ui.label(RichText::new("S I M").font(FontId::proportional(9.0)).color(DIM));
            }

            // Detail line, right-aligned: raw temp (dev), rpm, preview note.
            ui.with_layout(eframe::egui::Layout::right_to_left(eframe::egui::Align::Center), |ui| {
                if let Some(c) = snap.channels.get(snap.selected) {
                    let mut parts: Vec<String> = Vec::new();
                    if c.raw_temp.is_none() {
                        parts.push("no temp sensor".into());
                    } else if self.dev {
                        parts.push(format!("now {:.1}°", c.raw_temp.unwrap_or(f64::NAN)));
                    }
                    if let Some(r) = c.rpm {
                        parts.push(format!("{r:.0} rpm"));
                    }
                    if !c.applied {
                        parts.push("preview — no fan header".into());
                    }
                    ui.label(
                        RichText::new(parts.join(" · "))
                            .font(FontId::monospace(11.5))
                            .color(DIM),
                    );
                }
            });
        });
        ui.add_space(4.0);

        // Curve chart — editable in developer mode only.
        let sense = if self.dev { Sense::click_and_drag() } else { Sense::hover() };
        let (rect, response) = ui.allocate_exact_size(Vec2::new(width, curve_h), sense);
        if self.dev {
            self.edit_curve(ui, &response, rect, snap.selected);
        }
        // An edit made this frame must be drawn this frame.
        let mut snap = snap;
        if let Some(d) = self.draft.as_ref() {
            if let Some(c) = d.channels.get(snap.selected) {
                snap.curve = c.points.clone();
            }
        }
        charts::draw_curve_chart(ui.painter(), rect, &snap);
        snap
    }
}

/// A selectable preset card: title + subtitle, ring when active.
fn preset_card(ui: &mut eframe::egui::Ui, title: &str, subtitle: &str, on: bool) -> bool {
    let width = ui.available_width();
    let response = Frame::new()
        .fill(if on { Color32::from_white_alpha(16) } else { CARD })
        .stroke(Stroke::new(1.0, if on { Color32::from_white_alpha(66) } else { HAIRLINE }))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::symmetric(12, 10))
        .show(ui, |ui| {
            ui.set_width(width - 26.0);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(title)
                        .font(FontId::proportional(13.0))
                        .color(if on { TEXT } else { DIM }),
                );
                if on {
                    ui.with_layout(
                        eframe::egui::Layout::right_to_left(eframe::egui::Align::Center),
                        |ui| {
                            // Drawn, not typed: the default font has no U+2713
                            // and renders it as a tofu box.
                            let (r, _) = ui.allocate_exact_size(Vec2::splat(12.0), Sense::hover());
                            let c = r.center();
                            let stroke = Stroke::new(1.6, TEXT);
                            ui.painter().line_segment(
                                [c + Vec2::new(-4.5, 0.0), c + Vec2::new(-1.0, 3.5)],
                                stroke,
                            );
                            ui.painter().line_segment(
                                [c + Vec2::new(-1.0, 3.5), c + Vec2::new(5.0, -3.5)],
                                stroke,
                            );
                        },
                    );
                }
            });
            ui.label(
                RichText::new(subtitle)
                    .font(FontId::proportional(11.0))
                    .color(Color32::from_white_alpha(128)),
            );
        })
        .response;
    response.interact(Sense::click()).clicked()
}

/// "45 s" / "2 min" / "1 min 30 s" — the WPF FormatAvg.
fn format_avg(secs: f64) -> String {
    let s = secs.round() as i64;
    if s < 60 {
        format!("{s} s")
    } else if s % 60 == 0 {
        format!("{} min", s / 60)
    } else {
        format!("{} min {} s", s / 60, s % 60)
    }
}

/// The app's card surface: near-black fill, hairline edge, generous radius.
fn card_frame() -> Frame {
    Frame::new()
        .fill(CARD)
        .stroke(Stroke::new(1.0, HAIRLINE))
        .corner_radius(CornerRadius::same(16))
        .inner_margin(Margin::same(16))
}

/// Bare text button (CLEAR / LIVE): no fill, no border — the WPF strip's style.
fn text_button(text: &str) -> Button<'static> {
    Button::new(RichText::new(devpanel::tracked(text)).font(FontId::proportional(9.0)).color(DIM))
        .fill(Color32::TRANSPARENT)
        .stroke(Stroke::NONE)
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
