//! Track atomizer for Super Rustogon, revision 2.
//!
//! * BPM detection scans 50..=240 and reports the top three
//!   local maxima separated by at least 5 BPM. Both halves
//!   of an octave-ambiguous track end up in the report.
//! * A new intensity curve, defined as
//!   `density * mean_strength * sqrt(overall)`, is computed
//!   per second and normalized to 0..1. This metric tracks
//!   the felt aggression of each second far better than the
//!   spectral mass curve does.
//! * Section segmentation now uses a six dimensional feature
//!   vector per second (overall, bass ratio, HF ratio,
//!   onset density, mean onset strength, crest factor) and
//!   a lower novelty threshold, so a track with constant
//!   spectral content but rising onset density splits into
//!   the right sub-sections.
//! * Onset clusters group runs of strong onsets into named
//!   moments. These usually correspond directly to the
//!   author-meaningful events: a drop, a fill, a climax.
//! * dB and crest factor are reported per second so the
//!   actual loudness and transient density are visible
//!   independent of normalization.
//! * The report opens with a TRACK SUMMARY block that lists
//!   the aggression class, the hot moments, the BPM
//!   candidates, and the drop count, so a level designer
//!   gets a one-glance view before reading the tables.
//!
//! Usage is unchanged:
//!
//!     cargo run --release --bin atomize -- track.qoa
//!     cargo run --release --bin atomize -- track.qoa report.txt

#[path = "../qoa.rs"]
mod qoa;

use std::env;
use std::f32::consts::PI;
use std::fs::File;
use std::io::Write;

// ---------- tunables ----------

const FFT_SIZE: usize = 2048;
const HOP_SIZE: usize = 512;

const NUM_BANDS: usize = 7;
const BAND_NAMES: [&str; NUM_BANDS] = [
    "sub", "bass", "lomid", "mid", "himid", "high", "brill",
];
const BAND_BOUNDS_HZ: [(f32, f32); NUM_BANDS] = [
    (   20.0,    60.0),
    (   60.0,   250.0),
    (  250.0,   500.0),
    (  500.0,  2000.0),
    ( 2000.0,  4000.0),
    ( 4000.0,  8000.0),
    ( 8000.0, 16000.0),
];

const ONSET_MIN_GAP_S: f32 = 0.10;
const MIN_SECTION_LEN_S: f32 = 4.0;
const NUM_SECTION_FEATURES: usize = 6;

// ---------- complex math and FFT ----------

#[derive(Clone, Copy)]
struct Cplx { re: f32, im: f32 }

impl Cplx {
    fn zero() -> Self { Cplx { re: 0.0, im: 0.0 } }
    fn norm(self) -> f32 { (self.re * self.re + self.im * self.im).sqrt() }
}

impl std::ops::Add for Cplx {
    type Output = Cplx;
    fn add(self, o: Cplx) -> Cplx { Cplx { re: self.re + o.re, im: self.im + o.im } }
}
impl std::ops::Sub for Cplx {
    type Output = Cplx;
    fn sub(self, o: Cplx) -> Cplx { Cplx { re: self.re - o.re, im: self.im - o.im } }
}
impl std::ops::Mul for Cplx {
    type Output = Cplx;
    fn mul(self, o: Cplx) -> Cplx {
        Cplx {
            re: self.re * o.re - self.im * o.im,
            im: self.re * o.im + self.im * o.re,
        }
    }
}

fn fft(buf: &mut [Cplx]) {
    let n = buf.len();
    if n < 2 { return; }
    let mut j = 0usize;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j ^= bit;
        if i < j { buf.swap(i, j); }
    }
    let mut len = 2usize;
    while len <= n {
        let ang = -2.0 * PI / (len as f32);
        let wlen = Cplx { re: ang.cos(), im: ang.sin() };
        let half = len / 2;
        let mut i = 0usize;
        while i < n {
            let mut w = Cplx { re: 1.0, im: 0.0 };
            for k in 0..half {
                let u = buf[i + k];
                let v = buf[i + k + half] * w;
                buf[i + k]        = u + v;
                buf[i + k + half] = u - v;
                w = w * wlen;
            }
            i += len;
        }
        len <<= 1;
    }
}

fn hann_window(n: usize) -> Vec<f32> {
    (0..n)
        .map(|i| 0.5 * (1.0 - (2.0 * PI * i as f32 / (n - 1) as f32).cos()))
        .collect()
}

// ---------- spectral pass ----------

struct Frame {
    bands:   [f32; NUM_BANDS],
    overall: f32,
    /// Time domain RMS of the windowed samples in this frame.
    /// Stored linear in 0..1 so the report can convert to dB.
    rms:     f32,
    /// Peak absolute sample inside the windowed frame.
    peak:    f32,
}

