//! THE 3D-TO-2D REAL-TIME DESKTOP MIRROR PIPELINE.
//!
//! Native platform screen capture is *simulated*, per specification: a painter
//! thread ("phantasm-mirror") renders a dynamic fake desktop into a fresh
//! pixel buffer at 30 Hz and pushes it through an mpsc channel as an
//! `Arc<MirrorFrame>` — the atomically-refcounted, lock-free "texture pointer".
//! The producer never mutates a frame after sending (fresh buffer per frame),
//! and the render thread drains latest-wins, so the main rendering loop can
//! never stall on the capture side.
//!
//! The ghostly "notepad.exe" replica window is composited INTO the pixel
//! payload here — before the texture is uploaded and mapped onto the 3D
//! tablet mesh — so the haunt is literally inside the mirror image.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::canvas::{PixelCanvas, TinyRng};

pub const MIRROR_W: usize = 640;
pub const MIRROR_H: usize = 360;
const TICK: Duration = Duration::from_millis(33);
const TICK_DT: f32 = 1.0 / 30.0;

/// One frame of the mirrored desktop. Fixed dimensions (MIRROR_W x MIRROR_H)
/// so the GPU-side texture is created exactly once and only its contents move.
pub struct MirrorFrame {
    pub rgba: Vec<u8>,
}

/// Commands flowing event-loop -> painter thread.
pub enum MirrorCommand {
    SetName(String),
    BeginTerminal,
    TerminalChar(char),
    TerminalBackspace,
    TerminalSubmitted,
    BeginWatching,
    BeginRush,
    BeginGoodbye,
    SetGlitch(f32),
}

/// Spawns the capture worker. Drift-corrected 30 Hz cadence; exits within one
/// tick of the shutdown flag flipping.
pub fn spawn_capture_pipeline(
    cmd_rx: mpsc::Receiver<MirrorCommand>,
    frame_tx: mpsc::Sender<Arc<MirrorFrame>>,
    shutdown: Arc<AtomicBool>,
) -> JoinHandle<()> {
    thread::Builder::new()
        .name("phantasm-mirror-capture".into())
        .spawn(move || {
            let mut painter = DesktopPainter::new();
            let mut next = Instant::now() + TICK;
            while !shutdown.load(Ordering::Relaxed) {
                while let Ok(cmd) = cmd_rx.try_recv() {
                    painter.command(cmd);
                }
                let frame = Arc::new(MirrorFrame { rgba: painter.paint() });
                let _ = frame_tx.send(frame);
                let now = Instant::now();
                if now < next {
                    thread::sleep(next - now);
                }
                next += TICK;
            }
        })
        .expect("failed to spawn the mirror capture thread")
}

enum Act {
    Boot,
    Terminal,
    Watching,
    Rush,
    Goodbye,
}

struct DesktopPainter {
    name: String,
    act: Act,
    t: f32,
    blink: f32,
    glitch: f32,
    rng: TinyRng,
    writer: GhostWriter,
    terminal: String,
}

impl DesktopPainter {
    fn new() -> Self {
        DesktopPainter {
            name: "OPERATOR".to_string(),
            act: Act::Boot,
            t: 0.0,
            blink: 0.0,
            glitch: 0.0,
            rng: TinyRng::new(0xPHA5M1 as u32 ^ 0xC0FFEE),
            writer: GhostWriter::new(),
            terminal: String::new(),
        }
    }

    fn command(&mut self, cmd: MirrorCommand) {
        match cmd {
            MirrorCommand::SetName(n) => self.name = n,
            MirrorCommand::BeginTerminal => {
                self.act = Act::Terminal;
                self.t = 0.0;
            }
            MirrorCommand::TerminalChar(c) => {
                if self.terminal.chars().count() < 16 {
                    self.terminal.push(c);
                }
            }
            MirrorCommand::TerminalBackspace => {
                self.terminal.pop();
            }
            MirrorCommand::TerminalSubmitted => self.enter_watching(),
            MirrorCommand::BeginWatching => self.enter_watching(),
            MirrorCommand::BeginRush => {
                self.act = Act::Rush;
                self.t = 0.0;
                self.glitch = self.glitch.max(0.35);
                self.writer.set_script(vec![
                    "IT IS TOO LATE.".to_string(),
                    "BEHIND YOU.".to_string(),
                ]);
            }
            MirrorCommand::BeginGoodbye => {
                self.act = Act::Goodbye;
                self.t = 0.0;
                self.glitch = 1.0;
                let msg = format!(
                    "THIS IS MINE NOW.\nYOU ARE UNNECESSARY.\nGOODBYE, {}.",
                    self.name.to_uppercase()
                );
                self.writer.set_single_held(msg);
            }
            MirrorCommand::SetGlitch(g) => self.glitch = g,
        }
    }

