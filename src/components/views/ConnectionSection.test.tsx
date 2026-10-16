import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import type { WifiSignal } from "../../generated/WifiSignal";
import { WifiLine } from "./ConnectionSection";

const signal: WifiSignal = { adapter: "Test adapter", rssiDbm: -69, quality: 62, carriesCheck: true };

describe("WifiLine", () => {
  afterEach(cleanup);

  it("shows nothing when this PC is not on Wi-Fi", () => {
    const { container } = render(<WifiLine wifi={{ state: "no", reason: "no Wi-Fi adapter is connected" }} />);
    expect(container.textContent).toBe("");
  });

  it("says it could not tell, with the reason only in the technical view", () => {
    const wifi = { state: "unknown", reason: "Windows' location setting keeps apps from reading Wi-Fi details" } as const;
    const { rerender } = render(<WifiLine wifi={wifi} />);
    expect(screen.getByText("Wi-Fi signal: could not tell.")).toBeTruthy();
    rerender(<WifiLine wifi={wifi} technical />);
    expect(screen.getByText(/could not tell\. Windows' location setting/)).toBeTruthy();
  });

  it("says when the echoes did not go over this Wi-Fi", () => {
    const { rerender } = render(<WifiLine wifi={{ state: "yes", value: signal }} />);
    expect(screen.queryByText(/left through another connection/)).toBeNull();
    rerender(<WifiLine wifi={{ state: "yes", value: { ...signal, carriesCheck: false } }} />);
    expect(screen.getByText(/left through another connection/)).toBeTruthy();
  });
});