/// Run the STFT and produce per frame band energies, the per
/// frame spectral flux, and the time domain rms / peak which
/// drive the dB and crest factor columns of the report.
fn spectral_pass(
    pcm_mono: &[f32], sample_rate: u32,
) -> (Vec<Frame>, Vec<f32>) {
    let win = hann_window(FFT_SIZE);
    let bin_hz = sample_rate as f32 / FFT_SIZE as f32;

    let mut band_bins = [(0usize, 0usize); NUM_BANDS];
    for (i, (lo, hi)) in BAND_BOUNDS_HZ.iter().enumerate() {
        let lo_b = (lo / bin_hz) as usize;
        let hi_b = ((hi / bin_hz) as usize).min(FFT_SIZE / 2);
        band_bins[i] = (lo_b, hi_b.max(lo_b + 1));
    }

    let mut frames: Vec<Frame> = Vec::new();
    let mut flux:   Vec<f32>   = Vec::new();

    let mut buf = vec![Cplx::zero(); FFT_SIZE];
    let mut spectrum = vec![0.0f32; FFT_SIZE / 2];
    let mut prev_spectrum: Option<Vec<f32>> = None;

    let mut pos = 0usize;
    while pos + FFT_SIZE <= pcm_mono.len() {
        // Window into the FFT buffer and accumulate time
        // domain rms / peak in the same loop, since we have
        // the windowed samples right there.
        let mut rms_acc = 0.0f32;
        let mut peak    = 0.0f32;
        for i in 0..FFT_SIZE {
            let s = pcm_mono[pos + i] * win[i];
            buf[i] = Cplx { re: s, im: 0.0 };
            rms_acc += s * s;
            let a = s.abs();
            if a > peak { peak = a; }
        }
        let rms = (rms_acc / FFT_SIZE as f32).sqrt();

        fft(&mut buf);
        for k in 0..FFT_SIZE / 2 {
            spectrum[k] = buf[k].norm();
        }

        let f = match &prev_spectrum {
            Some(prev) => {
                let mut s = 0.0f32;
                for k in 0..spectrum.len() {
                    let d = spectrum[k] - prev[k];
                    if d > 0.0 { s += d; }
                }
                s
            }
            None => 0.0,
        };
        flux.push(f);

        let mut bands = [0.0f32; NUM_BANDS];
        for (i, (lo, hi)) in band_bins.iter().enumerate() {
            let mut s = 0.0;
            for k in *lo..*hi { s += spectrum[k]; }
            bands[i] = s;
        }
        let overall: f32 = bands.iter().sum();
        frames.push(Frame { bands, overall, rms, peak });

        prev_spectrum = Some(spectrum.clone());
        pos += HOP_SIZE;
    }

    (frames, flux)
}

// ---------- onset detection ----------

struct Onset { time: f32, strength: f32 }

fn detect_onsets(flux: &[f32], frame_rate: f32) -> (Vec<f32>, Vec<Onset>) {
    let n = flux.len();
    if n == 0 { return (Vec::new(), Vec::new()); }

    let win_frames = (frame_rate * 0.20) as usize;
    let mut osn = vec![0.0f32; n];
    for i in 0..n {
        let lo = i.saturating_sub(win_frames);
        let hi = (i + win_frames + 1).min(n);
        let mut sum = 0.0;
        for k in lo..hi { sum += flux[k]; }
        let avg = sum / (hi - lo) as f32;
        osn[i] = (flux[i] - avg).max(0.0);
    }

    let max_v = osn.iter().cloned().fold(0.0f32, f32::max).max(1e-6);
    let thresh = max_v * 0.10;
    let min_gap = (frame_rate * ONSET_MIN_GAP_S) as usize;

    let mut peaks = Vec::new();
    let mut last_idx: Option<usize> = None;
    for i in 1..n.saturating_sub(1) {
        if osn[i] > thresh
            && osn[i] > osn[i - 1]
            && osn[i] >= osn[i + 1]
        {
            let ok = match last_idx {
                Some(li) => i - li >= min_gap,
                None     => true,
            };
            if ok {
                peaks.push(Onset {
                    time:     i as f32 / frame_rate,
                    strength: (osn[i] / max_v).min(1.0),
                });
                last_idx = Some(i);
            }
        }
    }
    (osn, peaks)
}

// ---------- BPM with octave reporting ----------

struct BpmCandidate {
    bpm:        f32,
    score:      f32,
    confidence: f32,
}

/// Scan plausible BPMs, return the top three local maxima
/// separated by at least 5 BPM. This makes octave ambiguity
/// visible: a track whose kick lands on the half note will
/// produce two candidates one octave apart with comparable
/// scores, instead of collapsing to a single misleading
/// number.
fn estimate_bpm_candidates(osn: &[f32], frame_rate: f32) -> Vec<BpmCandidate> {
    if osn.is_empty() { return Vec::new(); }

    let mut scored: Vec<(f32, f32)> = Vec::new();
    let mut sum_score = 0.0f32;
    let mut count = 0usize;
    for bpm_int in 50..=240 {
        let bpm = bpm_int as f32;
        let period = (60.0 / bpm * frame_rate) as usize;
        if period == 0 || period >= osn.len() / 2 { continue; }
        let mut score = 0.0f32;
        let mut n = 0usize;
        for i in 0..osn.len() - period {
            score += osn[i] * osn[i + period];
            n += 1;
        }
        if n > 0 { score /= n as f32; }
        scored.push((bpm, score));
        sum_score += score;
        count += 1;
    }

    let mean = if count > 0 { sum_score / count as f32 } else { 0.0 };

    // Sort by score descending, then dedupe by BPM proximity.
    let mut sorted = scored.clone();
    sorted.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());

    let mut top: Vec<(f32, f32)> = Vec::new();
    for (bpm, score) in sorted {
        let too_close = top.iter().any(|(b2, _)| (bpm - b2).abs() < 5.0);
        if !too_close {
            top.push((bpm, score));
            if top.len() >= 3 { break; }
        }
    }

    top.into_iter().map(|(bpm, score)| {
        let conf = if mean > 0.0 {
            ((score / mean - 1.0) * 0.25).clamp(0.0, 1.0)
        } else { 0.0 };
        BpmCandidate { bpm, score, confidence: conf }
    }).collect()
}

// ---------- per second profiles ----------

struct EnergyAt {
    time:     f32,
    overall:  f32,
    bands:    [f32; NUM_BANDS],
    rms_db:   f32,
    peak_db:  f32,
    crest_db: f32,
}

