//! QOA (Quite OK Audio) whole-file decoder.
//!
//! QOA is a fixed-rate lossy audio codec built around a 4-tap LMS
//! predictor. The entire bitstream specification fits on one page
//! and the reference decoder is a couple of hundred lines of C
//! without any dependencies, external tables that cannot be
//! written as constants, or heap gymnastics. It was picked as the
//! music codec for this game for exactly those reasons: the
//! previous OGG/Vorbis stack was six files of bit manipulation
//! and IMDCT math that consistently failed in subtle ways whose
//! diagnosis consumed more time than the rest of the audio
//! subsystem combined.
//!
//! Compression ratio is about 3.2x relative to 16 bit PCM. At
//! 44.1 kHz stereo that puts a three minute music track around
//! 10 MB on disk and 30 MB in memory after decoding. The decoder
//! here is a whole-file loader: the audio worker pulls the file
//! off disk once at level entry, hands the bytes to
//! `decode_bytes`, and keeps the resulting interleaved i16 buffer
//! around for the lifetime of the track. Every runtime operation
//! the game wants to do to music (looping, rate-based pitch
//! shifting, slicing, seeking) is then a plain array index.
//!
//! Reference: https://qoaformat.org/qoa-specification.pdf
//!
//! Failure mode: every constructor returns `Option`. A missing,
//! truncated or invalid file degrades silently to "no music for
//! this level", which matches the rest of the engine's policy of
//! never crashing over a cosmetic asset.

use std::fs::File;
use std::io::Read;

macro_rules! qdbg {
    ($($arg:tt)*) => {
        eprintln!("[qoa] {}", format_args!($($arg)*));
    };
}

/// 4 byte magic at the start of every QOA file: ASCII "qoaf".
const QOA_MAGIC: &[u8; 4] = b"qoaf";
/// Samples per slice. Fixed by the spec.
const QOA_SLICE_LEN: usize = 20;
/// Number of history / weight taps in the LMS predictor.
const QOA_LMS_LEN: usize = 4;

/// Dequantization table: `QOA_DEQUANT_TAB[scalefactor][residual]`
/// returns the reconstructed residual value. Taken verbatim from
/// the specification; precomputing it as a `const` keeps the
/// decoder free of any table construction at startup.
const QOA_DEQUANT_TAB: [[i32; 8]; 16] = [
    [   1,    -1,    3,    -3,    5,    -5,     7,     -7],
    [   5,    -5,   18,   -18,   32,   -32,    49,    -49],
    [  16,   -16,   53,   -53,   95,   -95,   147,   -147],
    [  34,   -34,  113,  -113,  203,  -203,   315,   -315],
    [  63,   -63,  210,  -210,  378,  -378,   588,   -588],
    [ 104,  -104,  345,  -345,  621,  -621,   966,   -966],
    [ 158,  -158,  528,  -528,  950,  -950,  1477,  -1477],
    [ 228,  -228,  760,  -760, 1368, -1368,  2128,  -2128],
    [ 316,  -316, 1053, -1053, 1895, -1895,  2947,  -2947],
    [ 422,  -422, 1405, -1405, 2529, -2529,  3934,  -3934],
    [ 548,  -548, 1828, -1828, 3290, -3290,  5117,  -5117],
    [ 696,  -696, 2320, -2320, 4176, -4176,  6496,  -6496],
    [ 868,  -868, 2893, -2893, 5207, -5207,  8099,  -8099],
    [1064, -1064, 3548, -3548, 6386, -6386,  9933,  -9933],
    [1286, -1286, 4288, -4288, 7718, -7718, 12005, -12005],
    [1536, -1536, 5120, -5120, 9216, -9216, 14336, -14336],
];

/// Result of decoding a whole QOA file.
pub struct QoaAudio {
    /// Sample rate in Hz, as stored in the first frame header.
    pub sample_rate: u32,
    /// Channel count, 1 or 2 in practice. Stored as `u16` to match
    /// the rest of the audio stack.
    pub channels: u16,
    /// Interleaved 16 bit PCM samples, in frame order. Length is
    /// `total_frames * channels`.
    pub samples: Vec<i16>,
}

/// One channel's LMS predictor state. Reinitialized from the
/// per-frame header once per frame and then updated sample by
/// sample within the frame.
#[derive(Clone, Copy)]
struct Lms {
    history: [i32; QOA_LMS_LEN],
    weights: [i32; QOA_LMS_LEN],
}

impl Lms {
    /// Predict the next sample as a dot product of history and
    /// weights, shifted to undo the 2^13 scaling the encoder uses.
    fn predict(&self) -> i32 {
        let mut p: i32 = 0;
        for i in 0..QOA_LMS_LEN {
            p = p.wrapping_add(self.weights[i].wrapping_mul(self.history[i]));
        }
        p >> 13
    }

    /// Update the predictor after emitting one sample. `sample` is
    /// the reconstructed value, `residual` the dequantized delta
    /// used to get there.
    fn update(&mut self, sample: i32, residual: i32) {
        let delta = residual >> 4;
        for i in 0..QOA_LMS_LEN {
            let d = if self.history[i] < 0 { -delta } else { delta };
            self.weights[i] = self.weights[i].wrapping_add(d);
        }
        for i in 0..QOA_LMS_LEN - 1 {
            self.history[i] = self.history[i + 1];
        }
        self.history[QOA_LMS_LEN - 1] = sample;
    }
}

