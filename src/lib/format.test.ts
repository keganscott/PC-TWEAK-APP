import { describe, expect, it } from "vitest";

import { formatBytes, formatDuration } from "./format";

describe("format", () => {
  it("says sizes in GB from a gigabyte up, then MB, then KB", () => {
    expect(formatBytes(2.5 * 1024 ** 3)).toBe("2.5 GB");
    expect(formatBytes(300 * 1024 ** 2)).toBe("300 MB");
    expect(formatBytes(1)).toBe("1 KB");
  });

  it("says durations in seconds, minutes, then hours and minutes", () => {
    expect(formatDuration(1)).toBe("1 second");
    expect(formatDuration(41)).toBe("41 seconds");
    expect(formatDuration(90)).toBe("2 minutes");
    expect(formatDuration(3599)).toBe("1 hour");
    expect(formatDuration(3900)).toBe("1 hour 5 minutes");
    expect(formatDuration(2 * 3600 + 60)).toBe("2 hours 1 minute");
  });
});