fn energy_per_second(
    frames: &[Frame], frame_rate: f32, total_s: f32,
) -> Vec<EnergyAt> {
    if frames.is_empty() { return Vec::new(); }
    let secs = total_s.ceil() as usize;
    let mut max_overall = 1e-6f32;
    let mut max_band = [1e-6f32; NUM_BANDS];
    for f in frames {
        if f.overall > max_overall { max_overall = f.overall; }
        for b in 0..NUM_BANDS {
            if f.bands[b] > max_band[b] { max_band[b] = f.bands[b]; }
        }
    }

    let mut out = Vec::with_capacity(secs);
    for s in 0..secs {
        let lo = (s as f32 * frame_rate) as usize;
        let hi = (((s + 1) as f32) * frame_rate)
            .min(frames.len() as f32) as usize;
        if lo >= hi { break; }
        let mut overall = 0.0;
        let mut bands = [0.0f32; NUM_BANDS];
        let mut rms_acc = 0.0f32;
        let mut peak_max = 0.0f32;
        for i in lo..hi {
            overall += frames[i].overall;
            for b in 0..NUM_BANDS { bands[b] += frames[i].bands[b]; }
            rms_acc += frames[i].rms * frames[i].rms;
            if frames[i].peak > peak_max { peak_max = frames[i].peak; }
        }
        let n = (hi - lo) as f32;
        overall = (overall / n / max_overall).clamp(0.0, 1.0);
        for b in 0..NUM_BANDS {
            bands[b] = (bands[b] / n / max_band[b]).clamp(0.0, 1.0);
        }
        let rms_lin = (rms_acc / n).sqrt().max(1e-6);
        let peak_lin = peak_max.max(1e-6);
        let rms_db   = 20.0 * rms_lin.log10();
        let peak_db  = 20.0 * peak_lin.log10();
        let crest_db = peak_db - rms_db;
        out.push(EnergyAt {
            time: s as f32, overall, bands,
            rms_db, peak_db, crest_db,
        });
    }
    out
}

struct IntensityAt {
    time:          f32,
    onset_density: f32,
    mean_strength: f32,
    sum_strength:  f32,
    intensity:     f32,
}

/// Build the per second intensity curve. This is the central
/// addition of revision 2: it folds onset density, onset
/// strength, and overall energy into a single number that
/// tracks felt aggression. Final values are normalized to
/// 0..1 across the track so that 1.0 is always the loudest
/// most percussive moment in the file.
fn intensity_per_second(
    onsets: &[Onset], energy: &[EnergyAt], total_s: f32,
) -> Vec<IntensityAt> {
    let secs = total_s.ceil() as usize;
    let mut out = Vec::with_capacity(secs);
    let mut raw = Vec::with_capacity(secs);

    for s in 0..secs {
        let t = s as f32;
        let lo = t - 0.5;
        let hi = t + 0.5;
        let mut count = 0u32;
        let mut sum = 0.0f32;
        for o in onsets {
            if o.time >= lo && o.time < hi {
                count += 1;
                sum += o.strength;
            }
        }
        let mean = if count > 0 { sum / count as f32 } else { 0.0 };
        let density = count as f32;
        let overall = energy.get(s).map(|e| e.overall).unwrap_or(0.0);
        let raw_i = density * mean * overall.sqrt();
        raw.push(raw_i);
        out.push(IntensityAt {
            time: t,
            onset_density: density,
            mean_strength: mean,
            sum_strength:  sum,
            intensity:     raw_i,
        });
    }

    let max_i = raw.iter().cloned().fold(0.0f32, f32::max).max(1e-6);
    for i in 0..out.len() {
        out[i].intensity = (out[i].intensity / max_i).clamp(0.0, 1.0);
    }
    out
}

// ---------- onset clusters ----------

struct Cluster {
    start:         f32,
    end:           f32,
    count:         u32,
    peak_strength: f32,
    avg_strength:  f32,
}

fn detect_clusters(onsets: &[Onset]) -> Vec<Cluster> {
    let strength_threshold = 0.30;
    let max_gap_s = 0.40;
    let min_count = 4u32;

    let mut clusters: Vec<Cluster> = Vec::new();
    let mut cur_start = 0.0f32;
    let mut cur_end   = 0.0f32;
    let mut cur_count = 0u32;
    let mut cur_sum   = 0.0f32;
    let mut cur_peak  = 0.0f32;

    let push_if_significant = |
        clusters: &mut Vec<Cluster>,
        start: f32, end: f32, count: u32, sum: f32, peak: f32,
    | {
        if count >= min_count {
            clusters.push(Cluster {
                start, end, count,
                peak_strength: peak,
                avg_strength:  sum / count as f32,
            });
        }
    };

    for o in onsets {
        if o.strength < strength_threshold {
            push_if_significant(&mut clusters,
                cur_start, cur_end, cur_count, cur_sum, cur_peak);
            cur_count = 0;
            continue;
        }
        if cur_count == 0 {
            cur_start = o.time; cur_end = o.time;
            cur_count = 1; cur_sum = o.strength; cur_peak = o.strength;
        } else if o.time - cur_end > max_gap_s {
            push_if_significant(&mut clusters,
                cur_start, cur_end, cur_count, cur_sum, cur_peak);
            cur_start = o.time; cur_end = o.time;
            cur_count = 1; cur_sum = o.strength; cur_peak = o.strength;
        } else {
            cur_end = o.time;
            cur_count += 1;
            cur_sum += o.strength;
            if o.strength > cur_peak { cur_peak = o.strength; }
        }
    }
    push_if_significant(&mut clusters,
        cur_start, cur_end, cur_count, cur_sum, cur_peak);
    clusters
}

// ---------- drops and hot moments ----------

struct Drop {
    time:           f32,
    duration:       f32,
    peak_intensity: f32,
}

