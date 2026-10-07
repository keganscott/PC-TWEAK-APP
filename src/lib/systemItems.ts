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
    case "tcp_global":
      return `TCP setting ${item.name}`;
    case "nvidia_setting":
      return `NVIDIA setting 0x${item.setting.toString(16).toUpperCase().padStart(8, "0")} (${item.profile || "global"} profile)`;
    case "file":
      return `file ${item.path}`;
  }
}

export function describeState(state: SysState): string {
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