/// Decode a QOA file from disk. Absolute paths are used verbatim;
/// relative paths are first tried against the executable's
/// directory, then against the current working directory, so both
/// double-click and `cargo run` launches work.
pub fn decode_file(path: &str) -> Option<QoaAudio> {
    let resolved = resolve_asset_path(path);
    let mut f = match File::open(&resolved) {
        Ok(f) => f,
        Err(e) => {
            qdbg!("open failed for {}: {}", resolved, e);
            return None;
        }
    };
    let mut buf = Vec::new();
    if let Err(e) = f.read_to_end(&mut buf) {
        qdbg!("read failed for {}: {}", resolved, e);
        return None;
    }
    decode_bytes(&buf)
}

/// Decode a QOA bitstream already resident in memory.
pub fn decode_bytes(data: &[u8]) -> Option<QoaAudio> {
    if data.len() < 8 || &data[0..4] != QOA_MAGIC {
        qdbg!("missing 'qoaf' magic (len={})", data.len());
        return None;
    }
    let total_samples =
        u32::from_be_bytes([data[4], data[5], data[6], data[7]]) as usize;
    let mut cur = 8usize;
    let mut pcm: Vec<i16> = Vec::new();
    let mut first_channels: Option<u16> = None;
    let mut first_rate:     Option<u32> = None;
    let mut samples_done = 0usize;

    // Walk frames until either the declared total sample count is
    // reached or the stream ends. A mid-stream truncation bails out
    // with whatever we managed to decode so far, rounded down to a
    // complete frame, rather than returning `None` and losing the
    // whole track.
    while samples_done < total_samples {
        if cur + 8 > data.len() {
            qdbg!("truncated frame header at byte {}", cur);
            break;
        }
        let channels = data[cur] as u16;
        let samplerate =
            u32::from_be_bytes([0, data[cur + 1], data[cur + 2], data[cur + 3]]);
        let fsamples = u16::from_be_bytes([data[cur + 4], data[cur + 5]]) as usize;
        let _fsize   = u16::from_be_bytes([data[cur + 6], data[cur + 7]]) as usize;
        cur += 8;

        if channels == 0 || channels > 8 || samplerate == 0 {
            qdbg!(
                "rejecting frame: channels={}, rate={}",
                channels, samplerate
            );
            return None;
        }

        if first_channels.is_none() {
            first_channels = Some(channels);
            first_rate     = Some(samplerate);
            pcm.reserve(total_samples * channels as usize);
        }

        // Per channel LMS state: 4 history + 4 weights, each i16 BE.
        let mut lmses: Vec<Lms> = Vec::with_capacity(channels as usize);
        for _ in 0..channels {
            if cur + 16 > data.len() {
                qdbg!("truncated LMS state at byte {}", cur);
                return Some(finalize(first_channels?, first_rate?, pcm));
            }
            let mut l = Lms { history: [0; QOA_LMS_LEN], weights: [0; QOA_LMS_LEN] };
            for i in 0..QOA_LMS_LEN {
                l.history[i] =
                    i16::from_be_bytes([data[cur], data[cur + 1]]) as i32;
                cur += 2;
            }
            for i in 0..QOA_LMS_LEN {
                l.weights[i] =
                    i16::from_be_bytes([data[cur], data[cur + 1]]) as i32;
                cur += 2;
            }
            lmses.push(l);
        }

        let num_slices = (fsamples + QOA_SLICE_LEN - 1) / QOA_SLICE_LEN;
        let frame_base = pcm.len();
        pcm.resize(frame_base + fsamples * channels as usize, 0);

        // Slices are interleaved across channels: (slice 0 ch 0),
        // (slice 0 ch 1), (slice 1 ch 0), (slice 1 ch 1), ...
        for si in 0..num_slices {
            for ch in 0..channels as usize {
                if cur + 8 > data.len() {
                    qdbg!("truncated slice at byte {}", cur);
                    return Some(finalize(first_channels?, first_rate?, pcm));
                }
                let mut slice: u64 = 0;
                for k in 0..8 {
                    slice = (slice << 8) | data[cur + k] as u64;
                }
                cur += 8;
                let sf = ((slice >> 60) & 0xF) as usize;
                let slice_start = si * QOA_SLICE_LEN;
                let slice_end   = (slice_start + QOA_SLICE_LEN).min(fsamples);
                for j in slice_start..slice_end {
                    let shift = 57 - (j - slice_start) * 3;
                    let q = ((slice >> shift) & 0x7) as usize;
                    let dq = QOA_DEQUANT_TAB[sf][q];
                    let predicted = lmses[ch].predict();
                    let r = (predicted + dq).clamp(-32768, 32767);
                    pcm[frame_base + j * channels as usize + ch] = r as i16;
                    lmses[ch].update(r, dq);
                }
            }
        }

        samples_done += fsamples;
    }

    Some(finalize(first_channels?, first_rate?, pcm))
}

/// Package the decoded PCM into a `QoaAudio`. Pulled out so the
/// truncation fallback paths can call it without duplicating the
/// struct literal.
fn finalize(channels: u16, sample_rate: u32, samples: Vec<i16>) -> QoaAudio {
    QoaAudio { channels, sample_rate, samples }
}

/// Resolve an asset path against the executable's directory.
///
/// Absolute paths are returned unchanged. Relative paths are tried
/// against the directory of `current_exe()` first; if that does
/// not produce an existing file we fall back to the path as given,
/// which is what makes `cargo run` from the crate root work.
fn resolve_asset_path(path: &str) -> String {
    use std::path::Path;
    let p = Path::new(path);
    if p.is_absolute() {
        return path.to_string();
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let candidate = dir.join(p);
            if candidate.exists() {
                if let Some(s) = candidate.to_str() {
                    return s.to_string();
                }
            }
        }
    }
    path.to_string()
}