fn detect_drops(intensity: &[IntensityAt]) -> Vec<Drop> {
    if intensity.is_empty() { return Vec::new(); }
    let mut sorted: Vec<f32> = intensity.iter().map(|i| i.intensity).collect();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let pct75 = sorted[(sorted.len() * 3) / 4];
    let threshold = pct75.max(0.40);
    let release   = threshold * 0.65;
    let min_run   = 4usize;

    let mut drops = Vec::new();
    let mut i = 0usize;
    while i < intensity.len() {
        if intensity[i].intensity >= threshold {
            let start = i;
            let mut end = i;
            let mut peak = intensity[i].intensity;
            while end < intensity.len()
                && intensity[end].intensity >= release
            {
                if intensity[end].intensity > peak {
                    peak = intensity[end].intensity;
                }
                end += 1;
            }
            if end - start >= min_run {
                drops.push(Drop {
                    time:           intensity[start].time,
                    duration:       (end - start) as f32,
                    peak_intensity: peak,
                });
            }
            i = end.max(start + 1);
        } else {
            i += 1;
        }
    }
    drops
}

struct HotMoment { time: f32, intensity: f32 }

fn detect_hot_moments(intensity: &[IntensityAt]) -> Vec<HotMoment> {
    let n = intensity.len();
    if n < 5 { return Vec::new(); }
    let mut peaks: Vec<HotMoment> = Vec::new();
    for i in 2..n - 2 {
        let v = intensity[i].intensity;
        if v >= intensity[i - 1].intensity
            && v >= intensity[i + 1].intensity
            && v >= intensity[i - 2].intensity
            && v >= intensity[i + 2].intensity
            && v >= 0.40
        {
            peaks.push(HotMoment {
                time:      intensity[i].time,
                intensity: v,
            });
        }
    }
    peaks.sort_by(|a, b| b.intensity.partial_cmp(&a.intensity).unwrap());

    let min_gap = 8.0f32;
    let mut picked: Vec<HotMoment> = Vec::new();
    for p in peaks {
        let too_close = picked.iter()
            .any(|q| (q.time - p.time).abs() < min_gap);
        if !too_close {
            picked.push(p);
            if picked.len() >= 8 { break; }
        }
    }
    picked.sort_by(|a, b| a.time.partial_cmp(&b.time).unwrap());
    picked
}

// ---------- section detection v2 ----------

struct Section {
    start:         f32,
    end:           f32,
    label:         String,
    energy_label:  String,
    character:     String,
    avg_energy:    f32,
    avg_intensity: f32,
    avg_density:   f32,
    avg_strength:  f32,
}

