//! The sound coming out of the speakers, as a row of bars.
//!
//! One PipeWire stream on the default sink's *monitor* — what is being played, not what a microphone hears —
//! feeds a windowed FFT, and each transform is folded into the handful of log-spaced bands a visualiser draws.
//! The shell's usual rule applies: one capture for the whole process, however many surfaces subscribe.
//!
//! **A visualiser is the one service that can undo the idle budget**, because its data source never stops: a
//! monitor stream delivers silence at exactly the same rate it delivers music, so a naive producer would wake
//! every surface sixty times a second in front of a paused player. Two things prevent that. Nothing starts
//! until something subscribes — `Service` is lazy — and a frame identical to the one before it is not
//! published, so silence costs one final all-zero frame and then nothing at all until sound returns. That is
//! also what gives every consumer its auto-hide for free: [`Spectrum::silent`] is a reading, not a timer.

use std::collections::VecDeque;
use std::ops::ControlFlow;
use std::sync::Arc;
use std::time::Duration;

use platform_wayland::EventSender;
use rustfft::num_complex::Complex32;

use crate::pwstream;
use config::VisualiserConfig;
use util::broadcast::{Broadcast, Service};

/// Samples per second asked of the capture. Chosen over 48 kHz so the top band sits near the limit of what a
/// person hears rather than a couple of empty bins above it; PipeWire resamples either way.
const RATE: u32 = 44_100;

/// Samples per transform. 2048 at 44.1 kHz is a 21.5 Hz bin and a 46 ms window — fine enough to separate a
/// bass line from a kick, short enough that a bar follows the note rather than trailing it.
const WINDOW: usize = 2048;

/// The band edges, in hertz. Below 40 Hz is rumble no speaker reproduces and above 16 kHz is bin noise; both
/// only ever contribute a bar that never moves.
const LOW_HZ: f32 = 40.0;
const HIGH_HZ: f32 = 16_000.0;

/// Bands under this are "the bass" for beat detection — a kick drum and the low end of a bass guitar.
const BEAT_HZ: f32 = 150.0;

/// How much history the beat detector compares against. Long enough to average over a bar of music, short
/// enough to follow a track getting louder rather than calling every beat of it.
const BEAT_HISTORY: Duration = Duration::from_millis(1500);

/// The shortest gap between two beats, as a fraction of the frame rate — about 8 per second, which is past any
/// tempo a person taps to and still stops one kick from registering as three.
const BEAT_REFRACTORY_HZ: f32 = 8.0;

/// A band quieter than this reads as nothing. Snapping it to zero is what lets an unchanged frame be *equal* to
/// the one before it, which is what stops a silent room waking the compositor sixty times a second.
const EPSILON: f32 = 0.001;

/// How long to wait before re-attaching after the capture exits. Only reached when PipeWire restarted.
const REATTACH: Duration = Duration::from_secs(3);

/// One transform's worth of sound.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Spectrum {
    /// One normalised 0–1 magnitude per band, low frequencies first. As many entries as `[visualiser] bars`.
    pub bars: Arc<[f32]>,
    /// Overall loudness, 0–1, on the same curve the bars use. What a single meter draws.
    pub level: f32,
    /// A transient landed in the bass on this frame. Momentary: true for one frame, not for the beat's length.
    pub beat: bool,
    /// Nothing is playing. A consumer hides on this rather than on `level == 0.0`, because it is also what the
    /// producer stops publishing on — the last frame before the quiet always carries it.
    pub silent: bool,
}

impl Spectrum {
    /// A spectrum with `bars` bands, all silent. What a surface draws before the first frame arrives, and what
    /// the producer publishes once when the sound stops.
    pub fn quiet(bars: usize) -> Self {
        Self {
            bars: vec![0.0; bars].into(),
            level: 0.0,
            beat: false,
            silent: true,
        }
    }
}

static SPECTRUM: Service<Spectrum> = Service::new("hogar-shell-visualiser", run);

pub fn subscribe(tx: EventSender<Spectrum>) {
    SPECTRUM.subscribe(tx);
}

/// The last published spectrum, without touching PipeWire.
pub fn current() -> Option<Spectrum> {
    SPECTRUM.current()
}

fn settings() -> VisualiserConfig {
    config::shared_config()
        .map(|config| config.visualiser)
        .unwrap_or_default()
}