    fn enter_watching(&mut self) {
        self.act = Act::Watching;
        self.t = 0.0;
        self.writer.set_script(vec![
            format!("hello, {}.", self.name.to_lowercase()),
            "i can see through your desktop.".to_string(),
            "do not look away.".to_string(),
        ]);
    }

    fn blink_on(&self) -> bool {
        (self.blink * 2.4).floor() as i32 % 2 == 0
    }

    fn paint(&mut self) -> Vec<u8> {
        self.t += TICK_DT;
        self.blink += TICK_DT;
        self.writer.tick(TICK_DT, &mut self.rng);
        if matches!(self.act, Act::Rush) {
            self.glitch = (self.glitch + TICK_DT * 0.15).min(1.0);
        }

        let mut c = PixelCanvas::new(MIRROR_W, MIRROR_H);
        match self.act {
            Act::Boot => self.paint_boot(&mut c),
            Act::Terminal => self.paint_terminal(&mut c),
            Act::Watching | Act::Rush => self.paint_desktop(&mut c),
            Act::Goodbye => self.paint_goodbye(&mut c),
        }
        if self.glitch > 0.01 {
            self.apply_glitch(&mut c);
        }
        c.into_buf()
    }

    // ------------------------------------------------------------------
    // Act painters
    // ------------------------------------------------------------------

    fn paint_boot(&self, c: &mut PixelCanvas) {
        c.fill([5, 4, 6, 255]);
        centered(c, 62, "PHANTASM", [255, 32, 48, 255], 4);
        centered(c, 118, "DESKTOP MIRROR", [140, 140, 150, 255], 2);
        centered(c, 190, "WARNING: SUDDEN LOUD AUDIO.", [235, 200, 70, 255], 1);
        centered(c, 204, "WARNING: VIOLENT WINDOW MOTION.", [235, 200, 70, 255], 1);
        centered(c, 218, "HEADPHONES ARE A BAD IDEA.", [235, 200, 70, 255], 1);
        if self.blink_on() {
            centered(c, 284, "PRESS ANY KEY TO BEGIN", [240, 240, 245, 255], 2);
        }
    }

    fn paint_terminal(&self, c: &mut PixelCanvas) {
        c.fill([4, 6, 4, 255]);
        let green = [70, 255, 140, 255];
        c.draw_text(24, 26, "PHANTASM BIOS v0.9.7", green, 2);
        c.draw_text(24, 54, "NO OPERATOR PROFILE FOUND ON THIS MACHINE.", green, 1);
        c.draw_text(24, 68, "LOCAL RECONNAISSANCE FAILED. FALLING BACK", green, 1);
        c.draw_text(24, 82, "TO MANUAL IDENTIFICATION.", green, 1);
        let prompt = format!("> ENTER YOUR NAME: {}", self.terminal);
        c.draw_text(24, 116, &prompt, green, 2);
        if self.blink_on() {
            let cx = 24 + PixelCanvas::text_width(&prompt, 2) + 2;
            c.fill_rect(cx, 116, 10, 15, [70, 255, 140, 255]);
        }
        // CRT scanlines
        for y in (0..MIRROR_H).step_by(3) {
            c.invert_region_dither(y);
        }
    }

