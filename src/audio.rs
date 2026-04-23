//! Audio output, music streaming, and onset tracking.
//!
//! Output backend: the legacy `waveOut*` family from `winmm.dll`.
//! Music codec: QOA, whole file decoded at level entry.
//!
//! Architecture:
//!
//! * A dedicated worker thread owns the entire audio subsystem.
//! * Worker publishes (atomically):
//!     - sample clock,
//!     - music playback position / duration (for the UI panel),
//!     - paused flag,
//!     - "music is loaded" flag,
//!     - an `onset_counter` that increments on every detected
//!       kick drum hit in the currently playing track,
//!     - `onset_phase_q24`, normalized time since the last onset
//!       (0 at the onset itself, reaching 1 after ~500 ms).
//! * Main thread publishes:
//!     - per SFX trigger counters (atomic fetch_add),
//!     - a FIFO of music commands (Play / Stop / Pause / Seek),
//!     - master volume,
//!     - legacy `bpm` (still supported, but only a fallback:
//!       onset based pulses take precedence when a track is
//!       loaded).
//! * A short linear fade envelope is applied to music samples at
//!   every transition so track swaps do not click.
//!
//! # Volume mix
//!
//! This revision splits the single master gain into three
//! independent attenuations applied in the worker before the
//! final soft-clip:
//!
//!   output = ((music * music_volume) + (sfx * sfx_volume))
//!            * master_volume
//!
//! Each channel uses its own atomic so the menu can wire the
//! three sliders (`master`, `music`, `sfx`) from the config file
//! straight through without locking anything.

use std::ffi::c_void;
use std::ptr;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use crate::music::MusicTrack;

const SAMPLE_RATE:     u32   = 44100;
const CHANNELS:        u16   = 2;
const BITS_PER_SAMPLE: u16   = 16;
const BUFFER_FRAMES:   usize = 1024;
const BUFFER_BYTES:    usize = BUFFER_FRAMES * (CHANNELS as usize) * 2;
const NUM_BUFFERS:     usize = 3;

/// Linear fade envelope increment per audio sample. 1/8820 means
/// a full 0 to 1 ramp takes ~200 ms at 44.1 kHz.
const FADE_STEP: f32 = 1.0 / 8820.0;

/// Q16 scale for the published `music_position_q16` /
/// `music_duration_q16` atomics.
const MUSIC_TIME_Q16_SCALE: u64 = 65536;

/// Time window over which the pulse decays, in seconds. After
/// this many seconds past the last onset the published phase
/// saturates at 1.0.
const ONSET_DECAY_WINDOW_SEC: f64 = 0.50;

pub const SFX_TOUCH:    usize = 0;
pub const SFX_INTERACT: usize = 1;
pub const SFX_ENTER:    usize = 2;
pub const SFX_EXIT:     usize = 3;
const SFX_COUNT: usize = 4;

#[allow(non_snake_case)]
#[repr(C)]
struct WAVEFORMATEX {
    wFormatTag: u16, nChannels: u16, nSamplesPerSec: u32,
    nAvgBytesPerSec: u32, nBlockAlign: u16,
    wBitsPerSample: u16, cbSize: u16,
}

#[allow(non_snake_case)]
#[repr(C)]
struct WAVEHDR {
    lpData: *mut i8, dwBufferLength: u32, dwBytesRecorded: u32,
    dwUser: usize, dwFlags: u32, dwLoops: u32,
    lpNext: *mut WAVEHDR, reserved: usize,
}

type HWAVEOUT = *mut c_void;
type HANDLE   = *mut c_void;

const WAVE_MAPPER:     u32 = 0xFFFFFFFF;
const WAVE_FORMAT_PCM: u16 = 0x0001;
const CALLBACK_EVENT:  u32 = 0x00050000;
const WHDR_DONE:       u32 = 0x00000001;