fn run(out: &Arc<Broadcast<Spectrum>>) {
    let config = settings();
    out.publish(Spectrum::quiet(config.band_count()));

    let mut attached = false;
    loop {
        match capture(out, &config) {
            Ok(Wanted::No) => return,
            Ok(Wanted::Yes) => {
                attached = true;
                tracing::warn!("the audio capture exited; re-attaching");
            }
            // The same retirement `pipewire` makes: a machine with no PipeWire does not grow one while the
            // shell runs, so retrying forever would fork a doomed process all day.
            Err(e) if !attached => {
                tracing::info!("no audio capture ({e}); the visualiser will stay silent");
                return;
            }
            Err(e) => tracing::warn!("cannot re-attach the audio capture ({e}); retrying"),
        }
        out.publish(Spectrum::quiet(config.band_count()));
        if !out.wanted() {
            return;
        }
        std::thread::sleep(REATTACH);
    }
}

/// Why a capture stopped: because the stream did, or because the last spectrum widget went away.
enum Wanted {
    Yes,
    No,
}

/// Runs one capture to completion, publishing a spectrum per hop that differs from the one before it.
///
/// This is the one service that publishes at a frame rate, so it is also the one that must notice soonest that
/// nobody is watching: the check runs per hop and breaks the stream itself rather than waiting for a reattach,
/// which is what stops an idle machine paying for an FFT nothing draws.
fn capture(out: &Arc<Broadcast<Spectrum>>, config: &VisualiserConfig) -> std::io::Result<Wanted> {
    let hop = (RATE as f32 / config.rate() as f32).round().max(64.0) as usize;
    let mut analyser = Analyser::new(config, hop);
    let mut last = Spectrum::quiet(config.band_count());
    let mut wanted = Wanted::Yes;

    let captured = pwstream::monitor(RATE, hop, &mut |samples| {
        let next = analyser.push(samples);
        // Silence is a state, not a stream of frames: publishing the same all-zero spectrum sixty times a
        // second is exactly the idle cost this service exists to avoid.
        if next != last {
            last = next.clone();
            out.publish(next);
        }
        if out.wanted() {
            ControlFlow::Continue(())
        } else {
            wanted = Wanted::No;
            ControlFlow::Break(())
        }
    });
    captured.map(|()| wanted)
}

/// The sliding window, the transform, and the state that smooths one frame into the next.
struct Analyser {
    plan: Arc<dyn rustfft::Fft<f32>>,
    /// Hann coefficients, precomputed — the same multiply runs on every hop for the life of the process.
    window: Vec<f32>,
    /// The last `WINDOW` samples, oldest first. Each hop shifts it along by `hop`.
    history: Vec<f32>,
    /// The transform's buffer: the windowed samples going in, the spectrum coming out.
    scratch: Vec<Complex32>,
    /// Working space the transform needs beside its buffer, held rather than asked for each hop.
    ///
    /// `Fft::process` allocates this itself on every call — `vec![Complex::zero(); get_inplace_scratch_len()]`,
    /// straight from its source — and the visualiser runs one per audio hop for as long as anything is
    /// listening. It was the single largest source of transient allocation in the shell: 66,571 of the 66,839
    /// allocations at that call site were freed almost immediately, 99.6%, and that churn is what keeps glibc's
    /// arenas fragmented and RSS above the heap that is actually live.
    fft_scratch: Vec<Complex32>,
    /// The first and last FFT bin of each band, inclusive.
    bands: Vec<(usize, usize)>,
    /// How many leading bands count as bass for beat detection.
    beat_bands: usize,
    /// Smoothed bar heights, carried between frames.
    smoothed: Vec<f32>,
    /// Recent bass energies, oldest first, for the beat's moving average.
    history_energy: VecDeque<f32>,
    energy_capacity: usize,
    /// Frames still to wait before another beat may be reported.
    refractory: u32,
    refractory_frames: u32,
    attack: f32,
    decay: f32,
    floor_db: f32,
    gain: f32,
    sensitivity: f32,
}

