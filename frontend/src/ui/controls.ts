/**
 * Filter controls — PID and event type filters.
 */

import type { Connection } from "../connection";
import type { EventType, WorldStateMessage } from "../types";
import type { PanelRefs } from "./layout";

const ALL_EVENT_TYPES: { value: EventType; label: string }[] = [
  { value: "syscall_enter", label: "syscall" },
  { value: "gpu_submit", label: "gpu" },
  { value: "sched_switch", label: "sched" },
  { value: "power_trace", label: "power" },
  { value: "process_fork", label: "proc" },
];

export class Controls {
  private conn: Connection;
  private refs: PanelRefs;
  private activePids: Set<number> = new Set();
  private activeTypes: Set<string> = new Set();
  private allPids: number[] = [];

  constructor(conn: Connection, refs: PanelRefs) {
    this.conn = conn;
    this.refs = refs;
    this._initTypeFilters();
  }

  handleWorldState(msg: WorldStateMessage): void {
    const pids = msg.processes
      .map((p) => p.pid)
      .filter((pid) => pid !== 0)
      .sort((a, b) => a - b);

    // Only rebuild if PIDs changed
    if (
      pids.length === this.allPids.length &&
      pids.every((p, i) => p === this.allPids[i])
    ) {
      return;
    }

    this.allPids = pids;
    this._rebuildPidFilters();
  }

  private _rebuildPidFilters(): void {
    this.refs.pidFilters.innerHTML = "";

    for (const pid of this.allPids) {
      const chip = document.createElement("span");
      chip.className = "chip active";
      chip.textContent = String(pid);
      chip.dataset.pid = String(pid);
      this.activePids.add(pid);

      chip.addEventListener("click", () => {
        if (this.activePids.has(pid)) {
          this.activePids.delete(pid);
          chip.classList.remove("active");
        } else {
          this.activePids.add(pid);
          chip.classList.add("active");
        }
        this._sendFilter();
      });

      this.refs.pidFilters.appendChild(chip);
    }
  }

  private _initTypeFilters(): void {
    for (const { value, label } of ALL_EVENT_TYPES) {
      const chip = document.createElement("span");
      chip.className = "chip active";
      chip.textContent = label;
      chip.dataset.type = value;
      this.activeTypes.add(value);

      chip.addEventListener("click", () => {
        // For syscall, toggle both enter and exit
        const types =
          value === "syscall_enter"
            ? ["syscall_enter", "syscall_exit"]
            : value === "gpu_submit"
              ? ["gpu_submit", "gpu_complete"]
              : value === "process_fork"
                ? ["process_fork", "process_exit"]
                : [value];

        if (this.activeTypes.has(value)) {
          for (const t of types) this.activeTypes.delete(t);
          chip.classList.remove("active");
        } else {
          for (const t of types) this.activeTypes.add(t);
          chip.classList.add("active");
        }
        this._sendFilter();
      });

      this.refs.typeFilters.appendChild(chip);
    }
  }

  private _sendFilter(): void {
    // If all are active, send null (no filter)
    const pids =
      this.activePids.size === this.allPids.length
        ? undefined
        : [...this.activePids];
    const types =
      this.activeTypes.size >= ALL_EVENT_TYPES.length
        ? undefined
        : [...this.activeTypes];

    this.conn.send({ cmd: "filter", pids, types });
  }
}