#[link(name = "winmm")]
unsafe extern "system" {
    fn waveOutOpen(phwo: *mut HWAVEOUT, uDeviceID: u32,
        pwfx: *const WAVEFORMATEX, dwCallback: usize,
        dwInstance: usize, fdwOpen: u32) -> u32;
    fn waveOutClose(hwo: HWAVEOUT) -> u32;
    fn waveOutPrepareHeader(hwo: HWAVEOUT, pwh: *mut WAVEHDR, cbwh: u32) -> u32;
    fn waveOutUnprepareHeader(hwo: HWAVEOUT, pwh: *mut WAVEHDR, cbwh: u32) -> u32;
    fn waveOutWrite(hwo: HWAVEOUT, pwh: *mut WAVEHDR, cbwh: u32) -> u32;
    fn waveOutReset(hwo: HWAVEOUT) -> u32;
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn CreateEventW(lpEventAttributes: *mut c_void, bManualReset: i32,
        bInitialState: i32, lpName: *const u16) -> HANDLE;
    fn CloseHandle(hObject: HANDLE) -> i32;
    fn WaitForSingleObject(hHandle: HANDLE, dwMilliseconds: u32) -> u32;
}

#[derive(Clone)]
enum MusicCommand {
    Play {
        path: String,
        force_restart: bool,
        /// Source-stream offset to seek to the moment the new
        /// track is installed. Used by `restart_music_at` so a
        /// debug `#[startfrom]` directive lands on the desired
        /// timestamp atomically with the swap-in, instead of
        /// racing against the async decoder.
        start_seconds: f32,
    },
    Stop,
    SetPaused(bool),
    Seek(f32),
}

struct Shared {
    sfx_requested: [AtomicU32; SFX_COUNT],
    volume_q8:     AtomicU32,
    /// Music sub-channel gain. Q8, 0..=255 mapped to 0.0..=1.0.
    music_volume_q8: AtomicU32,
    /// SFX sub-channel gain. Q8, 0..=255 mapped to 0.0..=1.0.
    sfx_volume_q8:   AtomicU32,

    sample_pos:    AtomicU64,
    bpm:           AtomicU32,
    stop:          AtomicBool,

    music_cmds:    Mutex<Vec<MusicCommand>>,

    music_position_q16: AtomicU64,
    music_duration_q16: AtomicU64,
    music_is_paused:    AtomicBool,
    music_is_loaded:    AtomicBool,

    /// Counter incremented once per detected kick drum onset in
    /// the currently playing track. Game / menu code watches this
    /// for change to know "a new beat just fired".
    onset_counter: AtomicU32,
    /// Normalized time since the last onset, Q24 in 0..=1.
    /// `0` immediately after an onset, saturates at `1 << 24`
    /// after `ONSET_DECAY_WINDOW_SEC`.
    onset_phase_q24: AtomicU32,
}