impl Analyser {
    fn new(config: &VisualiserConfig, hop: usize) -> Self {
        let bars = config.band_count();
        let bands = band_bins(bars);
        let bin_hz = RATE as f32 / WINDOW as f32;
        let beat_bands = bands
            .iter()
            .take_while(|(start, _)| *start as f32 * bin_hz < BEAT_HZ)
            .count()
            .max(1);
        let frames_per_second = RATE as f32 / hop as f32;

        let plan = rustfft::FftPlanner::new().plan_fft_forward(WINDOW);
        Self {
            fft_scratch: vec![Complex32::default(); plan.get_inplace_scratch_len()],
            plan,
            window: (0..WINDOW)
                .map(|n| 0.5 * (1.0 - (std::f32::consts::TAU * n as f32 / WINDOW as f32).cos()))
                .collect(),
            history: vec![0.0; WINDOW],
            scratch: vec![Complex32::default(); WINDOW],
            bands,
            beat_bands,
            smoothed: vec![0.0; bars],
            history_energy: VecDeque::new(),
            energy_capacity: (BEAT_HISTORY.as_secs_f32() * frames_per_second)
                .round()
                .max(4.0) as usize,
            refractory: 0,
            refractory_frames: (frames_per_second / BEAT_REFRACTORY_HZ).round().max(1.0) as u32,
            attack: config.attack(),
            decay: config.decay(),
            floor_db: config.floor_db(),
            gain: config.gain(),
            sensitivity: config.sensitivity(),
        }
    }

    /// Slides `samples` into the window and returns the spectrum they produce.
    fn push(&mut self, samples: &[f32]) -> Spectrum {
        let fresh = samples.len().min(self.history.len());
        let keep = self.history.len() - fresh;
        self.history.copy_within(fresh.., 0);
        self.history[keep..].copy_from_slice(&samples[samples.len() - fresh..]);

        for (slot, (sample, weight)) in self
            .scratch
            .iter_mut()
            .zip(self.history.iter().zip(&self.window))
        {
            *slot = Complex32::new(sample * weight, 0.0);
        }
        self.plan
            .process_with_scratch(&mut self.scratch, &mut self.fft_scratch);

        // A full-scale sine puts half its energy in each of two mirrored bins and the Hann window halves the
        // amplitude again, so this is the factor that makes such a tone read as exactly 1.0.
        let normalise = 4.0 / WINDOW as f32;
        let bars: Vec<f32> = (0..self.bands.len())
            .map(|band| {
                let (start, end) = self.bands[band];
                // The loudest bin in the band, not their mean: a band spanning an octave of the top end is
                // mostly empty, and averaging it flattens every cymbal into the noise floor beside it.
                let peak = self.scratch[start..=end]
                    .iter()
                    .map(|bin| bin.norm())
                    .fold(0.0f32, f32::max);
                self.normalise(peak * normalise)
            })
            .collect();

        for (current, target) in self.smoothed.iter_mut().zip(&bars) {
            let rate = if *target > *current {
                self.attack
            } else {
                self.decay
            };
            *current += (*target - *current) * rate;
            if *current < EPSILON {
                *current = 0.0;
            }
        }

        let rms = (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt();
        let level = self.normalise(rms * std::f32::consts::SQRT_2);
        let bass = &self.smoothed[..self.beat_bands];
        let energy = bass.iter().sum::<f32>() / bass.len() as f32;
        let beat = self.beat(energy);
        let silent = self.smoothed.iter().all(|bar| *bar == 0.0) && level < EPSILON;

        Spectrum {
            bars: self.smoothed.clone().into(),
            level: if silent { 0.0 } else { level },
            beat,
            silent,
        }
    }

    /// An amplitude on the 0–1 curve the bars are drawn against: decibels, floored, then rescaled.
    ///
    /// Linear amplitude is the wrong axis for a visualiser for the same reason it is the wrong axis for a
    /// volume slider — a bar drawn from it spends its life within a few pixels of the bottom.
    fn normalise(&self, amplitude: f32) -> f32 {
        let amplitude = amplitude * self.gain;
        if amplitude <= 0.0 {
            return 0.0;
        }
        let db = 20.0 * amplitude.log10();
        ((db - self.floor_db) / -self.floor_db).clamp(0.0, 1.0)
    }

    /// Whether the bass just jumped clear of where it has recently been.
    ///
    /// A ratio against a moving average rather than an absolute threshold: any fixed number is either deaf to a
    /// quiet track or triggered continuously by a loud one, and the thing a beat *is* is a transient relative to
    /// the music around it.
    fn beat(&mut self, energy: f32) -> bool {
        let average = if self.history_energy.is_empty() {
            0.0
        } else {
            self.history_energy.iter().sum::<f32>() / self.history_energy.len() as f32
        };
        if self.history_energy.len() == self.energy_capacity {
            self.history_energy.pop_front();
        }
        self.history_energy.push_back(energy);

        if self.refractory > 0 {
            self.refractory -= 1;
            return false;
        }
        // The floor is what stops the ratio finding beats in silence, where any sample at all is infinitely
        // louder than an average of nothing.
        let struck = energy > EPSILON * 20.0 && energy > average * self.sensitivity;
        if struck {
            self.refractory = self.refractory_frames;
        }
        struck
    }
}

/// The FFT bins each band covers, log-spaced from [`LOW_HZ`] to [`HIGH_HZ`].
///
/// Every band gets at least one bin, and no bin is shared: at the bottom the log spacing asks for bands
/// narrower than one bin, and letting them overlap draws four identical bass bars instead of a slope.
fn band_bins(bars: usize) -> Vec<(usize, usize)> {
    let bin_hz = RATE as f32 / WINDOW as f32;
    let top = WINDOW / 2 - 1;
    let ratio = (HIGH_HZ / LOW_HZ).ln();
    let mut bands = Vec::with_capacity(bars);
    let mut next = (LOW_HZ / bin_hz).round().max(1.0) as usize;
    for band in 0..bars {
        let upper_hz = LOW_HZ * (ratio * (band + 1) as f32 / bars as f32).exp();
        let start = next.min(top);
        let end = ((upper_hz / bin_hz).round() as usize).clamp(start, top);
        bands.push((start, end));
        next = (end + 1).min(top);
    }
    bands
}

#[cfg(test)]
mod tests {
    use super::*;

