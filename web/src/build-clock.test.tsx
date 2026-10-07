import { act, renderHook } from "@testing-library/react";
import { expect, test, vi } from "vitest";
import { useBuildClock } from "./build-evidence";

test("live clock updates before one second, bounds React work and stops when completed", () => {
  const frames = new Map<number, FrameRequestCallback>();
  let id = 0;
  vi.stubGlobal("requestAnimationFrame", (callback: FrameRequestCallback) => {
    frames.set(++id, callback);
    return id;
  });
  vi.stubGlobal("cancelAnimationFrame", (id: number) => frames.delete(id));
  const clock = vi.spyOn(Date, "now").mockReturnValue(1000);
  const f = renderHook(({ active }) => useBuildClock(active), {
    initialProps: { active: true },
  });
  const frame = (time: number, now: number) =>
    act(() => {
      clock.mockReturnValue(now);
      const next = frames.entries().next().value;
      if (!next) throw new Error("Missing frame");
      frames.delete(next[0]);
      next[1](time);
    });
  try {
    frame(0, 1100);
    expect(f.result.current).toBe(1100000n);
    frame(50, 1200);
    expect(f.result.current).toBe(1100000n);
    frame(100, 1300);
    expect(f.result.current).toBe(1300000n);
    f.rerender({ active: false });
    expect(frames.size).toBe(0);
    expect(f.result.current).toBe(1300000n);
  } finally {
    f.unmount();
    clock.mockRestore();
    vi.unstubAllGlobals();
  }
});
test("reduced motion uses discrete updates and changing preference cleans up its clock", async () => {
  vi.useFakeTimers();
  vi.setSystemTime(1000);
  let change: (() => void) | undefined;
  const motion = {
    matches: true,
    addEventListener: vi.fn((_name: string, callback: () => void) => {
      change = callback;
    }),
    removeEventListener: vi.fn(),
  };
  const frame = vi.fn(() => 1),
    cancel = vi.fn();
  vi.stubGlobal("matchMedia", () => motion);
  vi.stubGlobal("requestAnimationFrame", frame);
  vi.stubGlobal("cancelAnimationFrame", cancel);
  const f = renderHook(() => useBuildClock(true));
  try {
    expect(frame).not.toHaveBeenCalled();
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1000);
    });
    expect(f.result.current).toBe(2000000n);
    act(() => {
      motion.matches = false;
      change?.();
    });
    expect(frame).toHaveBeenCalledOnce();
    expect(vi.getTimerCount()).toBe(0);
    f.unmount();
    expect(cancel).toHaveBeenLastCalledWith(1);
    expect(motion.removeEventListener).toHaveBeenCalledOnce();
  } finally {
    f.unmount();
    vi.unstubAllGlobals();
    vi.useRealTimers();
  }
});
