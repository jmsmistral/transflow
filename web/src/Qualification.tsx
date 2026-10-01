// Opt-in local measurement only; Vite removes this panel from normal production builds.
import { useEffect, useRef, useState } from "react";
const percentile = (values: readonly number[]) => {
  const sorted = [...values].sort((a, b) => a - b);
  return (
    sorted[Math.max(0, Math.ceil(sorted.length * 0.95) - 1)]?.toFixed(2) ??
    "Unavailable"
  );
};
export function Qualification() {
  const samples = useRef<number[]>([]);
  const interactions = useRef<number[]>([]);
  const [active, setActive] = useState(false);
  const [result, setResult] = useState("");
  useEffect(() => {
    if (!active) return;
    let frame = 0,
      previous: number | undefined;
    let stopped = false;
    const tick = (now: number) => {
      if (previous !== undefined && samples.current.length < 2000)
        samples.current.push(now - previous);
      previous = now;
      frame = requestAnimationFrame(tick);
    };
    const interaction = (event: Event) => {
      if ((event.target as Element).closest('[aria-label="UI qualification"]'))
        return;
      const at = performance.now();
      requestAnimationFrame(() =>
        requestAnimationFrame(() => {
          if (!stopped && interactions.current.length < 2000)
            interactions.current.push(performance.now() - at);
        }),
      );
    };
    frame = requestAnimationFrame(tick);
    document.addEventListener("click", interaction, true);
    document.addEventListener("keydown", interaction, true);
    return () => {
      stopped = true;
      cancelAnimationFrame(frame);
      document.removeEventListener("click", interaction, true);
      document.removeEventListener("keydown", interaction, true);
    };
  }, [active]);
  return (
    <aside
      aria-label="UI qualification"
      style={{
        position: "fixed",
        bottom: 30,
        left: 12,
        zIndex: 100,
        background: "var(--surface)",
        border: "1px solid var(--line)",
        padding: 8,
        fontSize: 12,
      }}
    >
      <button
        onClick={() => {
          if (!active) {
            samples.current = [];
            interactions.current = [];
            setResult("");
          } else
            setResult(
              `Frames ${samples.current.length}; p95 ${percentile(samples.current)} ms. Input to second animation frame ${interactions.current.length}; p95 ${percentile(interactions.current)} ms.`,
            );
          setActive(!active);
        }}
      >
        {active ? "Stop measurement" : "Start measurement"}
      </button>
      <output>{result}</output>
    </aside>
  );
}