pub struct Audio {
    shared: Arc<Shared>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Audio {
    pub fn new() -> Self {
        let shared = Arc::new(Shared {
            sfx_requested: [
                AtomicU32::new(0), AtomicU32::new(0),
                AtomicU32::new(0), AtomicU32::new(0),
            ],
            volume_q8:       AtomicU32::new(180),
            music_volume_q8: AtomicU32::new(255),
            sfx_volume_q8:   AtomicU32::new(255),
            sample_pos:     AtomicU64::new(0),
            bpm:            AtomicU32::new(128),
            stop:           AtomicBool::new(false),
            music_cmds:     Mutex::new(Vec::new()),
            music_position_q16: AtomicU64::new(0),
            music_duration_q16: AtomicU64::new(0),
            music_is_paused:    AtomicBool::new(false),
            music_is_loaded:    AtomicBool::new(false),
            onset_counter:      AtomicU32::new(0),
            onset_phase_q24:    AtomicU32::new(1 << 24),
        });
        let s2 = shared.clone();
        let thread = thread::spawn(move || audio_worker(s2));
        Audio { shared, thread: Some(thread) }
    }

    pub fn play_touch(&self)    { self.shared.sfx_requested[SFX_TOUCH].fetch_add(1, Ordering::Relaxed); }
    pub fn play_interact(&self) { self.shared.sfx_requested[SFX_INTERACT].fetch_add(1, Ordering::Relaxed); }
    pub fn play_enter(&self)    { self.shared.sfx_requested[SFX_ENTER].fetch_add(1, Ordering::Relaxed); }
    pub fn play_exit(&self)     { self.shared.sfx_requested[SFX_EXIT].fetch_add(1, Ordering::Relaxed); }

    pub fn play_music(&self, path: &str) {
        self.push_cmd(MusicCommand::Play {
            path: path.to_string(),
            force_restart: false,
            start_seconds: 0.0,
        });
    }

    pub fn restart_music(&self, path: &str) {
        self.push_cmd(MusicCommand::Play {
            path: path.to_string(),
            force_restart: true,
            start_seconds: 0.0,
        });
    }

    /// Restart `path` and seek to `start_seconds` atomically:
    /// the seek is applied at the moment the freshly-decoded
    /// track is installed, not when the seek command is
    /// received. Eliminates the race that made
    /// `restart_music` + `seek_music` produce a half-second of
    /// the old track's audio at the new offset before the new
    /// track took over from zero.
    pub fn restart_music_at(&self, path: &str, start_seconds: f32) {
        self.push_cmd(MusicCommand::Play {
            path: path.to_string(),
            force_restart: true,
            start_seconds: start_seconds.max(0.0),
        });
    }

    pub fn stop_music(&self)  { self.push_cmd(MusicCommand::Stop); }
    pub fn pause_music(&self) { self.push_cmd(MusicCommand::SetPaused(true)); }
    pub fn resume_music(&self){ self.push_cmd(MusicCommand::SetPaused(false)); }
    pub fn set_music_paused(&self, paused: bool) {
        self.push_cmd(MusicCommand::SetPaused(paused));
    }
    pub fn seek_music(&self, seconds: f32) {
        self.push_cmd(MusicCommand::Seek(seconds));
    }

    fn push_cmd(&self, cmd: MusicCommand) {
        if let Ok(mut g) = self.shared.music_cmds.lock() {
            g.push(cmd);
        }
    }

    pub fn music_position(&self) -> f32 {
        let q = self.shared.music_position_q16.load(Ordering::Relaxed);
        q as f32 / MUSIC_TIME_Q16_SCALE as f32
    }

    pub fn music_duration(&self) -> f32 {
        let q = self.shared.music_duration_q16.load(Ordering::Relaxed);
        q as f32 / MUSIC_TIME_Q16_SCALE as f32
    }

    pub fn is_music_paused(&self) -> bool {
        self.shared.music_is_paused.load(Ordering::Relaxed)
    }

    pub fn has_music(&self) -> bool {
        self.shared.music_is_loaded.load(Ordering::Relaxed)
    }

    /// Onset counter. Game code snapshots this on one frame and
    /// compares to the previous snapshot to detect "a new kick
    /// just fired". Wraps naturally at u32, which the consumer
    /// handles with `!=` comparisons, not arithmetic.
    pub fn onset_counter(&self) -> u32 {
        self.shared.onset_counter.load(Ordering::Relaxed)
    }

    /// Normalized [0, 1] phase since the last onset. Drives the
    /// decaying pulse visual. 0 at the hit, 1 after the decay
    /// window has elapsed.
    pub fn onset_phase(&self) -> f32 {
        let q = self.shared.onset_phase_q24.load(Ordering::Relaxed) as f32;
        q / (1u32 << 24) as f32
    }

    /// Global post-mix master gain.
    pub fn set_volume(&self, v: f32) {
        let q = (v.clamp(0.0, 1.0) * 255.0) as u32;
        self.shared.volume_q8.store(q, Ordering::Relaxed);
    }

    /// Pre-mix gain applied to the music stream only. Kept
    /// independent from `set_volume` so a user who wants
    /// "music quiet, SFX loud" can dial them separately.
    pub fn set_music_volume(&self, v: f32) {
        let q = (v.clamp(0.0, 1.0) * 255.0) as u32;
        self.shared.music_volume_q8.store(q, Ordering::Relaxed);
    }

    /// Pre-mix gain applied to every SFX voice. Same rationale
    /// as `set_music_volume`.
    pub fn set_sfx_volume(&self, v: f32) {
        let q = (v.clamp(0.0, 1.0) * 255.0) as u32;
        self.shared.sfx_volume_q8.store(q, Ordering::Relaxed);
    }

    pub fn set_bpm(&self, bpm: u32) {
        self.shared.bpm.store(bpm.clamp(40, 300), Ordering::Relaxed);
    }

    pub fn shutdown(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            t.join().ok();
        }
    }
}

impl Drop for Audio {
    fn drop(&mut self) { self.shutdown(); }
}