    fn analyser(bars: usize) -> Analyser {
        Analyser::new(
            &VisualiserConfig {
                bars: bars as u32,
                ..VisualiserConfig::default()
            },
            735,
        )
    }

    /// `hop` samples of a sine at `hz`, full scale.
    fn tone(hz: f32, hop: usize) -> Vec<f32> {
        (0..hop)
            .map(|n| (std::f32::consts::TAU * hz * n as f32 / RATE as f32).sin())
            .collect()
    }

    #[test]
    fn the_bands_cover_the_spectrum_without_sharing_a_bin() {
        let bands = band_bins(48);
        assert_eq!(bands.len(), 48);
        for pair in bands.windows(2) {
            let (_, first_end) = pair[0];
            let (second_start, _) = pair[1];
            assert!(
                second_start > first_end,
                "two bands share bin {first_end}, so they draw the same height for ever"
            );
        }
        for (start, end) in bands {
            assert!(
                start <= end,
                "a band with no bins is a bar that never moves"
            );
            assert!(
                end < WINDOW / 2,
                "a band reaching past Nyquist reads mirror noise"
            );
        }
    }

    #[test]
    fn a_band_count_of_one_still_produces_one_band() {
        // The config clamps, but the geometry has to survive the edge on its own — `bars.windows(2)` above
        // never runs for a single band, so nothing else would catch a panic here.
        assert_eq!(band_bins(1).len(), 1);
        let _ = analyser(1).push(&tone(440.0, 735));
    }

    #[test]
    fn silence_reads_as_silent_rather_than_as_a_frame_of_zeroes() {
        // The distinction the whole idle budget rests on: a consumer hides on `silent`, and the producer
        // publishes nothing more once it is true.
        let mut analyser = analyser(24);
        let mut spectrum = analyser.push(&vec![0.0; 735]);
        for _ in 0..64 {
            spectrum = analyser.push(&vec![0.0; 735]);
        }
        assert!(spectrum.silent);
        assert_eq!(spectrum.level, 0.0);
        assert!(spectrum.bars.iter().all(|bar| *bar == 0.0));
    }

    #[test]
    fn a_silent_frame_equals_the_one_before_it() {
        // `capture` publishes on inequality, so this equality *is* the mechanism — a spectrum carrying a
        // decaying tail of 1e-9s would compare unequal for ever and wake every surface sixty times a second.
        let mut analyser = analyser(24);
        for _ in 0..64 {
            analyser.push(&vec![0.0; 735]);
        }
        assert_eq!(
            analyser.push(&vec![0.0; 735]),
            analyser.push(&vec![0.0; 735])
        );
    }