/// Multi-feature segmentation. Each second is summarized by
/// six normalized values (overall energy, bass ratio, HF
/// ratio, onset density, mean onset strength, crest factor).
/// Novelty is the L2 distance between smoothed feature
/// vectors at lag 3 s; peaks above 25% of the max novelty
/// become section boundaries. Minimum section length is 4 s.
fn detect_sections_v2(
    energy: &[EnergyAt], intensity: &[IntensityAt], duration: f32,
) -> Vec<Section> {
    if energy.len() < 4 || intensity.len() < 4 {
        return vec![Section {
            start: 0.0, end: duration,
            label: "main".into(),
            energy_label: "UNKNOWN".into(),
            character: "STEADY".into(),
            avg_energy: 0.5,
            avg_intensity: 0.0,
            avg_density: 0.0,
            avg_strength: 0.0,
        }];
    }

    let n = energy.len().min(intensity.len());
    let max_density = intensity.iter()
        .map(|i| i.onset_density).fold(0.0f32, f32::max).max(1e-6);
    let crest_norm = 30.0f32;

    let mut feats = vec![[0.0f32; NUM_SECTION_FEATURES]; n];
    for s in 0..n {
        let e = &energy[s];
        let i = &intensity[s];
        let bass_ratio = (e.bands[0] + e.bands[1]) / (e.overall + 1e-6);
        let hf_ratio   = (e.bands[4] + e.bands[5] + e.bands[6])
                        / (e.overall + 1e-6);
        feats[s][0] = e.overall;
        feats[s][1] = bass_ratio.clamp(0.0, 1.0);
        feats[s][2] = hf_ratio.clamp(0.0, 1.0);
        feats[s][3] = i.onset_density / max_density;
        feats[s][4] = i.mean_strength.clamp(0.0, 1.0);
        feats[s][5] = (e.crest_db / crest_norm).clamp(0.0, 1.0);
    }

    let smooth = 1usize;
    let mut smoothed = vec![[0.0f32; NUM_SECTION_FEATURES]; n];
    for i in 0..n {
        let lo = i.saturating_sub(smooth);
        let hi = (i + smooth + 1).min(n);
        let mut acc = [0.0f32; NUM_SECTION_FEATURES];
        for k in lo..hi {
            for d in 0..NUM_SECTION_FEATURES { acc[d] += feats[k][d]; }
        }
        let count = (hi - lo) as f32;
        for d in 0..NUM_SECTION_FEATURES {
            smoothed[i][d] = acc[d] / count;
        }
    }

    let lag = 3usize;
    let mut novelty = vec![0.0f32; n];
    for i in lag..n {
        let mut s = 0.0f32;
        for d in 0..NUM_SECTION_FEATURES {
            let diff = smoothed[i][d] - smoothed[i - lag][d];
            s += diff * diff;
        }
        novelty[i] = s.sqrt();
    }

    let max_n  = novelty.iter().cloned().fold(0.0f32, f32::max).max(1e-6);
    let thresh = max_n * 0.25;
    let min_gap = MIN_SECTION_LEN_S as usize;

    let mut bounds: Vec<f32> = vec![0.0];
    let mut last = 0usize;
    for i in 2..novelty.len().saturating_sub(2) {
        if novelty[i] > thresh
            && novelty[i] >= novelty[i - 1]
            && novelty[i] >= novelty[i + 1]
            && i - last >= min_gap
        {
            bounds.push(i as f32);
            last = i;
        }
    }
    bounds.push(duration);

    let mut out: Vec<Section> = Vec::new();
    for w in bounds.windows(2) {
        let start = w[0]; let end = w[1];
        if end - start < 1.0 { continue; }
        let lo = start as usize;
        let hi = (end as usize).min(n);
        if lo >= hi { continue; }

        let mut avg_e = 0.0f32;
        let mut bands_avg = [0.0f32; NUM_BANDS];
        let mut avg_inten = 0.0f32;
        let mut avg_density = 0.0f32;
        let mut avg_strength = 0.0f32;
        for i in lo..hi {
            avg_e += energy[i].overall;
            for b in 0..NUM_BANDS { bands_avg[b] += energy[i].bands[b]; }
            avg_inten += intensity[i].intensity;
            avg_density += intensity[i].onset_density;
            avg_strength += intensity[i].mean_strength;
        }
        let count = (hi - lo) as f32;
        avg_e /= count;
        for b in 0..NUM_BANDS { bands_avg[b] /= count; }
        avg_inten /= count;
        avg_density /= count;
        avg_strength /= count;

        let energy_label = if avg_e < 0.30 { "LOW" }
                           else if avg_e < 0.65 { "MED" }
                           else { "HIGH" }.to_string();

        let bass_d = bands_avg[0] + bands_avg[1];
        let mid_d  = bands_avg[2] + bands_avg[3];
        let high_d = bands_avg[4] + bands_avg[5] + bands_avg[6];
        let dom = if bass_d >= mid_d && bass_d >= high_d { "BASS" }
                  else if high_d >= mid_d { "BRIGHT" }
                  else { "MID" };

        let trend = if hi - lo >= 6 {
            let third = (hi - lo) / 3;
            let head: f32 = intensity[lo .. lo + third]
                .iter().map(|i| i.intensity).sum::<f32>()
                / third as f32;
            let tail: f32 = intensity[hi - third .. hi]
                .iter().map(|i| i.intensity).sum::<f32>()
                / third as f32;
            if tail > head * 1.20 { "RISING" }
            else if tail < head * 0.80 { "FALLING" }
            else { "STEADY" }
        } else { "STEADY" }.to_string();

        let intensity_class = if avg_inten >= 0.60 { "PEAK" }
                              else if avg_inten >= 0.40 { "HOT"  }
                              else if avg_inten >= 0.20 { "COOL" }
                              else { "CALM" };

        let label = if out.is_empty() && start < 1.0 {
            "intro".to_string()
        } else if intensity_class == "PEAK" && trend != "FALLING" {
            format!("drop_{}", out.len())
        } else if intensity_class == "PEAK" {
            format!("climax_{}", out.len())
        } else if trend == "RISING" {
            format!("buildup_{}", out.len())
        } else if avg_inten < 0.20 && avg_e < 0.35 {
            format!("breakdown_{}", out.len())
        } else if intensity_class == "HOT" {
            format!("groove_{}", out.len())
        } else if trend == "FALLING"
                  && (out.len() > 3 || start > duration * 0.6) {
            format!("outro_{}", out.len())
        } else {
            format!("section_{}", out.len())
        };

        let character = format!("{}_{}_{}", dom, intensity_class, trend);

        out.push(Section {
            start, end, label,
            energy_label, character,
            avg_energy: avg_e,
            avg_intensity: avg_inten,
            avg_density, avg_strength,
        });
    }

    if out.is_empty() {
        out.push(Section {
            start: 0.0, end: duration,
            label: "main".into(),
            energy_label: "UNKNOWN".into(),
            character: "STEADY".into(),
            avg_energy: 0.5,
            avg_intensity: 0.0,
            avg_density: 0.0,
            avg_strength: 0.0,
        });
    }
    out
}

// ---------- transitions ----------

struct Transition {
    time:      f32,
    kind:      &'static str,
    intensity: f32,
}

/// Transitions are now computed on the intensity curve, not
/// on the spectral overall energy, so they fire on the
/// felt-aggression edges that a level designer cares about.
fn detect_transitions_v2(intensity: &[IntensityAt]) -> Vec<Transition> {
    if intensity.len() < 6 { return Vec::new(); }
    let win = 3usize;
    let mut raw = Vec::new();
    for i in win..intensity.len() - win {
        let before: f32 = intensity[i - win .. i]
            .iter().map(|x| x.intensity).sum::<f32>() / win as f32;
        let after:  f32 = intensity[i .. i + win]
            .iter().map(|x| x.intensity).sum::<f32>() / win as f32;
        let delta = after - before;
        if delta.abs() < 0.20 { continue; }
        let kind = if delta > 0.0 {
            if before < 0.20 && after > 0.50 { "DROP" }
            else { "BUILDUP" }
        } else {
            if before > 0.50 && after < 0.20 { "BREAKDOWN" }
            else { "REDUCTION" }
        };
        raw.push(Transition {
            time: intensity[i].time, kind,
            intensity: delta.abs().min(1.0),
        });
    }
    let mut filtered: Vec<Transition> = Vec::new();
    for t in raw {
        let nearby = filtered.iter().rposition(|p| (t.time - p.time).abs() < 4.0);
        match nearby {
            Some(idx) => {
                if t.intensity > filtered[idx].intensity {
                    filtered.remove(idx);
                    filtered.push(t);
                }
            }
            None => filtered.push(t),
        }
    }
    filtered.sort_by(|a, b| a.time.partial_cmp(&b.time).unwrap());
    filtered
}

