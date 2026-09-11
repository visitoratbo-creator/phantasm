//! THE SPRITE FOUNDRY — the red monster and its farewell text, rasterised
//! procedurally into a 4x2 sheet of 256x256 frames. No asset files: the thing
//! is *born* inside the binary, which is thematically appropriate.

use crate::canvas::{PixelCanvas, TinyRng};

pub const FRAME_PX: usize = 256;
pub const SHEET_COLS: usize = 4;
pub const SHEET_ROWS: usize = 2;
pub const FRAME_COUNT: usize = SHEET_COLS * SHEET_ROWS;

pub const GOODBYE_W: usize = 480;
pub const GOODBYE_H: usize = 160;

/// 8-frame wave cycle on a 1024x512 RGBA sheet.
pub fn generate_monster_sheet() -> Vec<u8> {
    let mut sheet = PixelCanvas::new(FRAME_PX * SHEET_COLS, FRAME_PX * SHEET_ROWS);
    for f in 0..FRAME_COUNT {
        draw_frame(&mut sheet, f);
    }
    sheet.into_buf()
}

fn draw_frame(sheet: &mut PixelCanvas, f: usize) {
    let ox = (f % SHEET_COLS) * FRAME_PX;
    let oy = (f / SHEET_COLS) * FRAME_PX;
    let mut rng = TinyRng::new(0x5EED_0001u32.wrapping_add(f as u32));

    // The whole body jitters a pixel between frames: stop-motion wrongness.
    let jx = (rng.next_bipolar() * 1.4).round() as i32;
    let jy = (rng.next_bipolar() * 1.4).round() as i32;
    let ox = ox as i32 + jx;
    let oy = oy as i32 + jy;

    let body_dark = [96, 8, 18, 255];
    let body = [176, 26, 38, 255];
    let body_hi = [214, 44, 52, 255];

    // legs
    sheet.fill_rect(ox + 104, oy + 198, 22, 34, body_dark);
    sheet.fill_rect(ox + 136, oy + 198, 22, 34, body_dark);

    // left arm — hangs dead
    for i in 0..5 {
        let t = i as f32 / 4.0;
        let px = ox as f32 + 68.0 - t * 10.0;
        let py = oy as f32 + 150.0 + t * 46.0;
        sheet.fill_circle(px, py, 9.0 - t * 2.0, body_dark);
    }

    // waving arm — the whole point. Sine-driven claw sweeping a ~80-degree arc.
    let phase = (f as f32 / FRAME_COUNT as f32) * std::f32::consts::TAU;
    let theta = 0.45 + 0.8 * phase.sin();
    let shoulder = (ox as f32 + 182.0, oy as f32 + 148.0);
    let hand = (
        shoulder.0 + theta.sin() * 60.0,
        shoulder.1 - theta.cos() * 60.0,
    );
    for i in 0..6 {
        let t = i as f32 / 5.0;
        let px = shoulder.0 + (hand.0 - shoulder.0) * t;
        let py = shoulder.1 + (hand.1 - shoulder.1) * t;
        sheet.fill_circle(px, py, 10.0 - t * 2.0, body);
    }
    sheet.fill_circle(hand.0, hand.1, 11.0, body_hi);
    for k in -1i32..=1 {
        let fx = hand.0 + k as f32 * 9.0;
        let fy = hand.1 - 9.0 + (k * k) as f32 * 3.0;
        sheet.fill_circle(fx, fy, 4.5, body_hi);
    }

    // torso
    sheet.fill_circle(ox as f32 + 128.0, oy as f32 + 148.0, 64.0, body_dark);
    sheet.fill_circle(ox as f32 + 124.0, oy as f32 + 140.0, 56.0, body);
    sheet.fill_circle(ox as f32 + 118.0, oy as f32 + 132.0, 34.0, body_hi);

    // horns
    sheet.fill_triangle(
        ox as f32 + 92.0, oy as f32 + 100.0,
        ox as f32 + 104.0, oy as f32 + 62.0,
        ox as f32 + 112.0, oy as f32 + 100.0,
        body_dark,
    );
    sheet.fill_triangle(
        ox as f32 + 144.0, oy as f32 + 100.0,
        ox as f32 + 152.0, oy as f32 + 62.0,
        ox as f32 + 164.0, oy as f32 + 100.0,
        body_dark,
    );

    // eyes — hollow white, pupils drifting to keep watching *you*
    let look = phase.cos() * 2.5;
    for ex in [104.0f32, 152.0] {
        sheet.fill_circle(ox as f32 + ex, oy as f32 + 122.0, 13.5, [255, 255, 255, 90]);
        sheet.fill_circle(ox as f32 + ex, oy as f32 + 122.0, 9.5, [250, 250, 250, 255]);
        sheet.fill_circle(ox as f32 + ex + look, oy as f32 + 123.0, 4.5, [12, 6, 8, 255]);
    }

    // the grin: a void, with teeth
    sheet.fill_rect(ox + 94, oy + 156, 70, 22, [10, 4, 6, 255]);
    sheet.fill_circle(ox as f32 + 94.0, oy as f32 + 167.0, 11.0, [10, 4, 6, 255]);
    sheet.fill_circle(ox as f32 + 164.0, oy as f32 + 167.0, 11.0, [10, 4, 6, 255]);
    for i in 0..6 {
        let tx = ox as f32 + 98.0 + i as f32 * 12.0;
        sheet.fill_triangle(
            tx, oy as f32 + 157.0,
            tx + 9.0, oy as f32 + 157.0,
            tx + 4.5, oy as f32 + 170.0,
            [240, 236, 230, 255],
        );
    }
    for i in 0..5 {
        let tx = ox as f32 + 102.0 + i as f32 * 12.0;
        sheet.fill_triangle(
            tx, oy as f32 + 177.0,
            tx + 9.0, oy as f32 + 177.0,
            tx + 4.5, oy as f32 + 164.0,
            [240, 236, 230, 255],
        );
    }

    // film-grain rot
    for _ in 0..900 {
        let x = ox + rng.next_range(FRAME_PX) as i32;
        let y = oy + rng.next_range(FRAME_PX) as i32;
        sheet.set(x, y, [0, 0, 0, 60]);
    }
}

/// The farewell texture: "THIS IS MINE NOW. YOU ARE UNNECESSARY. GOODBYE, [name]."
/// in red with a drop shadow, rendered into a 480x160 RGBA buffer.
pub fn generate_goodbye_texture(name: &str) -> Vec<u8> {
    let mut c = PixelCanvas::new(GOODBYE_W, GOODBYE_H);
    let l3 = format!("GOODBYE, {}.", name.to_uppercase());
    let lines = ["THIS IS MINE NOW.", "YOU ARE UNNECESSARY.", l3.as_str()];
    let red = [255, 24, 36, 255];
    let shadow = [0, 0, 0, 220];
    let mut y = 38;
    for line in &lines {
        let w = PixelCanvas::text_width(line, 2);
        let x = (GOODBYE_W as i32 - w) / 2;
        c.draw_text(x + 2, y + 2, line, shadow, 2);
        c.draw_text(x, y, line, red, 2);
        y += 26;
    }
    c.into_buf()
}