    #[test]
    fn a_tone_lands_in_the_band_that_covers_it() {
        let mut analyser = analyser(24);
        // Several hops, because one fills only a third of the window — the rest is still the zeroed history.
        let mut spectrum = Spectrum::default();
        for _ in 0..6 {
            spectrum = analyser.push(&tone(1000.0, 735));
        }

        let bin_hz = RATE as f32 / WINDOW as f32;
        let expected = band_bins(24)
            .into_iter()
            .position(|(start, end)| {
                (start as f32 * bin_hz..=(end + 1) as f32 * bin_hz).contains(&1000.0)
            })
            .expect("1 kHz is inside the covered range");
        let loudest = spectrum
            .bars
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .map(|(index, _)| index)
            .expect("there are bands");
        assert_eq!(loudest, expected, "bars: {:?}", spectrum.bars);
        assert!(!spectrum.silent);
    }

    #[test]
    fn a_full_scale_tone_reaches_the_top_of_the_scale() {
        // The normalisation is the one number in here with an absolute right answer, and getting it wrong is
        // invisible — every bar simply sits low, which reads as "quiet music" rather than as a bug.
        let mut analyser = analyser(24);
        let mut spectrum = Spectrum::default();
        for _ in 0..40 {
            spectrum = analyser.push(&tone(1000.0, 735));
        }
        let peak = spectrum.bars.iter().fold(0.0f32, |a, b| a.max(*b));
        assert!(
            peak > 0.9,
            "a full-scale sine should fill its bar; got {peak}"
        );
    }

    #[test]
    fn a_beat_is_reported_once_rather_than_for_its_whole_length() {
        // A kick held for a tenth of a second is one beat. Without the refractory it is six, and anything
        // pulsing on `beat` flickers instead of pulsing.
        let mut analyser = analyser(24);
        for _ in 0..40 {
            analyser.push(&vec![0.0; 735]);
        }
        let kick = tone(60.0, 735);
        let beats = (0..8).filter(|_| analyser.push(&kick).beat).count();
        assert_eq!(
            beats, 1,
            "the transient is one beat, however long it is held"
        );
    }

    #[test]
    fn a_quiet_spectrum_has_the_band_count_it_was_asked_for() {
        // What a surface draws before the first frame: the row has to be the right width immediately, or the
        // bars visibly reflow the moment sound starts.
        assert_eq!(Spectrum::quiet(32).bars.len(), 32);
        assert!(Spectrum::quiet(32).silent);
    }
}

#[cfg(test)]
mod live {
    use super::*;

    /// Captures whatever the speakers are playing and prints the bars, to check the three things a unit test
    /// cannot: that the format negotiates at all, that `stream.capture.sink` really turns the stream around
    /// onto the sink's monitor rather than onto a microphone, and that a buffer read as `f32` is one. Play
    /// something, then:
    /// `TELAR_LIVE_VISUALISER=1 cargo test -p hogar-shell --lib live_capture -- --nocapture`
    #[test]
    fn live_capture() {
        if std::env::var("TELAR_LIVE_VISUALISER").is_err() {
            eprintln!("set TELAR_LIVE_VISUALISER to capture real audio; skipping");
            return;
        }
        let config = VisualiserConfig::default();
        let hop = (RATE as f32 / config.rate() as f32).round() as usize;
        let mut analyser = Analyser::new(&config, hop);
        let mut frame = 0;
        pwstream::monitor(RATE, hop, &mut |samples| {
            assert_eq!(samples.len(), hop, "a hop is what the consumer asked for");
            let spectrum = analyser.push(samples);
            if frame % 10 == 0 {
                let art: String = spectrum
                    .bars
                    .iter()
                    .map(|b| " ▁▂▃▄▅▆▇█".chars().nth((b * 8.0) as usize).unwrap_or('█'))
                    .collect();
                println!(
                    "{art} level={:.2} beat={} silent={}",
                    spectrum.level, spectrum.beat, spectrum.silent
                );
            }
            frame += 1;
            match frame < 120 {
                true => ControlFlow::Continue(()),
                false => ControlFlow::Break(()),
            }
        })
        .expect("the capture starts");
    }
}