fn audio_worker(shared: Arc<Shared>) {
    unsafe {
        let event = CreateEventW(ptr::null_mut(), 0, 0, ptr::null());
        if event.is_null() { return; }
        let fmt = WAVEFORMATEX {
            wFormatTag: WAVE_FORMAT_PCM,
            nChannels: CHANNELS,
            nSamplesPerSec: SAMPLE_RATE,
            nAvgBytesPerSec: SAMPLE_RATE * CHANNELS as u32 * (BITS_PER_SAMPLE / 8) as u32,
            nBlockAlign: CHANNELS * (BITS_PER_SAMPLE / 8),
            wBitsPerSample: BITS_PER_SAMPLE,
            cbSize: 0,
        };
        let mut hwo: HWAVEOUT = ptr::null_mut();
        let r = waveOutOpen(&mut hwo, WAVE_MAPPER, &fmt, event as usize, 0, CALLBACK_EVENT);
        if r != 0 || hwo.is_null() {
            CloseHandle(event);
            return;
        }
        let mut buffers: Vec<Vec<i16>> = (0..NUM_BUFFERS)
            .map(|_| vec![0i16; BUFFER_FRAMES * CHANNELS as usize]).collect();
        let mut headers: Vec<WAVEHDR> = (0..NUM_BUFFERS).map(|i| WAVEHDR {
            lpData: buffers[i].as_mut_ptr() as *mut i8,
            dwBufferLength: BUFFER_BYTES as u32,
            dwBytesRecorded: 0, dwUser: 0, dwFlags: 0, dwLoops: 0,
            lpNext: ptr::null_mut(), reserved: 0,
        }).collect();
        let mut state = SynthState::new();
        let hdr_size = std::mem::size_of::<WAVEHDR>() as u32;
        for i in 0..NUM_BUFFERS {
            state.fill_buffer(&shared, &mut buffers[i]);
            waveOutPrepareHeader(hwo, &mut headers[i], hdr_size);
            waveOutWrite(hwo, &mut headers[i], hdr_size);
        }
        loop {
            if shared.stop.load(Ordering::Relaxed) { break; }
            WaitForSingleObject(event, 100);
            for i in 0..NUM_BUFFERS {
                if (headers[i].dwFlags & WHDR_DONE) != 0 {
                    waveOutUnprepareHeader(hwo, &mut headers[i], hdr_size);
                    state.fill_buffer(&shared, &mut buffers[i]);
                    headers[i].dwFlags = 0;
                    headers[i].dwBufferLength = BUFFER_BYTES as u32;
                    headers[i].lpData = buffers[i].as_mut_ptr() as *mut i8;
                    waveOutPrepareHeader(hwo, &mut headers[i], hdr_size);
                    waveOutWrite(hwo, &mut headers[i], hdr_size);
                }
            }
        }
        waveOutReset(hwo);
        for i in 0..NUM_BUFFERS {
            waveOutUnprepareHeader(hwo, &mut headers[i], hdr_size);
        }
        waveOutClose(hwo);
        CloseHandle(event);
    }
}

struct ActiveSfx {
    kind: u8,
    elapsed: u32,
    samples_remaining: u32,
}

struct DecodeResult {
    gend:   u32,
    path:  String,
    track: Option<MusicTrack>,
}

enum PendingSwap {
    Stop,
    SwapIn,
}

struct SynthState {
    sfx_consumed: [u32; SFX_COUNT],
    active: Vec<ActiveSfx>,

    sample_pos: u64,

    music:        Option<MusicTrack>,
    music_path:   Option<String>,
    music_paused: bool,

    music_fade:        f32,
    music_fade_target: f32,

    pending_swap: Option<PendingSwap>,

    decode_gen: u32,
    pending_track: Option<(u32, MusicTrack, String)>,

    music_rx: Receiver<DecodeResult>,
    music_tx: Sender<DecodeResult>,

    // Onset tracking state.
    music_prev_cursor:     f64,
    music_onset_idx:       usize,
    music_last_onset_src:  u64,
    onset_counter:         u32,
    pending_start: f32,
}

impl SynthState {
    fn new() -> Self {
        let (music_tx, music_rx) = mpsc::channel();
        SynthState {
            sfx_consumed: [0; SFX_COUNT],
            active: Vec::with_capacity(16),
            sample_pos: 0,
            music: None,
            music_path: None,
            music_paused: false,
            music_fade: 0.0,
            music_fade_target: 0.0,
            pending_swap: None,
            decode_gen: 0,
            pending_track: None,
            music_rx,
            music_tx,
            music_prev_cursor: 0.0,
            music_onset_idx: 0,
            music_last_onset_src: 0,
            onset_counter: 0,
            pending_start: 0.0,
        }
    }

