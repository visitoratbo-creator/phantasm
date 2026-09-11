//! DESKTOP MIRROR: PHANTASM — bootstrap, OS event loop, and the kill-switch sequence.
//!
//! Thread topology:
//!   [main / event loop]  game state, winit windows, wgpu rendering, mirror drain
//!   [phantasm-mirror]    simulated desktop capture painter (producer), 30 Hz cadence
//!   [rodio internal]     pulls samples out of the DreadMixer (consumer)
//! Command flow: event loop --(AudioCommand)--> audio mailbox
//!               event loop --(MirrorCommand)--> painter thread
//! Data flow:    painter --(Arc<MirrorFrame>)--> mpsc --> render thread (latest-wins)

mod audio;
mod canvas;
mod font;
mod gfx;
mod identity;
mod math;
mod mirror;
mod sprite;
mod state;
mod window_fx;

use std::sync::mpsc;
use std::sync::Arc;
use std::time::Instant;

use state::{EngineController, GameState, Phase};
use winit::event::{Event, WindowEvent};
use winit::event_loop::EventLoopBuilder;
use winit::window::Window;
use window_fx::{MonsterLayer, ShakeOscillator};

fn main() {
    // The only console output the haunting ever produces — informed consent first.
    println!("DESKTOP MIRROR: PHANTASM");
    println!("This game reads your account name locally, produces SUDDEN LOUD AUDIO,");
    println!("flashing imagery and VIOLENT WINDOW MOTION. Nothing leaves this process.");
    println!("ESC terminates immediately.\n");

    let controller = Arc::new(EngineController::new());

    // ------------------------------------------------------------------
    // The Sonic Dread Engine. rodio owns the device; we own every sample.
    // ------------------------------------------------------------------
    let (audio, audio_stream, audio_sink) = audio::init_audio();

    // ------------------------------------------------------------------
    // Identity reconnaissance — layers 1 & 2 here, layer 3 (terminal)
    // lives inside the game loop in state.rs.
    // ------------------------------------------------------------------
    let identity = identity::resolve_from_environment()
        .or_else(identity::resolve_from_profile_metadata);

    // ------------------------------------------------------------------
    // The 3D-to-2D desktop-mirror pipeline (producer thread -> channel).
    // ------------------------------------------------------------------
    let (frame_tx, frame_rx) = mpsc::channel::<Arc<mirror::MirrorFrame>>();
    let (mirror_tx, mirror_rx) = mpsc::channel::<mirror::MirrorCommand>();
    let capture_thread = mirror::spawn_capture_pipeline(mirror_rx, frame_tx, controller.shutdown_flag());
    controller.register_thread("phantasm-mirror-capture", capture_thread);

    // ------------------------------------------------------------------
    // Window + GPU.
    // ------------------------------------------------------------------
    let event_loop = EventLoopBuilder::<()>::new()
        .build()
        .expect("failed to start the OS event loop");
    let window_attrs = Window::default()
        .with_title("Desktop Mirror: Phantasm")
        .with_inner_size(winit::dpi::LogicalSize::new(1280.0, 720.0));
    let main_window = Arc::new(
        event_loop
            .create_window(window_attrs)
            .expect("failed to open the main window"),
    );

    let (gpu, main_surface) = gfx::Gpu::for_window(&main_window);
    let mut main_renderer = gfx::MainRenderer::new(&gpu, &main_window, main_surface);

    let mut game = GameState::new(identity, mirror_tx, audio);

    let mut monster: Option<MonsterLayer> = None;
    let mut shake = ShakeOscillator::new();
    let mut last = Instant::now();
    let mut exit_requested = false;

    let _ = event_loop.run(move |event, elwt| {
        if exit_requested {
            return;
        }
        match event {
            Event::AboutToWait => {
                elwt.set_control_flow(winit::event_loop::ControlFlow::Poll);
                let now = Instant::now();
                let dt = (now - last).as_secs_f32().min(0.05);
                last = now;

                game.update(dt);

                // ---- the fourth wall breaks here ----
                if game.wants_monster && monster.is_none() {
                    let name = game
                        .identity
                        .as_ref()
                        .map(|i| i.display_name.clone())
                        .unwrap_or_else(|| "PLAYER".to_string());
                    monster = Some(MonsterLayer::spawn(elwt, &gpu, &main_window, &name));
                    game.wants_monster = false;
                }
                if let Some(title) = game.take_title() {
                    main_window.set_title(&title);
                }

                // ---- chaotic physical window shaking on the player's real monitor ----
                if shake.base.is_none() {
                    if let Ok(p) = main_window.outer_position() {
                        shake.base = Some(p);
                    }
                }
                if let Some((dx, dy)) = shake.advance(dt, game.shake_magnitude()) {
                    if let Some(base) = shake.base {
                        main_window.set_outer_position(winit::dpi::PhysicalPosition::new(
                            base.x + dx,
                            base.y + dy,
                        ));
                    }
                }

                // ---- latest-wins drain of the mirror frames. The render thread
                //      never blocks on the producer: stale frames are discarded
                //      and only the freshest Arc<MirrorFrame> reaches the GPU. ----
                let mut latest_frame: Option<Arc<mirror::MirrorFrame>> = None;
                while let Ok(f) = frame_rx.try_recv() {
                    latest_frame = Some(f);
                }
                if let Some(f) = latest_frame {
                    main_renderer.upload_mirror_frame(&f);
                }

                main_window.request_redraw();
                if let Some(m) = monster.as_mut() {
                    m.tick(dt);
                    m.window.request_redraw();
                }

                if game.pending_exit {
                    exit_requested = true;
                    elwt.exit();
                }
            }
            Event::WindowEvent { window_id, event } => {
                if window_id == main_window.id() {
                    match event {
                        WindowEvent::CloseRequested => {
                            exit_requested = true;
                            elwt.exit();
                        }
                        WindowEvent::KeyboardInput { event: key_event, .. } => {
                            game.handle_key(&key_event.logical_key, key_event.state);
                        }
                        WindowEvent::Focused(false) => {
                            // META-SCARE: it notices when you look away mid-approach.
                            if game.phase == Phase::Rush {
                                game.escalate_on_focus_loss();
                            }
                        }
                        WindowEvent::Resized(size) => {
                            main_renderer.resize(size.width.max(1), size.height.max(1));
                        }
                        WindowEvent::RedrawRequested => {
                            main_renderer.render(game.total_t, game.glitch);
                        }
                        _ => {}
                    }
                } else if let Some(m) = monster.as_ref() {
                    if window_id == m.window.id() {
                        match event {
                            WindowEvent::RedrawRequested => {
                                m.render(game.total_t, m.current_frame());
                            }
                            WindowEvent::CloseRequested => {
                                // It cannot be closed. That is the point.
                            }
                            _ => {}
                        }
                    }
                }
            }
            _ => {}
        }
    });

    // ------------------------------------------------------------------
    // GRACEFUL KILL-SWITCH SEQUENCE (single code path for every exit route):
    //   (1) silence the audio FIRST — never leave a screech on a dead engine,
    //   (2) flip the atomic flags every worker thread polls,
    //   (3) join all worker threads deterministically,
    //   (4) drop the winit/wgpu/rodio resources in scope order.
    // No recursion, no unbounded stacks, no orphaned device contexts.
    // ------------------------------------------------------------------
    let joined = controller.finalize(&audio_sink);
    drop(audio_stream);
    println!("[phantasm] engine terminated cleanly — {} worker thread(s) joined.", joined);
}
