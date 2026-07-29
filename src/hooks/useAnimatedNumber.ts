import { useEffect, useRef, useState } from "react";

function prefersReducedMotion(): boolean {
  if (typeof window === "undefined") return false;
  return window.matchMedia("(prefers-reduced-motion: reduce)").matches;
}

/**
 * Unity-style SmoothDamp: critically damped, no overshoot, keeps velocity
 * across retargets so live balance updates glide instead of restarting.
 */
function smoothDamp(
  current: number,
  target: number,
  velocityRef: { current: number },
  smoothTime: number,
  maxSpeed: number,
  deltaTime: number,
): number {
  const st = Math.max(1e-4, smoothTime);
  const omega = 2 / st;
  const x = omega * deltaTime;
  const exp = 1 / (1 + x + 0.48 * x * x + 0.235 * x * x * x);

  let change = current - target;
  const maxChange = maxSpeed * st;
  change = Math.min(Math.max(change, -maxChange), maxChange);

  const originalTo = target;
  const adjustedTarget = current - change;
  const temp = (velocityRef.current + omega * change) * deltaTime;
  velocityRef.current = (velocityRef.current - omega * temp) * exp;
  let output = adjustedTarget + (change + temp) * exp;

  // Clamp so we never cross the real target (important for money).
  if (originalTo - current > 0 === output > originalTo) {
    output = originalTo;
    velocityRef.current = 0;
  }

  return output;
}

/** Map a one-shot "duration" into SmoothDamp's smoothTime (seconds). */
function smoothTimeForDelta(durationMs: number, from: number, to: number): number {
  const delta = Math.abs(to - from);
  const scale = Math.max(Math.abs(from), Math.abs(to), 1);
  const rel = delta / scale;
  // Larger jumps get a touch more air; small cent ticks stay crisp.
  const logBoost = Math.min(Math.log10(1 + delta) / 5, 0.5);
  const relBoost = Math.min(rel, 1) * 0.28;
  const settleMs = Math.min(
    1500,
    Math.max(420, durationMs * (0.9 + logBoost + relBoost)),
  );
  // SmoothDamp mostly settles in ~3× smoothTime.
  return settleMs / 1000 / 3.05;
}

function maxSpeedForDelta(from: number, to: number, smoothTime: number): number {
  const delta = Math.abs(to - from);
  // Cap peak speed so the digits have time to roll instead of blinking.
  // ~1.4× smoothTime at peak ≈ a readable glide before the soft settle.
  return Math.max(delta / Math.max(smoothTime * 1.4, 0.14), 6);
}

/**
 * Tween `target` toward the latest value with a velocity-preserving damp.
 * Mid-flight retargets continue from the current displayed value and speed.
 * `snapKey` changes (e.g. fiat currency) jump immediately.
 */
export function useAnimatedNumber(
  target: number,
  options?: {
    durationMs?: number;
    enabled?: boolean;
    snapKey?: string | number | boolean | null;
  },
): number {
  const enabled = options?.enabled ?? true;
  const durationMs = options?.durationMs ?? 720;
  const snapKey = options?.snapKey;
  const safeTarget = Number.isFinite(target) ? target : 0;

  const [display, setDisplay] = useState(safeTarget);
  const displayRef = useRef(safeTarget);
  const targetRef = useRef(safeTarget);
  const velocityRef = useRef(0);
  const rafRef = useRef<number | null>(null);
  const lastTsRef = useRef<number | null>(null);
  const snapRef = useRef(snapKey);
  const bootedRef = useRef(false);
  const smoothTimeRef = useRef(0.24);
  const maxSpeedRef = useRef(Number.POSITIVE_INFINITY);
  const durationRef = useRef(durationMs);

  durationRef.current = durationMs;

  const cancelLoop = () => {
    if (rafRef.current != null) {
      cancelAnimationFrame(rafRef.current);
      rafRef.current = null;
    }
    lastTsRef.current = null;
  };

  // Unmount-only teardown so mid-flight retargets keep velocity.
  useEffect(() => cancelLoop, []);

  useEffect(() => {
    const to = Number.isFinite(target) ? target : 0;
    const snapChanged = snapRef.current !== snapKey;
    snapRef.current = snapKey;
    targetRef.current = to;

    const snap = (n: number) => {
      cancelLoop();
      velocityRef.current = 0;
      displayRef.current = n;
      setDisplay(n);
    };

    // First paint: land on the real value (don't count up from 0 on mount).
    if (!bootedRef.current) {
      bootedRef.current = true;
      snap(to);
      return;
    }

    if (!enabled || prefersReducedMotion() || snapChanged) {
      snap(to);
      return;
    }

    const from = displayRef.current;
    if (Math.abs(to - from) < 0.005 && Math.abs(velocityRef.current) < 0.01) {
      snap(to);
      return;
    }

    smoothTimeRef.current = smoothTimeForDelta(durationRef.current, from, to);
    maxSpeedRef.current = maxSpeedForDelta(from, to, smoothTimeRef.current);

    if (rafRef.current != null) return;

    const tick = (now: number) => {
      const prev = lastTsRef.current ?? now;
      lastTsRef.current = now;
      // Clamp dt so a backgrounded tab doesn't teleport the balance.
      const dt = Math.min(0.064, Math.max(0.001, (now - prev) / 1000));

      const current = displayRef.current;
      const goal = targetRef.current;
      const next = smoothDamp(
        current,
        goal,
        velocityRef,
        smoothTimeRef.current,
        maxSpeedRef.current,
        dt,
      );

      const err = Math.abs(goal - next);
      const settled = err < 0.004 && Math.abs(velocityRef.current) < 0.08;

      if (settled) {
        displayRef.current = goal;
        velocityRef.current = 0;
        setDisplay(goal);
        rafRef.current = null;
        lastTsRef.current = null;
        return;
      }

      displayRef.current = next;
      // Publish when the visible cent would change — keeps the count readable
      // without flooding React on sub-cent spring noise.
      if (Math.round(next * 100) !== Math.round(current * 100)) {
        setDisplay(next);
      }

      rafRef.current = requestAnimationFrame(tick);
    };

    rafRef.current = requestAnimationFrame(tick);
  }, [target, enabled, durationMs, snapKey]);

  return display;
}