    fn reset_onset_state(&mut self) {
        self.music_prev_cursor = 0.0;
        self.music_onset_idx = 0;
        self.music_last_onset_src = 0;
    }

    fn apply_music_commands(&mut self, shared: &Shared) {
        while let Ok(res) = self.music_rx.try_recv() {
            if res.gend == self.decode_gen {
                if let Some(track) = res.track {
                    self.pending_track = Some((res.gend, track, res.path));
                }
            }
        }

        let cmds: Vec<MusicCommand> = {
            match shared.music_cmds.lock() {
                Ok(mut g) => std::mem::take(&mut *g),
                Err(_)    => Vec::new(),
            }
        };
        for cmd in cmds {
            match cmd {
                MusicCommand::Play { path, force_restart, start_seconds } => {
                    let same = self.music_path.as_deref() == Some(path.as_str());
                    let alive = self.music.is_some()
                        || matches!(self.pending_swap, Some(PendingSwap::SwapIn))
                        || self.pending_track.is_some();
                    if same && alive && !force_restart {
                        // Same track already alive and the
                        // caller did not insist on a restart;
                        // honour the requested start position
                        // anyway by seeking the live track.
                        if start_seconds > 0.0 {
                            if let Some(m) = self.music.as_mut() {
                                m.seek_seconds(start_seconds);
                                let cur = m.cursor_src_sample() as u64;
                                let os  = m.onsets();
                                self.music_onset_idx = match os.binary_search(&cur) {
                                    Ok(i)  => i + 1,
                                    Err(i) => i,
                                };
                                self.music_last_onset_src = if self.music_onset_idx == 0 {
                                    0
                                } else {
                                    os[self.music_onset_idx - 1]
                                };
                                self.music_prev_cursor = m.cursor_src_sample();
                            }
                        }
                        continue;
                    }
                    self.decode_gen = self.decode_gen.wrapping_add(1);
                    let gend = self.decode_gen;
                    let tx  = self.music_tx.clone();
                    let p   = path.clone();
                    thread::spawn(move || {
                        let track = MusicTrack::open(&p, SAMPLE_RATE, true);
                        tx.send(DecodeResult { gend, path: p, track }).ok();
                    });
                    self.pending_track = None;
                    self.pending_swap = Some(PendingSwap::SwapIn);
                    self.music_fade_target = 0.0;
                    self.music_path = Some(path);
                    // Park the requested offset until the new
                    // track actually arrives. This is the
                    // critical change: a Seek issued right
                    // after Play used to operate on the OLD
                    // track (which was about to be dropped),
                    // and the new track always installed at
                    // cursor 0. With pending_start the offset
                    // travels with the swap.
                    self.pending_start = start_seconds;
                }
                MusicCommand::Stop => {
                    self.pending_swap = Some(PendingSwap::Stop);
                    self.music_fade_target = 0.0;
                }
                MusicCommand::SetPaused(p) => {
                    self.music_paused = p;
                }
                MusicCommand::Seek(s) => {
                    if let Some(m) = self.music.as_mut() {
                        m.seek_seconds(s);
                        let cur = m.cursor_src_sample() as u64;
                        let os  = m.onsets();
                        self.music_onset_idx = match os.binary_search(&cur) {
                            Ok(i)  => i + 1,
                            Err(i) => i,
                        };
                        self.music_last_onset_src = if self.music_onset_idx == 0 {
                            0
                        } else {
                            os[self.music_onset_idx - 1]
                        };
                        self.music_prev_cursor = m.cursor_src_sample();
                    }
                }
            }
        }
    }

    /// Move the freshly decoded track into the live slot,
    /// honouring any deferred start offset. Returns `true` if
    /// a track was actually installed (so callers do not have
    /// to re-check `pending_track` themselves).
    fn install_pending_track(&mut self) -> bool {
        let Some((_, track, path)) = self.pending_track.take() else {
            return false;
        };
        self.music = Some(track);
        self.music_path = Some(path);
        self.music_fade_target = 1.0;
        self.reset_onset_state();
        let start = self.pending_start;
        self.pending_start = 0.0;
        if start > 0.0 {
            if let Some(m) = self.music.as_mut() {
                m.seek_seconds(start);
                // Sync onset bookkeeping with the new cursor
                // so the "on beat" pulse and the gameplay
                // schedule stay in lockstep with what the
                // listener actually hears.
                let cur = m.cursor_src_sample() as u64;
                let os  = m.onsets();
                self.music_onset_idx = match os.binary_search(&cur) {
                    Ok(i)  => i + 1,
                    Err(i) => i,
                };
                self.music_last_onset_src = if self.music_onset_idx == 0 {
                    0
                } else {
                    os[self.music_onset_idx - 1]
                };
                self.music_prev_cursor = m.cursor_src_sample();
            }
        }
        true
    }

