//! High level music track abstraction (QOA-backed, see `qoa.rs`).
//!
//! The track is fully decoded into RAM at open time. Every runtime
//! operation is then plain array arithmetic: looping, slicing,
//! pitch/speed (via `rate_mult`), seeking.
//!
//! Beat onsets:
//!
//! Right after decode we run a small onset detector over the PCM,
//! targeting the kick drum: a one pole low pass at ~150 Hz, energy
//! accumulated in ~23 ms windows, peaks above a rolling average
//! with a minimum spacing of 150 ms. The resulting sample positions
//! are stored in `onsets` and consumed by the audio worker to
//! publish the game's "on beat" signal (see `audio.rs`).
//!
//! Failure mode: every constructor returns `Option`. A missing,
//! truncated or invalid file degrades silently to "no music".

use crate::qoa;

macro_rules! mdbg {
    ($($arg:tt)*) => { eprintln!("[music] {}", format_args!($($arg)*)); };
}

pub struct MusicTrack {
    pcm: Vec<i16>,
    channels: usize,
    src_rate: u32,
    dst_rate: u32,
    cursor: f64,
    rate_mult: f32,
    loop_on_end: bool,
    finished: bool,
    range_start: f64,
    range_end: f64,

    /// Sample indices (in the source stream, i.e. indexed the same
    /// way as `self.cursor`) where a kick drum onset was detected.
    /// Sorted ascending. May be empty on very quiet tracks.
    onsets: Vec<u64>,
}

impl MusicTrack {
    pub fn open(path: &str, dst_rate: u32, loop_on_end: bool) -> Option<Self> {
        mdbg!("opening track: {}", path);
        let audio = match qoa::decode_file(path) {
            Some(a) => a,
            None => {
                mdbg!("FAILED to decode: {}", path);
                return None;
            }
        };
        let channels = audio.channels as usize;
        if channels == 0 || audio.samples.is_empty() {
            mdbg!("track is empty: {}", path);
            return None;
        }
        let frames = audio.samples.len() / channels;
        let onsets = detect_onsets(&audio.samples, channels, audio.sample_rate);
        mdbg!(
            "track ready: frames={}, channels={}, src_rate={}, dst_rate={}, loop={}, onsets={}",
            frames, channels, audio.sample_rate, dst_rate, loop_on_end, onsets.len()
        );
        Some(MusicTrack {
            pcm: audio.samples,
            channels,
            src_rate: audio.sample_rate,
            dst_rate,
            cursor: 0.0,
            rate_mult: 1.0,
            loop_on_end,
            finished: false,
            range_start: 0.0,
            range_end: frames as f64,
            onsets,
        })
    }

    pub fn total_frames(&self) -> usize { self.pcm.len() / self.channels }

    /// Current playback position within the track, in seconds.
    pub fn position_seconds(&self) -> f32 {
        (self.cursor / self.src_rate as f64) as f32
    }

    /// Full length of the playable range, in seconds.
    pub fn duration_seconds(&self) -> f32 {
        ((self.range_end - self.range_start) / self.src_rate as f64) as f32
    }

    pub fn set_rate(&mut self, r: f32) { self.rate_mult = r.clamp(0.1, 4.0); }

    pub fn set_range_seconds(&mut self, start_s: f32, end_s: f32) {
        let total = self.total_frames() as f64;
        let s = ((start_s as f64) * self.src_rate as f64).clamp(0.0, total);
        let e = ((end_s   as f64) * self.src_rate as f64).clamp(s,   total);
        self.range_start = s;
        self.range_end   = e;
        if self.cursor < s { self.cursor = s; }
        if self.cursor > e { self.cursor = e; }
    }

    pub fn seek_seconds(&mut self, s: f32) {
        let f = ((s as f64) * self.src_rate as f64)
            .clamp(self.range_start, self.range_end);
        self.cursor = f;
        self.finished = false;
    }

    pub fn is_finished(&self) -> bool { self.finished }

    /// Current cursor expressed as a source sample index. Used by
    /// the audio worker for onset tracking.
    pub fn cursor_src_sample(&self) -> f64 { self.cursor }

    /// Source sample rate, needed to convert onset positions into
    /// seconds for the UI.
    pub fn source_sample_rate(&self) -> u32 { self.src_rate }

    /// Read only view on the precomputed onset list.
    pub fn onsets(&self) -> &[u64] { &self.onsets }

        /// Pull the next stereo frame from the track.
    ///
    /// * Mono sources are duplicated into both channels so the
    ///   downstream mixer never has to special-case them.
    /// * Stereo sources route channel 0 to the left output and
    ///   channel 1 to the right output. Tracks with three or
    ///   more channels are downmixed to a centered stereo pair
    ///   by averaging the leading half into L and the trailing
    ///   half into R, which is good enough for the few games
    ///   assets that ever ship as 5.1.
    ///
    /// Both samples share the same fractional cursor and are
    /// linearly interpolated against the next frame, so the
    /// pitch / speed multiplier and the loop-on-end logic from
    /// the previous mono code path apply unchanged.
    pub fn next_sample_stereo(&mut self) -> (f32, f32) {
        if self.finished { return (0.0, 0.0); }
        if self.cursor >= self.range_end {
            if self.loop_on_end {
                self.cursor = self.range_start;
            } else {
                self.finished = true;
                return (0.0, 0.0);
            }
        }

        let total = self.total_frames();
        let i0 = self.cursor.floor() as usize;
        let frac = (self.cursor - i0 as f64) as f32;
        let i0 = i0.min(total.saturating_sub(1));
        let i1 = (i0 + 1).min(total.saturating_sub(1));

        let (al, ar) = frame_to_stereo(&self.pcm, i0, self.channels);
        let (bl, br) = frame_to_stereo(&self.pcm, i1, self.channels);
        let l = al + (bl - al) * frac;
        let r = ar + (br - ar) * frac;

        let advance = (self.src_rate as f64 / self.dst_rate as f64)
                    * self.rate_mult as f64;
        self.cursor += advance;
        (l, r)
    }

