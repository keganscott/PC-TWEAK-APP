// Plain wording for non-registry changes in the Backups history (technical
// details view). Mirrors `SysItem::describe` / `SideEffect::describe` in
// crates/engine/src/system.rs.

import type { SideEffect } from "../generated/SideEffect";
import type { SysItem } from "../generated/SysItem";
import type { SysState } from "../generated/SysState";

export function describeItem(item: SysItem): string {
  switch (item.kind) {
    case "active_power_scheme":
      return "active power plan";
    case "power_scheme":
      return `power plan ${item.guid}`;
    case "power_setting":
      return `power plan ${item.scheme} setting ${item.subgroup}/${item.setting} (${item.ac ? "plugged in" : "on battery"})`;
    case "hibernation":
      return "hibernation";
    case "service":
      return `service ${item.name}`;
    case "scheduled_task":
      return `scheduled task ${item.path}`;
    case "dns_servers":
      return `DNS servers of adapter ${item.interface}`;
    case "interface_metric":
      return `${item.ipv6 ? "IPv6" : "IPv4"} interface metric of adapter ${item.interface}`;
    case "tcp_global":
      return `TCP setting ${item.name}`;
    case "nvidia_setting":
      return `NVIDIA setting 0x${item.setting.toString(16).toUpperCase().padStart(8, "0")} (${item.profile || "global"} profile)`;
    case "amd_setting":
      return `AMD ${AMD_LABELS[item.setting] ?? item.setting} of graphics card ${item.gpu}`;
    case "qos_policy":
      return `QoS policy ${item.name}`;
    case "refresh_rate":
      return `refresh rate of display ${item.display}`;
    case "file":
      return `file ${item.path}`;
  }
}

/** `adlx::Setting::label`, by key. */
const AMD_LABELS: Record<string, string> = {
  anti_lag: "Radeon Anti-Lag",
  anti_lag_level: "Radeon Anti-Lag level",
  chill: "Radeon Chill",
  wait_for_vertical_refresh: "Wait for Vertical Refresh",
};

/** AMD Software's words for each value (`adlx::vsync`, `adlx::anti_lag_level`;
 * Anti-Lag and Chill 0 off, 1 on). */
const AMD_VALUES: Record<string, string[]> = {
  anti_lag: ["off", "on"],
  anti_lag_level: ["Anti-Lag", "Anti-Lag Next"],
  chill: ["off", "on"],
  wait_for_vertical_refresh: ["always off", "off unless the game asks", "on unless the game asks", "always on"],
};

/** `item`, when given, says what a value means: 0 is Windows' automatic
 * interface metric, an NVIDIA setting with no value of its own has the
 * driver's default, and AMD values read as AMD Software shows them. */
export function describeState(state: SysState, item?: SysItem): string {
  if (item?.kind === "interface_metric" && state.state === "dword" && state.value === 0) return "automatic";
  if (item?.kind === "nvidia_setting" && state.state === "absent") return "driver default";
  if (item?.kind === "amd_setting") {
    if (state.state === "absent") return "not on this card";
    const word = state.state === "dword" ? AMD_VALUES[item.setting]?.[state.value] : undefined;
    if (word) return word;
  }
  switch (state.state) {
    case "absent":
      return "not present";
    case "bool":
      return state.on ? "on" : "off";
    case "text":
      return state.text;
    case "dword":
      return String(state.value);
    case "list":
      return state.items.length ? state.items.join(", ") : "automatic";
    case "service":
      return `${state.start.replace("_", " ")}, ${state.running ? "running" : "stopped"}`;
    case "scheme":
      return `copy of ${state.source}`;
    case "qos_policy":
      return `${state.program}, DSCP ${state.dscp}`;
    case "file":
      return `saved copy ${state.backup}`;
  }
}

export function describeEffect(effect: SideEffect): string {
  switch (effect.effect) {
    case "restart_adapter":
      return `restart network adapter ${effect.interface}`;
    case "refresh_policy":
      return "refresh Windows policy";
    case "restart_service":
      return `restart service ${effect.name}`;
  }
}