    fn advance_fade(&mut self) {
        if self.music_fade < self.music_fade_target {
            self.music_fade = (self.music_fade + FADE_STEP).min(self.music_fade_target);
        } else if self.music_fade > self.music_fade_target {
            self.music_fade = (self.music_fade - FADE_STEP).max(self.music_fade_target);
        }
        if self.music_fade_target == 0.0 && self.music_fade <= 0.0 {
            match self.pending_swap.take() {
                None => {}
                Some(PendingSwap::Stop) => {
                    self.music = None;
                    self.music_path = None;
                    self.pending_track = None;
                    self.reset_onset_state();
                }
                Some(PendingSwap::SwapIn) => {
                    if !self.install_pending_track() {
                        self.pending_swap = Some(PendingSwap::SwapIn);
                    }
                }
            }
        }
    }

    fn fill_buffer(&mut self, shared: &Shared, buf: &mut [i16]) {
        self.apply_music_commands(shared);

        for slot in 0..SFX_COUNT {
            let req = shared.sfx_requested[slot].load(Ordering::Relaxed);
            let pending = req.wrapping_sub(self.sfx_consumed[slot]);
            for _ in 0..pending {
                self.active.push(ActiveSfx {
                    kind: slot as u8,
                    elapsed: 0,
                    samples_remaining: sfx_duration_samples(slot),
                });
            }
            self.sfx_consumed[slot] = req;
        }

        let master = shared.volume_q8.load(Ordering::Relaxed) as f32 / 255.0;
        let music_gain = shared.music_volume_q8.load(Ordering::Relaxed) as f32 / 255.0;
        let sfx_gain   = shared.sfx_volume_q8.load(Ordering::Relaxed)   as f32 / 255.0;
        let frames = buf.len() / CHANNELS as usize;

        for i in 0..frames {
            self.advance_fade();
            if self.music_fade_target == 0.0
                && self.music_fade <= 0.0
                && matches!(self.pending_swap, Some(PendingSwap::SwapIn))
                && self.pending_track.is_some()
            {
                if self.install_pending_track() {
                    self.pending_swap = None;
                }
            }

            // Stereo music sample. Onset bookkeeping uses the
            // mono-equivalent cursor advance so kick detection
            // still works on stereo tracks; the cursor itself
            // is shared because both channels move together.
            let (mut music_l, mut music_r) = if let Some(m) = self.music.as_mut() {
                if self.music_paused {
                    (0.0, 0.0)
                } else {
                    let prev = self.music_prev_cursor;
                    let (l, r) = m.next_sample_stereo();
                    let l = l * 0.45 * self.music_fade;
                    let r = r * 0.45 * self.music_fade;
                    let now = m.cursor_src_sample();

                    if now < prev - 1.0 {
                        self.music_onset_idx = 0;
                        self.music_last_onset_src = 0;
                    }

                    let now_u = now as u64;
                    let os = m.onsets();
                    while self.music_onset_idx < os.len()
                          && os[self.music_onset_idx] <= now_u {
                        self.music_last_onset_src = os[self.music_onset_idx];
                        self.music_onset_idx += 1;
                        self.onset_counter = self.onset_counter.wrapping_add(1);
                    }
                    self.music_prev_cursor = now;

                    if m.is_finished() {
                        self.music = None;
                        self.music_path = None;
                        self.reset_onset_state();
                    }
                    (l, r)
                }
            } else {
                (0.0, 0.0)
            };
            music_l *= music_gain;
            music_r *= music_gain;

            // SFX are authored as mono; sum once and pan equally
            // to both ears so transient feedback cues don't
            // suddenly localize on stereo tracks.
            let mut sfx_s = 0.0f32;
            let mut j = 0;
            while j < self.active.len() {
                let s = sfx_sample(self.active[j].kind, self.active[j].elapsed);
                sfx_s += s;
                self.active[j].elapsed += 1;
                if self.active[j].samples_remaining <= 1 {
                    self.active.swap_remove(j);
                } else {
                    self.active[j].samples_remaining -= 1;
                    j += 1;
                }
            }
            sfx_s *= sfx_gain;

            let sample_l = (music_l + sfx_s) * master;
            let sample_r = (music_r + sfx_s) * master;

            // Soft-clip per channel so an overdriven mix on one
            // side does not pull the other side down with it.
            let limited_l = soft_clip(sample_l);
            let limited_r = soft_clip(sample_r);

            buf[i * 2    ] = (limited_l * 32767.0) as i16;
            buf[i * 2 + 1] = (limited_r * 32767.0) as i16;
            self.sample_pos += 1;
        }

        shared.sample_pos.store(self.sample_pos, Ordering::Relaxed);
        shared.onset_counter.store(self.onset_counter, Ordering::Relaxed);

        if let Some(m) = &self.music {
            let pos_s = m.position_seconds();
            let dur_s = m.duration_seconds();
            shared.music_position_q16.store(
                (pos_s as f64 * MUSIC_TIME_Q16_SCALE as f64) as u64,
                Ordering::Relaxed);
            shared.music_duration_q16.store(
                (dur_s as f64 * MUSIC_TIME_Q16_SCALE as f64) as u64,
                Ordering::Relaxed);
            shared.music_is_loaded.store(true, Ordering::Relaxed);

            let rate = m.source_sample_rate() as f64;
            let since =
                (m.cursor_src_sample() - self.music_last_onset_src as f64).max(0.0) / rate;
            let phase = (since / ONSET_DECAY_WINDOW_SEC).clamp(0.0, 1.0);
            let q = (phase * (1u32 << 24) as f64) as u32;
            shared.onset_phase_q24.store(q, Ordering::Relaxed);
        } else {
            shared.music_position_q16.store(0, Ordering::Relaxed);
            shared.music_duration_q16.store(0, Ordering::Relaxed);
            shared.music_is_loaded.store(false, Ordering::Relaxed);
            shared.onset_phase_q24.store(1 << 24, Ordering::Relaxed);
        }
        shared.music_is_paused.store(self.music_paused, Ordering::Relaxed);
    }
}

