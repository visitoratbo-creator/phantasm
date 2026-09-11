//! Global game state, the entity tracker, the phase machine, and the
//! EngineController — the kill-switch heart of the process.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use winit::event::ElementState;
use winit::keyboard::{Key, NamedKey};

use crate::audio::{AudioCommand, AudioHandle};
use crate::identity::PlayerIdentity;
use crate::math::Vec3;
use crate::mirror::MirrorCommand;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Phase {
    /// Consent gate. Nothing personal happens until a key is pressed.
    Boot,
    /// In-game terminal fallback for identity acquisition.
    Terminal,
    /// Calm investigation. The mirror learns your name.
    Watching,
    /// The entity is coming. Rising distortion, mild shaking.
    Rush,
    /// The Glitched Window Snap has fired. Everything, at once.
    Seizure,
    /// Shutdown requested.
    Terminated,
}

/// One tracked entity in the fiction. Positions are in "corridor space":
/// the listener sits near the origin; the Rush entity approaches from outside.
#[derive(Clone, Copy, Debug)]
pub struct Entity {
    pub label: &'static str,
    pub pos: Vec3,
    pub vel: Vec3,
    pub aggression: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct EntityTracker {
    pub listener: Vec3,
    pub rush: Entity,
    pub ghost: Entity,
    pub events_fired: u32,
}

impl EntityTracker {
    pub fn new() -> Self {
        EntityTracker {
            listener: Vec3::new(0.0, 1.0, 1.7),
            rush: Entity {
                label: "RUSH",
                pos: Vec3::new(-9.0, 0.5, -7.0),
                vel: Vec3::new(0.0, 0.0, 0.035),
                aggression: 0.0,
            },
            ghost: Entity {
                label: "MIRROR_GHOST",
                pos: Vec3::new(2.5, 1.2, -1.0),
                vel: Vec3::ZERO,
                aggression: 0.15,
            },
            events_fired: 0,
        }
    }

    /// Advances the fiction: the Rush entity closes on the listener, the
    /// notepad ghost drifts lazily nearer, and positional rumbles fire at
    /// scripted bearings so the stereo image walks around the player's head.
    pub fn update(&mut self, dt: f32, watching_t: f32, audio: &AudioHandle) {
        let dir = self.listener.sub(self.rush.pos);
        if dir.len() > 0.001 {
            let step = dir.normalized().scale(self.rush.vel.len() * dt * 60.0);
            self.rush.pos = self.rush.pos.add(step);
        }
        self.rush.aggression = (self.rush.aggression + dt * 0.08).min(1.0);
        self.ghost.pos = self.ghost.pos.lerp(self.listener, 0.0006);

        match self.events_fired {
            0 if watching_t > 3.5 => {
                audio.send(AudioCommand::CorridorRumble { pan: -0.85 });
                self.events_fired = 1;
            }
            1 if watching_t > 7.5 => {
                audio.send(AudioCommand::CorridorRumble { pan: 0.9 });
                self.events_fired = 2;
            }
            2 if watching_t > 11.0 => {
                // Dead centre: equal power on both ears — it is behind you.
                audio.send(AudioCommand::CorridorRumble { pan: 0.0 });
                self.events_fired = 3;
            }
            _ => {}
        }
    }
}

pub struct GameState {
    pub phase: Phase,
    pub phase_t: f32,
    pub total_t: f32,
    pub identity: Option<PlayerIdentity>,
    pub terminal: String,
    pub tracker: EntityTracker,
    pub glitch: f32,
    pub wants_monster: bool,
    pub pending_exit: bool,
    title_request: Option<String>,
    audio: AudioHandle,
    mirror: mpsc::Sender<MirrorCommand>,
}

impl GameState {
    pub fn new(
        identity: Option<PlayerIdentity>,
        mirror: mpsc::Sender<MirrorCommand>,
        audio: AudioHandle,
    ) -> Self {
        GameState {
            phase: Phase::Boot,
            phase_t: 0.0,
            total_t: 0.0,
            identity,
            terminal: String::new(),
            tracker: EntityTracker::new(),
            glitch: 0.0,
            wants_monster: false,
            pending_exit: false,
            title_request: None,
            audio,
            mirror,
        }
    }

    pub fn elapsed(&self) -> f32 {
        self.total_t
    }

    pub fn take_title(&mut self) -> Option<String> {
        self.title_request.take()
    }

    /// Window-shake amplitude in physical pixels, derived from the phase.
    pub fn shake_magnitude(&self) -> f32 {
        match self.phase {
            Phase::Rush => 4.0 + self.phase_t * 3.5,
            Phase::Seizure => 26.0,
            _ => 0.0,
        }
    }

    pub fn update(&mut self, dt: f32) {
        self.total_t += dt;
        self.phase_t += dt;
        match self.phase {
            Phase::Boot | Phase::Terminal | Phase::Terminated => {}
            Phase::Watching => {
                self.tracker.update(dt, self.phase_t, &self.audio);
                if self.phase_t > 13.0 {
                    self.enter_rush();
                }
            }
            Phase::Rush => {
                self.glitch = (self.glitch + dt * 0.28).min(1.0);
                let _ = self.mirror.send(MirrorCommand::SetGlitch(self.glitch));
                if self.phase_t > 5.0 {
                    self.enter_seizure();
                }
            }
            Phase::Seizure => {
                if self.phase_t > 8.0 {
                    self.enter_terminated();
                }
            }
        }
    }

    fn set_phase(&mut self, p: Phase) {
        self.phase = p;
        self.phase_t = 0.0;
    }

