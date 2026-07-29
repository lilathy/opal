/** Soft UI sounds (Web Audio - no asset files). */

type ToneOpts = {
  type?: OscillatorType;
  freq: number;
  freqEnd?: number;
  start: number;
  duration: number;
  peak?: number;
  dest: AudioNode;
};

let sharedCtx: AudioContext | null = null;
let triangleWave: PeriodicWave | null = null;
let softClipCurve: Float32Array | null = null;
let outputBus: GainNode | null = null;

/** Global loudness - bump here to raise every SFX together. */
const MASTER_GAIN = 2.15;

/** Prefer a high, stable render rate when the device allows it. */
const PREFERRED_SAMPLE_RATE = 48_000;

/** High-resolution soft-clip curve (oversampled in the node). */
const SOFT_CLIP_SAMPLES = 8192;

function getCtx(): AudioContext | null {
  try {
    const AC =
      window.AudioContext ??
      (window as unknown as { webkitAudioContext?: typeof AudioContext })
        .webkitAudioContext;
    if (!AC) return null;
    if (!sharedCtx) {
      try {
        sharedCtx = new AC({
          latencyHint: "interactive",
          sampleRate: PREFERRED_SAMPLE_RATE,
        });
      } catch {
        sharedCtx = new AC({ latencyHint: "interactive" });
      }
      triangleWave = null;
      outputBus = null;
    }
    return sharedCtx;
  } catch {
    return null;
  }
}

/** Band-limited triangle - same shape, far less high-frequency aliasing. */
function getTriangleWave(ctx: AudioContext): PeriodicWave {
  if (triangleWave) return triangleWave;
  // Cap harmonics below Nyquist so high notes stay clean.
  const nyquist = ctx.sampleRate * 0.45;
  const maxHarmonic = Math.min(63, Math.floor(nyquist / 40));
  const real = new Float32Array(maxHarmonic + 1);
  const imag = new Float32Array(maxHarmonic + 1);
  for (let h = 1; h <= maxHarmonic; h += 2) {
    const sign = ((h - 1) / 2) % 2 === 0 ? 1 : -1;
    imag[h] = (sign * 8) / (Math.PI * Math.PI * h * h);
  }
  triangleWave = ctx.createPeriodicWave(real, imag, {
    disableNormalization: false,
  });
  return triangleWave;
}

function getSoftClipCurve(): Float32Array {
  if (softClipCurve) return softClipCurve;
  const curve = new Float32Array(SOFT_CLIP_SAMPLES);
  // Gentle tanh ceiling - keeps stacked voices from harsh digital clip.
  const drive = 1.15;
  for (let i = 0; i < SOFT_CLIP_SAMPLES; i++) {
    const x = (i * 2) / (SOFT_CLIP_SAMPLES - 1) - 1;
    curve[i] = Math.tanh(x * drive) / Math.tanh(drive);
  }
  softClipCurve = curve;
  return curve;
}

/** One shared mastering chain per AudioContext. */
function getOutputBus(ctx: AudioContext): GainNode {
  if (outputBus && outputBus.context === ctx) return outputBus;

  const bus = ctx.createGain();
  bus.gain.value = 1;

  // Light bus compressor: tames stacked-peak clipping without remolding the SFX.
  const comp = ctx.createDynamicsCompressor();
  comp.threshold.value = -8;
  comp.knee.value = 18;
  comp.ratio.value = 2.4;
  comp.attack.value = 0.003;
  comp.release.value = 0.12;

  const clip = ctx.createWaveShaper();
  clip.curve = getSoftClipCurve();
  clip.oversample = "4x";

  bus.connect(comp);
  comp.connect(clip);
  clip.connect(ctx.destination);
  outputBus = bus;
  return bus;
}

