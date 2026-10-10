import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { CHECK_MS } from "../../lib/mouseRate";
import { MouseRateCheck } from "./MouseRateCheck";

describe("MouseRateCheck", () => {
  let now = 0;
  beforeEach(() => {
    vi.useFakeTimers();
    now = 0;
    vi.spyOn(performance, "now").mockImplementation(() => now);
  });
  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
    vi.useRealTimers();
  });

  /** Mouse reports `gapMs` apart for `ms`, as raw pointer updates to the window. */
  const move = (gapMs: number, ms: number, pointerType = "mouse") =>
    act(() => {
      const end = now + ms;
      for (; now < end; now += gapMs) window.dispatchEvent(new PointerEvent("pointerrawupdate", { pointerType }));
    });

  it("counts from the first report for the whole check and names the matching setting", () => {
    render(<MouseRateCheck />);
    fireEvent.click(screen.getByRole("button", { name: "Count my mouse's reports" }));
    expect(screen.getByText("Move the mouse in quick circles now")).toBeTruthy();

    move(1, 1000);
    act(() => vi.advanceTimersByTime(400));
    expect(screen.getByText(/^Keep moving: \d s left$/)).toBeTruthy();
    move(1, CHECK_MS - 1000);
    act(() => vi.advanceTimersByTime(CHECK_MS));

    expect(screen.getByText("1,000")).toBeTruthy();
    expect(screen.getByText(/^That matches a 1,000 Hz setting\./)).toBeTruthy();
    expect(screen.getByText("4,000 reports counted.")).toBeTruthy();
    // Moves after the check are not counted.
    move(1, 500);
    expect(screen.getByText("4,000 reports counted.")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Count again" })).toBeTruthy();
  });

  it("says when too few reports arrived, and gives up when the mouse never moves", () => {
    render(<MouseRateCheck />);
    fireEvent.click(screen.getByRole("button", { name: "Count my mouse's reports" }));
    act(() => vi.advanceTimersByTime(10_000));
    expect(screen.getByText(/^Too few reports to tell\./)).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: "Count again" }));
    move(100, CHECK_MS);
    act(() => vi.advanceTimersByTime(CHECK_MS));
    expect(screen.getByText(/^Too few reports to tell\./)).toBeTruthy();
  });

  it("counts only the mouse, not a pen or a finger", () => {
    render(<MouseRateCheck />);
    fireEvent.click(screen.getByRole("button", { name: "Count my mouse's reports" }));
    move(1, CHECK_MS, "touch");
    move(1, CHECK_MS, "pen");
    act(() => vi.advanceTimersByTime(10_000));
    expect(screen.getByText(/^Too few reports to tell\./)).toBeTruthy();
  });
});