// ---------- aggression class ----------

fn aggression_class(avg_intensity: f32) -> &'static str {
    if      avg_intensity >= 0.50 { "EXTREME" }
    else if avg_intensity >= 0.30 { "HIGH"    }
    else if avg_intensity >= 0.15 { "MEDIUM"  }
    else                          { "LOW"     }
}

// ---------- report rendering ----------

struct Report<'a> {
    file:           &'a str,
    duration:       f32,
    sr:             u32,
    channels:       u16,
    bpm_candidates: &'a [BpmCandidate],
    energy:         &'a [EnergyAt],
    intensity:      &'a [IntensityAt],
    onsets:         &'a [Onset],
    sections:       &'a [Section],
    transitions:    &'a [Transition],
    clusters:       &'a [Cluster],
    drops:          &'a [Drop],
    hot_moments:    &'a [HotMoment],
}

fn fmt_time(s: f32) -> String {
    let total_ms = (s.max(0.0) * 1000.0).round() as i64;
    let m = total_ms / 60_000;
    let rem_ms = total_ms - m * 60_000;
    let s_part = rem_ms as f32 / 1000.0;
    format!("{}:{:06.3}", m, s_part)
}

fn render_report(r: &Report) -> String {
    let mut s = String::new();

    let avg_intensity: f32 = if r.intensity.is_empty() { 0.0 } else {
        r.intensity.iter().map(|i| i.intensity).sum::<f32>()
            / r.intensity.len() as f32
    };
    let class = aggression_class(avg_intensity);
    let strong = r.onsets.iter().filter(|o| o.strength >= 0.5).count();

    // ---- summary ----
    s.push_str("=== TRACK SUMMARY ===\n");
    s.push_str(&format!("file: {}\n", r.file));
    s.push_str(&format!(
        "duration: {:.3}s ({})\n", r.duration, fmt_time(r.duration)));
    s.push_str(&format!("sample_rate: {}\n", r.sr));
    s.push_str(&format!("channels: {}\n", r.channels));
    if !r.bpm_candidates.is_empty() {
        let bpms: Vec<String> = r.bpm_candidates.iter()
            .map(|c| format!("{:.0} (conf {:.2})", c.bpm, c.confidence))
            .collect();
        s.push_str(&format!("bpm_candidates: {}\n", bpms.join(" | ")));
    }
    s.push_str(&format!("aggression_class: {}\n", class));
    s.push_str(&format!("avg_intensity: {:.3}\n", avg_intensity));
    s.push_str(&format!(
        "total_onsets: {} ({:.2}/s)\n", r.onsets.len(),
        if r.duration > 0.0 { r.onsets.len() as f32 / r.duration } else { 0.0 }));
    s.push_str(&format!(
        "strong_onsets (>= 0.5): {} ({:.2}/s)\n", strong,
        if r.duration > 0.0 { strong as f32 / r.duration } else { 0.0 }));
    s.push_str(&format!("detected_drops: {}\n", r.drops.len()));
    s.push_str(&format!("detected_clusters: {}\n", r.clusters.len()));
    s.push_str(&format!("detected_sections: {}\n", r.sections.len()));
    if !r.hot_moments.is_empty() {
        let hot: Vec<String> = r.hot_moments.iter()
            .map(|h| format!("{} ({:.2})", fmt_time(h.time), h.intensity))
            .collect();
        s.push_str(&format!("hot_moments: {}\n", hot.join(" | ")));
    }
    s.push('\n');

    // ---- tempo ----
    s.push_str("=== TEMPO ===\n");
    let labels = ["primary", "second ", "third  "];
    for (i, c) in r.bpm_candidates.iter().enumerate() {
        let lbl = labels.get(i).cloned().unwrap_or("       ");
        let period = if c.bpm > 0.0 { 60.0 / c.bpm } else { 0.0 };
        s.push_str(&format!(
            "{}: {:.1} BPM | period {:.4}s | confidence {:.2} | score {:.4}\n",
            lbl, c.bpm, period, c.confidence, c.score));
    }
    s.push('\n');

    // ---- drops ----
    s.push_str(&format!("=== DROPS ({}) ===\n", r.drops.len()));
    s.push_str("time         duration  peak_intensity\n");
    for d in r.drops {
        s.push_str(&format!(
            "{:>11}  {:>7.1}s  {:.2}\n",
            fmt_time(d.time), d.duration, d.peak_intensity));
    }
    s.push('\n');

    // ---- clusters ----
    s.push_str(&format!("=== ONSET CLUSTERS ({}) ===\n", r.clusters.len()));
    s.push_str("start        end          duration  count  peak  avg\n");
    for c in r.clusters {
        s.push_str(&format!(
            "{:>11}  {:>11}  {:>7.1}s  {:>4}   {:.2}  {:.2}\n",
            fmt_time(c.start), fmt_time(c.end),
            c.end - c.start, c.count,
            c.peak_strength, c.avg_strength));
    }
    s.push('\n');

    // ---- intensity bar chart ----
    s.push_str("=== INTENSITY (per second, 0..1, 40-col bar) ===\n");
    s.push_str("time     density  mean_str  intensity\n");
    for inten in r.intensity {
        let bar_len = (inten.intensity * 40.0) as usize;
        let bar: String = "*".repeat(bar_len);
        s.push_str(&format!(
            "{:>7.1}  {:>5.1}    {:.2}      {:.2}  {}\n",
            inten.time, inten.onset_density,
            inten.mean_strength, inten.intensity, bar));
    }
    s.push('\n');

    // ---- levels ----
    s.push_str("=== LEVELS (per second, dB) ===\n");
    s.push_str("time     rms_db   peak_db  crest_db\n");
    for e in r.energy {
        s.push_str(&format!(
            "{:>7.1}  {:>+7.1}  {:>+7.1}   {:>5.1}\n",
            e.time, e.rms_db, e.peak_db, e.crest_db));
    }
    s.push('\n');

    // ---- band energy ----
    s.push_str("=== ENERGY (per second, normalized 0..1) ===\n");
    s.push_str("time     overall  ");
    for n in BAND_NAMES.iter() {
        s.push_str(&format!("{:<7}", n));
    }
    s.push('\n');
    for e in r.energy {
        s.push_str(&format!("{:>7.1}  {:.2}     ", e.time, e.overall));
        for b in 0..NUM_BANDS {
            s.push_str(&format!("{:.2}   ", e.bands[b]));
        }
        s.push('\n');
    }
    s.push('\n');

    // ---- strong onsets only ----
    s.push_str(&format!(
        "=== STRONG ONSETS (>= 0.5, {} total of {}) ===\n",
        strong, r.onsets.len()));
    s.push_str("time         strength\n");
    for o in r.onsets {
        if o.strength < 0.5 { continue; }
        s.push_str(&format!(
            "{:>11}  {:.2}\n", fmt_time(o.time), o.strength));
    }
    s.push('\n');

    // ---- transitions ----
    s.push_str(&format!(
        "=== TRANSITIONS ({}) ===\n", r.transitions.len()));
    s.push_str("time         kind         intensity\n");
    for t in r.transitions {
        s.push_str(&format!(
            "{:>11}  {:<11}  {:.2}\n",
            fmt_time(t.time), t.kind, t.intensity));
    }
    s.push('\n');

    // ---- sections ----
    s.push_str(&format!("=== SECTIONS ({}) ===\n", r.sections.len()));
    s.push_str(
        "start        end          label                  \
         energy   inten   dens    character\n");
    for sec in r.sections {
        s.push_str(&format!(
            "{:>11}  {:>11}  {:<22}  {:<7}  {:.2}    {:>5.1}   {}\n",
            fmt_time(sec.start), fmt_time(sec.end),
            sec.label, sec.energy_label,
            sec.avg_intensity, sec.avg_density, sec.character));
    }
    s.push('\n');

    // ---- skeleton ----
    let primary_bpm = r.bpm_candidates.first()
        .map(|c| c.bpm).unwrap_or(120.0);
    s.push_str("=== SUGGESTED RLF SKELETON ===\n");
    s.push_str("// Auto-generated. Section labels reflect detected\n");
    s.push_str("// segments. Drops, climaxes and grooves get heavier\n");
    s.push_str("// triggers; breakdowns and outros relax the field.\n\n");
    s.push_str("#[use_v3]\n");
    s.push_str("#[timestamp_format_use_tracklength]\n\n");
    s.push_str(&format!(
        "level \"GENERATED\" do\n    meta do\n        bpm = {}\n        \
         music = ~p\"{}\"\n    end\n\n",
        primary_bpm.round() as u32, r.file));
    s.push_str(
        "    palette    do bgA = rgb(0.10, 0.04, 0.16) end\n    \
         difficulty do range = :Rookie..:Expert, base = :Casual end\n    \
         generation do sides = 6, seed = 0x1 end\n\n");
    for sec in r.sections {
        s.push_str(&format!(
            "    // {} | energy={} avg_e={:.2} \
             intensity={:.2} density={:.1} character={}\n",
            sec.label, sec.energy_label, sec.avg_energy,
            sec.avg_intensity, sec.avg_density, sec.character));
        s.push_str(&format!(
            "    section \"{}\" at ~t\"{}\" do\n",
            sec.label, fmt_time(sec.start)));
        emit_section_template(&mut s, sec);
        s.push_str("    end\n\n");
    }
    s.push_str("end\n\n");

    s.push_str("=== END ===\n");
    s
}