function tone(ctx: AudioContext, opts: ToneOpts) {
  const {
    type = "sine",
    freq,
    freqEnd,
    start,
    duration,
    peak = 0.08,
    dest,
  } = opts;
  const osc = ctx.createOscillator();
  const gain = ctx.createGain();

  if (type === "triangle") {
    osc.setPeriodicWave(getTriangleWave(ctx));
  } else {
    osc.type = type;
  }

  osc.frequency.setValueAtTime(freq, start);
  if (freqEnd != null) {
    osc.frequency.exponentialRampToValueAtTime(
      Math.max(40, freqEnd),
      start + duration,
    );
  }

  // Same envelope shape as before, with a tiny linear settle to exact 0
  // so the oscillator stop is click-free at high sample rates.
  const attack = Math.min(0.01, duration * 0.35);
  const releaseStart = start + duration;
  gain.gain.setValueAtTime(0.0001, start);
  gain.gain.exponentialRampToValueAtTime(peak, start + attack);
  gain.gain.exponentialRampToValueAtTime(0.0001, releaseStart);
  gain.gain.linearRampToValueAtTime(0, releaseStart + 0.02);

  osc.connect(gain);
  gain.connect(dest);
  osc.start(start);
  osc.stop(releaseStart + 0.04);
  osc.onended = () => {
    try {
      osc.disconnect();
      gain.disconnect();
    } catch {
      /* already torn down */
    }
  };
}

function withAudio(
  run: (ctx: AudioContext, t0: number, dest: AudioNode) => void,
): void {
  const ctx = getCtx();
  if (!ctx) return;
  void ctx.resume().then(() => {
    const master = ctx.createGain();
    master.gain.value = MASTER_GAIN;
    master.connect(getOutputBus(ctx));
    run(ctx, ctx.currentTime + 0.02, master);
  });
}

/**
 * Ascending major sparkle - funds received / collected.
 * C6 → E6 → G6.
 */
export function playIncomingSound(): void {
  withAudio((ctx, t, dest) => {
    const notes = [1046.5, 1318.5, 1568.0];
    const step = 0.07;

    notes.forEach((freq, i) => {
      const at = t + i * step;
      tone(ctx, {
        type: "sine",
        freq,
        freqEnd: freq * 1.02,
        start: at,
        duration: 0.22,
        peak: 0.44 - i * 0.03,
        dest,
      });
      tone(ctx, {
        type: "triangle",
        freq: freq * 0.5,
        freqEnd: freq * 0.52,
        start: at,
        duration: 0.18,
        peak: 0.18,
        dest,
      });
      tone(ctx, {
        type: "sine",
        freq: freq * 2,
        freqEnd: freq * 2.04,
        start: at + 0.008,
        duration: 0.1,
        peak: 0.09,
        dest,
      });
    });

    const last = notes[notes.length - 1]!;
    tone(ctx, {
      type: "sine",
      freq: last,
      freqEnd: last * 0.98,
      start: t + notes.length * step,
      duration: 0.32,
      peak: 0.28,
      dest,
    });
  });
}

/**
 * Soft whoosh + settle - soft “didn’t work” without screaming.
 */
export function playErrorSound(): void {
  withAudio((ctx, t, dest) => {
    tone(ctx, {
      type: "sine",
      freq: 720,
      freqEnd: 280,
      start: t,
      duration: 0.22,
      peak: 0.22,
      dest,
    });
    tone(ctx, {
      type: "triangle",
      freq: 540,
      freqEnd: 220,
      start: t + 0.015,
      duration: 0.2,
      peak: 0.14,
      dest,
    });
    tone(ctx, {
      type: "sine",
      freq: 329.63,
      freqEnd: 329.63 * 1.01,
      start: t + 0.16,
      duration: 0.28,
      peak: 0.34,
      dest,
    });
    tone(ctx, {
      type: "sine",
      freq: 392.0,
      freqEnd: 392.0 * 1.01,
      start: t + 0.22,
      duration: 0.34,
      peak: 0.3,
      dest,
    });
    tone(ctx, {
      type: "triangle",
      freq: 196.0,
      start: t + 0.18,
      duration: 0.36,
      peak: 0.12,
      dest,
    });
  });
}