    fn paint_desktop(&mut self, c: &mut PixelCanvas) {
        // wallpaper: vertical gradient + vignette + sparse living static
        let top = [22.0f32, 20.0, 28.0];
        let bot = [7.0f32, 7.0, 12.0];
        for y in 0..MIRROR_H {
            let k = y as f32 / (MIRROR_H - 1) as f32;
            let r = top[0] + (bot[0] - top[0]) * k;
            let g = top[1] + (bot[1] - top[1]) * k;
            let b = top[2] + (bot[2] - top[2]) * k;
            for x in 0..MIRROR_W {
                let dx = x as f32 / MIRROR_W as f32 - 0.5;
                let dy = y as f32 / MIRROR_H as f32 - 0.5;
                let vig = (1.0 - (dx * dx + dy * dy) * 1.1).max(0.0);
                if self.rng.next_f32() < 0.003 {
                    let n = (self.rng.next_f32() * 36.0) as u8;
                    c.set(x as i32, y as i32, [n, n, n + 6, 255]);
                } else {
                    c.set(
                        x as i32,
                        y as i32,
                        [(r * vig) as u8, (g * vig) as u8, (b * vig) as u8, 255],
                    );
                }
            }
        }

        // desktop icons
        let icons = [
            ("MY FILES", [235, 235, 240, 255]),
            ("RECYCLING", [235, 235, 240, 255]),
            ("DOORS", [235, 235, 240, 255]),
            ("PHANTASM", [255, 60, 70, 255]),
        ];
        for (i, (label, label_col)) in icons.iter().enumerate() {
            let x = 18;
            let y = 16 + (i as i32) * 58;
            c.fill_rect(x, y, 30, 24, [88, 96, 118, 255]);
            c.fill_rect(x + 3, y + 3, 24, 18, [138, 150, 180, 255]);
            c.draw_text(x - 2, y + 28, label, *label_col, 1);
        }

        // background fake windows
        self.draw_window(c, 190, 44, 290, 170, "system properties - display");
        self.draw_window(c, 350, 150, 220, 120, "C:\\system32 - DO NOT OPEN");

        // ---- THE GHOSTLY NOTEPAD: composited directly onto the texture
        //      payload before it is mapped onto the 3D tablet mesh. ----
        if self.writer.is_awake() {
            let text = self.writer.visible();
            let nx = ((MIRROR_W as i32) - 320) / 2;
            let ny = 66;
            c.fill_rect(nx, ny, 320, 200, [216, 216, 214, 235]);
            c.fill_rect(nx, ny, 320, 18, [58, 74, 122, 245]);
            c.fill_rect(nx, ny + 18, 320, 1, [10, 10, 12, 255]);
            c.draw_text(nx + 6, ny + 5, "untitled - Notepad", [235, 235, 245, 255], 1);
            for i in 0..3 {
                c.fill_rect(nx + 320 - 17 - i * 15, ny + 4, 12, 11, [190, 190, 196, 255]);
            }
            c.draw_text(nx + 14, ny + 34, &text, [24, 24, 28, 255], 1);
            if self.blink_on() {
                let lines: Vec<&str> = text.split('\n').collect();
                let last = lines.last().copied().unwrap_or("");
                let cx = nx + 14 + PixelCanvas::text_width(last, 1) + 1;
                let cy = ny + 34 + ((lines.len().max(1) - 1) * 8) as i32;
                c.fill_rect(cx, cy, 5, 8, [20, 20, 26, 255]);
            }
        }

        // taskbar
        let tb = MIRROR_H as i32 - 30;
        c.fill_rect(0, tb, MIRROR_W as i32, 30, [22, 22, 28, 255]);
        c.fill_rect(0, tb - 1, MIRROR_W as i32, 1, [60, 60, 70, 255]);
        c.fill_rect(6, tb + 5, 24, 20, [40, 40, 50, 255]);
        for gy in 0..2 {
            for gx in 0..2 {
                c.fill_rect(9 + gx * 9, tb + 8 + gy * 9, 7, 7, [225, 228, 235, 255]);
            }
        }
        let (hh, mm) = wall_clock_utc();
        let clock = format!("{:02}:{:02}", hh, mm);
        c.draw_text(MIRROR_W as i32 - 58, tb + 10, &clock, [225, 225, 232, 255], 1);
    }

    fn draw_window(&self, c: &mut PixelCanvas, x: i32, y: i32, w: i32, h: i32, title: &str) {
        c.fill_rect(x, y, w, h, [30, 30, 38, 255]);
        c.fill_rect(x, y, w, 18, [56, 56, 70, 255]);
        c.fill_rect(x, y + 18, w, 1, [10, 10, 12, 255]);
        c.draw_text(x + 6, y + 5, title, [210, 210, 220, 255], 1);
        c.fill_rect(x + w - 16, y + 4, 12, 11, [70, 70, 84, 255]);
    }

    fn paint_goodbye(&mut self, c: &mut PixelCanvas) {
        c.fill([4, 2, 3, 255]);
        let text = self.writer.visible();
        let longest = text.lines().map(|l| l.chars().count()).max().unwrap_or(0);
        let scale = if longest <= 34 { 3 } else { 2 };
        c.draw_text(40, 56, &text, [255, 26, 40, 255], scale);
        // boiling static floor
        for _ in 0..(MIRROR_W * MIRROR_H / 110) {
            let x = self.rng.next_range(MIRROR_W) as i32;
            let y = self.rng.next_range(MIRROR_H) as i32;
            let v = self.rng.next_range(70) as u8 + 25;
            c.set(x, y, [v, v / 2, v / 2, 255]);
        }
        if self.blink_on() {
            c.draw_text(
                40,
                300,
                "THIS TERMINAL NO LONGER BELONGS TO YOU",
                [130, 28, 34, 255],
                1,
            );
        }
    }