fn sfx_duration_samples(kind: usize) -> u32 {
    match kind {
        SFX_TOUCH    => SAMPLE_RATE * 4 / 10,
        SFX_INTERACT => SAMPLE_RATE * 1 / 10,
        SFX_ENTER    => SAMPLE_RATE * 3 / 10,
        SFX_EXIT     => SAMPLE_RATE * 35 / 100,
        _            => 0,
    }
}

/// Symmetric saturating limiter. Mirrors the previous inline
/// implementation but lives standalone so the stereo mix can
/// apply it independently to left and right.
fn soft_clip(sample: f32) -> f32 {
    let threshold = 0.85f32;
    if sample.abs() <= threshold {
        sample
    } else {
        let sign = sample.signum();
        let over = sample.abs() - threshold;
        let squashed = threshold + (1.0 - threshold) * (over / (over + 0.15));
        sign * squashed
    }
}

fn sfx_sample(kind: u8, t: u32) -> f32 {
    let s = t as f32 / SAMPLE_RATE as f32;
    let phase = t as f32 / SAMPLE_RATE as f32 * std::f32::consts::TAU;
    match kind as usize {
        SFX_TOUCH => {
            let env = (1.0 - s / 0.4).max(0.0).powf(2.0);
            let freq = (220.0 * (1.0 - s * 1.5).max(0.1)).max(60.0);
            (phase * freq).sin() * env * 0.45
        }
        SFX_INTERACT => {
            let env = (1.0 - s / 0.10).max(0.0);
            (phase * 880.0).sin() * env * 0.25
        }
        SFX_ENTER => {
            let env = (1.0 - (s / 0.30 - 0.5).abs() * 2.0).max(0.0);
            let freq = 220.0 + s * 660.0;
            (phase * freq).sin() * env * 0.30
        }
        SFX_EXIT => {
            let env = (1.0 - (s / 0.35 - 0.5).abs() * 2.0).max(0.0);
            let freq = 880.0 - s * 660.0;
            (phase * freq.max(120.0)).sin() * env * 0.32
        }
        _ => 0.0,
    }
}