    /// Mono convenience wrapper. Kept around for any future
    /// caller that genuinely wants a single number (e.g. waveform
    /// visualisation). Internally calls the stereo path and
    /// averages, so behaviour stays consistent across both APIs.
    pub fn next_sample(&mut self) -> f32 {
        let (l, r) = self.next_sample_stereo();
        (l + r) * 0.5
    }
}

/// Extract one source frame as a `(left, right)` pair, applying
/// the same downmix policy described on `next_sample_stereo`.
fn frame_to_stereo(pcm: &[i16], frame_idx: usize, channels: usize) -> (f32, f32) {
    let base = frame_idx * channels;
    let scale = 1.0 / 32768.0;
    match channels {
        0 => (0.0, 0.0),
        1 => {
            let s = pcm[base] as f32 * scale;
            (s, s)
        }
        2 => (
            pcm[base    ] as f32 * scale,
            pcm[base + 1] as f32 * scale,
        ),
        n => {
            // Generic fan-in for 3+ channel sources. Front-half
            // goes left, back-half goes right; samples sitting
            // exactly on the seam (i.e. a true center channel)
            // are split evenly between the two outputs.
            let half = n / 2;
            let mut l = 0.0f32;
            let mut r = 0.0f32;
            for c in 0..n {
                let s = pcm[base + c] as f32 * scale;
                if n % 2 == 1 && c == half {
                    l += s * 0.5;
                    r += s * 0.5;
                } else if c < half {
                    l += s;
                } else {
                    r += s;
                }
            }
            let lc = (half as f32).max(1.0);
            let rc = ((n - half) as f32).max(1.0);
            (l / lc, r / rc)
        }
    }
}

/// Detect kick drum onsets.
///
/// Algorithm (deliberately simple, zero dependency):
///
/// 1. Downmix to mono.
/// 2. Pass each mono sample through a one pole low pass filter
///    whose cutoff is around 150 Hz. The kick drum energy lives
///    mostly below this; melodies and hi hats drop away.
/// 3. Accumulate squared samples into fixed size windows (~23 ms
///    at 44.1 kHz). Each window becomes one energy value indexed
///    by the frame at the middle of the window.
/// 4. Walk the energy array; a window is flagged as an onset when
///    it is a local maximum AND its value exceeds `RATIO` times
///    the average over a surrounding span of windows.
/// 5. Enforce a minimum inter onset interval of 150 ms so that a
///    single loud hit does not produce a cluster.
///
/// The result is a sorted list of source sample positions. Tracks
/// with no detectable kick still decode fine, they just drive no
/// pulse: the game treats "no onsets" as "no beat", which is a
/// silent no op, not a bug.
fn detect_onsets(pcm: &[i16], channels: usize, sample_rate: u32) -> Vec<u64> {
    let frames = pcm.len() / channels;
    if frames < 2048 { return Vec::new(); }

    let cutoff_hz = 600.0_f32;
    let dt = 1.0 / sample_rate as f32;
    let rc = 1.0 / (std::f32::consts::TAU * cutoff_hz);
    let alpha = dt / (rc + dt);

    let window_len = 1024usize;
    let hop = window_len;
    let num_windows = frames / hop;
    if num_windows < 4 { return Vec::new(); }

    let mut energies: Vec<(u64, f32)> = Vec::with_capacity(num_windows);
    let mut lpf = 0.0f32;
    let mut acc = 0.0f32;
    let mut count = 0usize;
    let mut window_start_frame: u64 = 0;

    for f in 0..frames {
        let base = f * channels;
        let mut s = 0.0f32;
        for c in 0..channels {
            s += pcm[base + c] as f32 / 32768.0;
        }
        s /= channels as f32;
        lpf += alpha * (s - lpf);
        acc += lpf * lpf;
        count += 1;
        if count >= window_len {
            let mid = window_start_frame + (window_len as u64 / 2);
            energies.push((mid, acc));
            acc = 0.0;
            count = 0;
            window_start_frame = f as u64 + 1;
        }
    }

    if energies.len() < 2 { return Vec::new(); }

    let avg_radius = 16usize;
    let ratio: f32 = 1.7;
    let min_spacing_samples =
        (sample_rate as u64).saturating_mul(20) / 1000;

    let mut onsets = Vec::new();
    let mut last_pos: u64 = 0;

    for i in 1..energies.len() - 1 {
        let (pos, e) = energies[i];
        let prev_e = energies[i - 1].1;
        let next_e = energies[i + 1].1;
        if !(e > prev_e && e >= next_e) { continue; }

        let lo = i.saturating_sub(avg_radius);
        let hi = (i + avg_radius).min(energies.len() - 1);
        let mut sum = 0.0f32;
        for k in lo..=hi { sum += energies[k].1; }
        let denom = (hi - lo + 1) as f32;
        let avg = sum / denom;

        if e < avg * ratio + 1e-6 { continue; }

        if !onsets.is_empty() && pos.saturating_sub(last_pos) < min_spacing_samples {
            continue;
        }
        onsets.push(pos);
        last_pos = pos;
    }

    onsets
}