/// Pick a stub trigger / emit pattern based on the section's
/// label and intensity. The output is deliberately simple:
/// the level designer reading the report will replace these
/// with hand-tuned content, but the defaults already match
/// the section's character so a draft load is playable.
fn emit_section_template(out: &mut String, sec: &Section) {
    let label = sec.label.as_str();
    let inten = sec.avg_intensity;

    if label.starts_with("drop") || label.starts_with("climax") || inten >= 0.60 {
        out.push_str(
            "        trigger :bassdrop { strength = 1.0, duration = 1.4 }\n            \
             |> :flip\n            \
             |> :speedwarp { walls = 1.35, rotation = 1.2, duration = 6.0 }\n            \
             |> :ringburst { count = 4, duration = 1.6 }\n");
        out.push_str(
            "        emit staircase { dir = :cw, steps = 8 }\n        \
             wait 4\n        emit cubes { layers = 5, dir = :cw }\n        wait 4\n");
    } else if label.starts_with("buildup") {
        out.push_str(
            "        trigger :speedwarp { walls = 1.2, music = 1.0, duration = 5.0 }\n            \
             |> :outline { thickness = 0.6, duration = 5.0 }\n");
        out.push_str(
            "        emit spiral { dir = :cw, loops = 3 }\n        \
             wait 4\n        emit ladder { rungs = 6 }\n        wait 4\n");
    } else if label.starts_with("breakdown") {
        out.push_str(
            "        trigger :fog { near = 0.3, far = 0.85, duration = 8.0 }\n            \
             |> :grayscale { strength = 0.5, duration = 6.0 }\n");
        out.push_str(
            "        emit bar { thickness = 0.8 }\n        wait 6\n        \
             emit alternate { parity = :even, thickness = 0.8 }\n        wait 4\n");
    } else if label.starts_with("groove") && inten >= 0.40 {
        out.push_str(
            "        emit corridor { length = 1.4, turns = 4, dir = :cw }\n        \
             wait 6\n        emit pinwheel { spokes = 5 }\n        \
             wait 4\n        emit doubleBar { spacing = 2 }\n        wait 4\n");
    } else if label.starts_with("groove") {
        out.push_str(
            "        emit pinwheel { spokes = 4 }\n        wait 4\n        \
             emit ladder { rungs = 6 }\n        wait 4\n");
    } else if label.starts_with("outro") {
        out.push_str(
            "        trigger :speedwarp { walls = 0.6, music = 1.0, duration = 5.0 }\n            \
             |> :zoom { target = 0.7, anim = :ease_in_out, duration = 3.0 }\n            \
             |> :grayscale { strength = 0.6, duration = 5.0 }\n");
        out.push_str("        emit pinwheel { spokes = 4 }\n        wait 4\n");
    } else if label == "intro" {
        out.push_str(
            "        trigger :fog { near = 0.4, far = 0.9, duration = 8.0 }\n");
        out.push_str("        emit bar\n        wait 4\n");
    } else {
        out.push_str(
            "        emit alternate { parity = :even }\n        wait 2\n        \
             emit spiral { dir = :cw, loops = 2 }\n        wait 2\n");
    }
}

