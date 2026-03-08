/**
 * Event log — scrollable list of recent events, color-coded by type.
 */

import type { EventsMessage, TraceEvent } from "../types";

const MAX_ENTRIES = 500;
const AUTO_SCROLL_THRESHOLD = 50; // px from bottom

// CSS class by event type category
function eventClass(event: TraceEvent): string {
  switch (event.type) {
    case "syscall_enter":
    case "syscall_exit":
      return "log-syscall";
    case "gpu_submit":
    case "gpu_complete":
      return "log-gpu";
    case "sched_switch":
      return "log-sched";
    case "power_trace":
      return "log-power";
    case "process_fork":
    case "process_exit":
      return "log-process";
    default:
      return "";
  }
}

function formatEvent(event: TraceEvent, startNs: bigint): string {
  const relNs = BigInt(event.ts) - startNs;
  const relSec = (Number(relNs) / 1e9).toFixed(3);
  const payload = event.payload;

  switch (event.type) {
    case "syscall_enter":
      return `[${relSec}s] ${event.pid}:${event.comm} ${payload.name}(${formatSyscallArgs(payload)})`;
    case "syscall_exit":
      return `[${relSec}s] ${event.pid}:${event.comm} ${payload.name} -> ${payload.ret}`;
    case "gpu_submit":
      return `[${relSec}s] ${event.pid}:${event.comm} gpu_submit(batch=${payload.batch_size})`;
    case "gpu_complete":
      return `[${relSec}s] ${event.pid}:${event.comm} gpu_complete(batch=${payload.batch_size})`;
    case "sched_switch":
      return `[${relSec}s] sched ${payload.prev_pid} -> ${payload.next_pid}`;
    case "power_trace":
      return `[${relSec}s] power ${(payload.watts as number).toFixed(1)}W [${payload.domain}]`;
    case "process_fork":
      return `[${relSec}s] fork ${payload.parent_pid} -> ${payload.child_pid} (${payload.child_comm})`;
    case "process_exit":
      return `[${relSec}s] ${event.pid}:${event.comm} exit(${payload.exit_code})`;
    default:
      return `[${relSec}s] ${event.pid}:${event.comm} ${event.type}`;
  }
}

function formatSyscallArgs(payload: Record<string, unknown>): string {
  const parts: string[] = [];
  if (payload.fd !== undefined && payload.fd !== null) parts.push(`fd=${payload.fd}`);
  if (payload.count !== undefined && payload.count !== null) parts.push(`${payload.count}`);
  if (payload.path !== undefined && payload.path !== null) parts.push(`"${payload.path}"`);
  return parts.join(", ");
}

export class EventLog {
  private container: HTMLDivElement;
  private entryCount = 0;
  private startNs = 0n;
  private autoScroll = true;

  constructor(container: HTMLDivElement) {
    this.container = container;

    // Track scroll position for auto-scroll behavior
    this.container.addEventListener("scroll", () => {
      const { scrollTop, scrollHeight, clientHeight } = this.container;
      this.autoScroll =
        scrollHeight - scrollTop - clientHeight < AUTO_SCROLL_THRESHOLD;
    });
  }

  setStartTimestamp(ns: string): void {
    this.startNs = BigInt(ns);
  }

  handleEvents(msg: EventsMessage): void {
    const fragment = document.createDocumentFragment();

    for (const event of msg.batch) {
      const div = document.createElement("div");
      div.className = `log-entry ${eventClass(event)}`;
      div.textContent = formatEvent(event, this.startNs);
      fragment.appendChild(div);
      this.entryCount++;
    }

    this.container.appendChild(fragment);

    // Trim old entries if over limit
    while (this.entryCount > MAX_ENTRIES) {
      const first = this.container.firstChild;
      if (first) {
        this.container.removeChild(first);
        this.entryCount--;
      } else {
        break;
      }
    }

    // Auto-scroll to bottom if user was at bottom
    if (this.autoScroll) {
      this.container.scrollTop = this.container.scrollHeight;
    }
  }

  clear(): void {
    this.container.innerHTML = "";
    this.entryCount = 0;
  }
}