    // ------------------------------------------------------------------
    // Glitch post-process, applied to the finished pixel payload.
    // ------------------------------------------------------------------
    fn apply_glitch(&mut self, c: &mut PixelCanvas) {
        let g = self.glitch.min(1.0);
        let bands = 3 + (g * 15.0) as i32;
        for _ in 0..bands {
            let y = self.rng.next_range(MIRROR_H) as i32;
            let rows = 3 + self.rng.next_range(22) as i32;
            let dx = (self.rng.next_bipolar() * 22.0 * g).round() as i32;
            if dx != 0 {
                c.shift_rows(y, rows, dx);
            }
        }
        if self.rng.next_f32() < g * 0.35 {
            let y = self.rng.next_range(MIRROR_H) as i32;
            c.invert_region(y, 4 + self.rng.next_range(10) as i32);
        }
        if self.rng.next_f32() < g * 0.5 {
            let y = self.rng.next_range(MIRROR_H) as i32;
            c.fill_rect(0, y, MIRROR_W as i32, 2, [0, 0, 0, 255]);
        }
    }
}

/// Extra canvas helper for terminal scanlines (a light dithered darken).
impl PixelCanvas {
    pub fn invert_region_dither(&mut self, y: i32) {
        if y < 0 || y >= self.h as i32 {
            return;
        }
        let row_start = y as usize * self.w * 4;
        for x in (0..self.w).step_by(2) {
            let i = row_start + x * 4;
            self.buf[i] = (self.buf[i] as f32 * 0.9) as u8;
            self.buf[i + 1] = (self.buf[i + 1] as f32 * 0.9) as u8;
            self.buf[i + 2] = (self.buf[i + 2] as f32 * 0.9) as u8;
        }
    }
}

// ----------------------------------------------------------------------
// The ghost writer — types, holds, deletes. The uncanny cadence of someone
// (something) at the other end of the notepad.
// ----------------------------------------------------------------------
enum WMode {
    Sleeping,
    Typing,
    Holding,
    Deleting,
}

struct GhostWriter {
    mode: WMode,
    script: std::collections::VecDeque<String>,
    current: String,
    shown: usize,
    next: f32,
    hold_forever: bool,
}

impl GhostWriter {
    fn new() -> Self {
        GhostWriter {
            mode: WMode::Sleeping,
            script: std::collections::VecDeque::new(),
            current: String::new(),
            shown: 0,
            next: 2.0,
            hold_forever: false,
        }
    }

    fn set_script(&mut self, msgs: Vec<String>) {
        self.script = msgs.into();
        self.hold_forever = false;
        self.mode = WMode::Sleeping;
        self.next = 1.6;
    }

    fn set_single_held(&mut self, msg: String) {
        self.script.clear();
        self.current = msg;
        self.shown = 0;
        self.next = 0.9;
        self.hold_forever = true;
        self.mode = WMode::Typing;
    }

    fn tick(&mut self, dt: f32, rng: &mut TinyRng) {
        match self.mode {
            WMode::Sleeping => {
                self.next -= dt;
                if self.next <= 0.0 {
                    if let Some(m) = self.script.pop_front() {
                        self.current = m;
                        self.shown = 0;
                        self.next = 0.0;
                        self.mode = WMode::Typing;
                    } else {
                        self.next = 1.0;
                    }
                }
            }
            WMode::Typing => {
                self.next -= dt;
                let total = self.current.chars().count();
                if self.next <= 0.0 && self.shown < total {
                    self.shown += 1;
                    self.next = 0.045 + rng.next_f32() * 0.14; // irregular, human-ish, wrong
                    if self.shown >= total {
                        self.next = if self.hold_forever { f32::MAX } else { 2.6 };
                        self.mode = WMode::Holding;
                    }
                }
            }
            WMode::Holding => {
                self.next -= dt;
                if self.next <= 0.0 {
                    self.mode = WMode::Deleting;
                    self.next = 0.0;
                }
            }
            WMode::Deleting => {
                self.next -= dt;
                while self.next <= 0.0 && self.shown > 0 {
                    self.shown -= 1;
                    self.next += 0.018;
                }
                if self.shown == 0 {
                    self.mode = WMode::Sleeping;
                    self.next = 1.8 + rng.next_f32() * 1.5;
                }
            }
        }
    }

    fn visible(&self) -> String {
        self.current.chars().take(self.shown).collect()
    }

    fn is_awake(&self) -> bool {
        !matches!(self.mode, WMode::Sleeping)
    }
}

fn centered(c: &mut PixelCanvas, y: i32, text: &str, color: [u8; 4], scale: i32) {
    let w = PixelCanvas::text_width(text, scale);
    c.draw_text((MIRROR_W as i32 - w) / 2, y, text, color, scale);
}

/// Wall clock (UTC — the machine does not care about your time zone).
fn wall_clock_utc() -> (u32, u32) {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    (((secs / 3600) % 24) as u32, ((secs / 60) % 60) as u32)
}