/**
 * Light air trail + flat friendly confirm - money sent.
 * No mid-range pitch drop (that reads as “took damage”).
 */
export function playSendSound(): void {
  withAudio((ctx, t, dest) => {
    // Soft high air only - leaving, not a hit
    tone(ctx, {
      type: "sine",
      freq: 1500,
      freqEnd: 2100,
      start: t,
      duration: 0.14,
      peak: 0.14,
      dest,
    });
    tone(ctx, {
      type: "sine",
      freq: 1900,
      freqEnd: 2400,
      start: t + 0.03,
      duration: 0.12,
      peak: 0.08,
      dest,
    });
    // Warm confirm - flat pitch, gentle attack (statement, not pain)
    tone(ctx, {
      type: "sine",
      freq: 1046.5, // C6
      start: t + 0.1,
      duration: 0.32,
      peak: 0.46,
      dest,
    });
    tone(ctx, {
      type: "sine",
      freq: 1568, // G6 - soft fifth
      start: t + 0.14,
      duration: 0.28,
      peak: 0.22,
      dest,
    });
    tone(ctx, {
      type: "triangle",
      freq: 523.25, // C5 bed
      start: t + 0.1,
      duration: 0.3,
      peak: 0.12,
      dest,
    });
  });
}

/**
 * Secure lock ping - Trezor plugged in / unlocked.
 */
export function playTrezorConnectedSound(): void {
  withAudio((ctx, t, dest) => {
    tone(ctx, {
      type: "sine",
      freq: 980,
      freqEnd: 1175,
      start: t,
      duration: 0.1,
      peak: 0.3,
      dest,
    });
    tone(ctx, {
      type: "sine",
      freq: 1760,
      freqEnd: 1865,
      start: t + 0.08,
      duration: 0.16,
      peak: 0.38,
      dest,
    });
    tone(ctx, {
      type: "sine",
      freq: 2349,
      freqEnd: 2200,
      start: t + 0.12,
      duration: 0.12,
      peak: 0.14,
      dest,
    });
    tone(ctx, {
      type: "triangle",
      freq: 587,
      start: t + 0.06,
      duration: 0.28,
      peak: 0.12,
      dest,
    });
  });
}

/**
 * Two voices meet, then resolve up - trade settled.
 */
export function playSwapSound(): void {
  withAudio((ctx, t, dest) => {
    tone(ctx, {
      type: "sine",
      freq: 740,
      freqEnd: 740 * 1.02,
      start: t,
      duration: 0.16,
      peak: 0.34,
      dest,
    });
    tone(ctx, {
      type: "triangle",
      freq: 370,
      start: t,
      duration: 0.14,
      peak: 0.14,
      dest,
    });
    tone(ctx, {
      type: "sine",
      freq: 880,
      freqEnd: 880 * 1.02,
      start: t + 0.07,
      duration: 0.16,
      peak: 0.34,
      dest,
    });
    tone(ctx, {
      type: "triangle",
      freq: 440,
      start: t + 0.07,
      duration: 0.14,
      peak: 0.14,
      dest,
    });
    tone(ctx, {
      type: "sine",
      freq: 1480,
      start: t + 0.12,
      duration: 0.08,
      peak: 0.12,
      dest,
    });
    tone(ctx, {
      type: "sine",
      freq: 1760,
      start: t + 0.13,
      duration: 0.08,
      peak: 0.12,
      dest,
    });
    tone(ctx, {
      type: "sine",
      freq: 987.8,
      freqEnd: 987.8 * 1.02,
      start: t + 0.2,
      duration: 0.32,
      peak: 0.44,
      dest,
    });
    tone(ctx, {
      type: "sine",
      freq: 1975.5,
      freqEnd: 1865,
      start: t + 0.22,
      duration: 0.18,
      peak: 0.12,
      dest,
    });
    tone(ctx, {
      type: "triangle",
      freq: 494,
      start: t + 0.2,
      duration: 0.34,
      peak: 0.14,
      dest,
    });
  });
}