// ---------- main ----------

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("usage: atomize <track.qoa> [output.txt]");
        std::process::exit(1);
    }
    let input = args[1].clone();
    let output = args.get(2).cloned();

    eprintln!("[atomize] loading {}", input);
    let audio = match qoa::decode_file(&input) {
        Some(a) => a,
        None => {
            eprintln!("[atomize] failed to decode {}", input);
            std::process::exit(2);
        }
    };
    let total_frames = audio.samples.len() / audio.channels.max(1) as usize;
    let duration = total_frames as f32 / audio.sample_rate as f32;
    eprintln!(
        "[atomize] decoded: {} frames, {} ch, {} Hz, {:.2}s",
        total_frames, audio.channels, audio.sample_rate, duration);

    let ch = audio.channels.max(1) as usize;
    let mut mono = Vec::with_capacity(total_frames);
    for i in 0..total_frames {
        let mut acc = 0.0f32;
        for c in 0..ch {
            acc += audio.samples[i * ch + c] as f32 / 32768.0;
        }
        mono.push(acc / ch as f32);
    }

    eprintln!("[atomize] running spectral pass ...");
    let (frames, flux) = spectral_pass(&mono, audio.sample_rate);
    let frame_rate = audio.sample_rate as f32 / HOP_SIZE as f32;

    eprintln!("[atomize] detecting onsets ...");
    let (osn, onsets) = detect_onsets(&flux, frame_rate);
    eprintln!("[atomize] {} onsets", onsets.len());

    eprintln!("[atomize] estimating bpm candidates ...");
    let bpm_candidates = estimate_bpm_candidates(&osn, frame_rate);
    if !bpm_candidates.is_empty() {
        let parts: Vec<String> = bpm_candidates.iter()
            .map(|c| format!("{:.0}({:.2})", c.bpm, c.confidence))
            .collect();
        eprintln!("[atomize] bpm candidates: {}", parts.join(" | "));
    }

    eprintln!("[atomize] computing energy profile ...");
    let energy = energy_per_second(&frames, frame_rate, duration);

    eprintln!("[atomize] computing intensity curve ...");
    let intensity = intensity_per_second(&onsets, &energy, duration);

    eprintln!("[atomize] detecting onset clusters ...");
    let clusters = detect_clusters(&onsets);
    eprintln!("[atomize] {} clusters", clusters.len());

    eprintln!("[atomize] detecting drops ...");
    let drops = detect_drops(&intensity);
    eprintln!("[atomize] {} drops", drops.len());

    eprintln!("[atomize] detecting hot moments ...");
    let hot_moments = detect_hot_moments(&intensity);
    eprintln!("[atomize] {} hot moments", hot_moments.len());

    eprintln!("[atomize] segmenting sections ...");
    let sections = detect_sections_v2(&energy, &intensity, duration);
    eprintln!("[atomize] {} sections", sections.len());

    eprintln!("[atomize] detecting transitions ...");
    let transitions = detect_transitions_v2(&intensity);
    eprintln!("[atomize] {} transitions", transitions.len());

    let avg_int: f32 = if intensity.is_empty() { 0.0 } else {
        intensity.iter().map(|i| i.intensity).sum::<f32>()
            / intensity.len() as f32
    };
    eprintln!(
        "[atomize] aggression class: {} (avg_intensity {:.3})",
        aggression_class(avg_int), avg_int);

    let rep = Report {
        file: &input,
        duration,
        sr: audio.sample_rate,
        channels: audio.channels,
        bpm_candidates: &bpm_candidates,
        energy: &energy,
        intensity: &intensity,
        onsets: &onsets,
        sections: &sections,
        transitions: &transitions,
        clusters: &clusters,
        drops: &drops,
        hot_moments: &hot_moments,
    };
    let text = render_report(&rep);

    match output {
        Some(p) => {
            let mut f = match File::create(&p) {
                Ok(f) => f,
                Err(e) => {
                    eprintln!("[atomize] cannot create {}: {}", p, e);
                    std::process::exit(3);
                }
            };
            if let Err(e) = f.write_all(text.as_bytes()) {
                eprintln!("[atomize] write failed: {}", e);
                std::process::exit(4);
            }
            eprintln!("[atomize] wrote {}", p);
        }
        None => {
            print!("{}", text);
        }
    }
}