    fn enter_rush(&mut self) {
        // Transition: positional 3D corridor rumbles give way to the
        // volume-normalised mono overdrive channel of the Rush entity.
        self.audio.send(AudioCommand::TriggerRush { duration_ms: 5200 });
        let _ = self.mirror.send(MirrorCommand::BeginRush);
        let _ = self.mirror.send(MirrorCommand::SetGlitch(0.35));
        self.set_phase(Phase::Rush);
    }

    fn enter_seizure(&mut self) {
        crate::glitched_window_snap!(self.audio, 4200);
        let _ = self.mirror.send(MirrorCommand::BeginGoodbye);
        self.wants_monster = true;
        let name = self
            .identity
            .as_ref()
            .map(|i| i.display_name.clone())
            .unwrap_or_else(|| "PLAYER".to_string());
        self.title_request = Some(format!("GOODBYE, {}", name.to_uppercase()));
        self.glitch = 1.0;
        self.set_phase(Phase::Seizure);
    }

    fn enter_terminated(&mut self) {
        self.audio.send(AudioCommand::FadeOut);
        self.set_phase(Phase::Terminated);
        self.pending_exit = true;
    }

    /// Looking away during the Rush act escalates immediately. It knows.
    pub fn escalate_on_focus_loss(&mut self) {
        if self.phase == Phase::Rush {
            self.enter_seizure();
        }
    }

    pub fn handle_key(&mut self, key: &Key, state: ElementState) {
        if state == ElementState::Released {
            return;
        }
        // ESC: unconditional, immediate, clean exit. The safety valve.
        if matches!(key, Key::Named(NamedKey::Escape)) {
            self.pending_exit = true;
            return;
        }
        match self.phase {
            Phase::Boot => {
                // Any key begins — the consent gate. Identity layers 1 & 2 either
                // resolved a name, or we drop into the in-game terminal (layer 3).
                match self.identity.clone() {
                    Some(id) => {
                        println!("[phantasm] identity acquired: \"{}\" via {}", id.display_name, id.source);
                        let _ = self.mirror.send(MirrorCommand::SetName(id.display_name.clone()));
                        let _ = self.mirror.send(MirrorCommand::BeginWatching);
                        self.audio.send(AudioCommand::Begin);
                        self.set_phase(Phase::Watching);
                    }
                    None => {
                        let _ = self.mirror.send(MirrorCommand::BeginTerminal);
                        self.set_phase(Phase::Terminal);
                    }
                }
            }
            Phase::Terminal => match key {
                Key::Character(s) => {
                    for ch in s.chars() {
                        if self.terminal.chars().count() < 16
                            && (ch.is_ascii_graphic() || ch == ' ')
                        {
                            self.terminal.push(ch);
                            let _ = self.mirror.send(MirrorCommand::TerminalChar(ch));
                        }
                    }
                }
                Key::Named(NamedKey::Backspace) => {
                    if self.terminal.pop().is_some() {
                        let _ = self.mirror.send(MirrorCommand::TerminalBackspace);
                    }
                }
                Key::Named(NamedKey::Enter) => self.submit_terminal(),
                _ => {}
            },
            Phase::Watching => {
                // SPACE skips ahead to the approach — demo valve.
                if matches!(key, Key::Named(NamedKey::Space)) {
                    self.enter_rush();
                }
            }
            _ => {}
        }
    }

    fn submit_terminal(&mut self) {
        let raw = std::mem::take(&mut self.terminal);
        let name = crate::identity::sanitize(&raw).unwrap_or_else(|| "PLAYER".to_string());
        println!("[phantasm] identity acquired: \"{}\" via terminal fallback", name);
        self.identity = Some(PlayerIdentity {
            display_name: name.clone(),
            source: crate::identity::NameSource::TerminalPrompt,
        });
        let _ = self.mirror.send(MirrorCommand::SetName(name));
        let _ = self.mirror.send(MirrorCommand::TerminalSubmitted);
        self.audio.send(AudioCommand::Begin);
        self.set_phase(Phase::Watching);
    }
}

/// The kill-switch. Owns the global shutdown flag and the worker-thread
/// registry so shutdown is a *sequence*, not an accident.
pub struct EngineController {
    shutdown: Arc<AtomicBool>,
    threads: Mutex<Vec<(String, JoinHandle<()>)>>,
}

impl EngineController {
    pub fn new() -> Self {
        EngineController {
            shutdown: Arc::new(AtomicBool::new(false)),
            threads: Mutex::new(Vec::new()),
        }
    }

    pub fn shutdown_flag(&self) -> Arc<AtomicBool> {
        self.shutdown.clone()
    }

    pub fn register_thread(&self, name: &str, handle: JoinHandle<()>) {
        self.threads.lock().unwrap().push((name.to_string(), handle));
    }

    pub fn begin_shutdown(&self) {
        self.shutdown.store(true, Ordering::SeqCst);
    }

    /// Graceful termination. Ordering matters:
    ///   audio sink first (never leak the screech), then flags, then joins.
    /// Joins are bounded because every worker polls the flag on a <=33ms cadence.
    pub fn finalize(&self, sink: &rodio::Sink) -> usize {
        sink.stop();
        self.begin_shutdown();
        let drained: Vec<(String, JoinHandle<()>)> =
            std::mem::take(&mut *self.threads.lock().unwrap());
        let n = drained.len();
        for (name, handle) in drained {
            match handle.join() {
                Ok(()) => {}
                Err(_) => eprintln!("[phantasm] worker '{}' panicked during shutdown", name),
            }
        }
        n
    }
}
