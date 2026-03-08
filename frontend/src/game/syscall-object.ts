/**
 * Syscall object pool — animated event indicators inside process rooms.
 *
 * Object pooling: pre-create ~200 Graphics objects, reuse them.
 * Each object: small colored circle, scales up, holds, fades out.
 */

import { Container, Graphics } from "pixi.js";
import type { TraceEvent } from "../types";

const POOL_SIZE = 200;
const ANIMATION_DURATION = 0.5; // seconds
const OBJECT_RADIUS = 5;

// Colors by syscall/event type
const EVENT_COLORS: Record<string, number> = {
  read: 0x58a6ff, // blue
  write: 0x3fb950, // green
  open: 0xd29922, // yellow
  mmap: 0xbc8cff, // purple
  close: 0x8b949e, // gray
  perf_event_open: 0xf85149, // red
  gpu_submit: 0xbc8cff, // purple
  gpu_complete: 0x7ee787, // light green
  sched_switch: 0xd29922, // yellow
  power_trace: 0xd18616, // orange
  process_fork: 0xf85149, // red
  process_exit: 0x484f58, // dim gray
};

interface PooledObject {
  graphics: Graphics;
  inUse: boolean;
  timer: number;
  duration: number;
}

export class SyscallObjectPool {
  private pool: PooledObject[] = [];
  readonly container: Container;

  constructor() {
    this.container = new Container();

    for (let i = 0; i < POOL_SIZE; i++) {
      const g = new Graphics();
      g.circle(0, 0, OBJECT_RADIUS).fill({ color: 0xffffff });
      g.visible = false;
      this.container.addChild(g);
      this.pool.push({ graphics: g, inUse: false, timer: 0, duration: 0 });
    }
  }

  /**
   * Spawn a syscall object at a random position within the given area.
   */
  spawn(
    event: TraceEvent,
    area: { x: number; y: number; w: number; h: number },
  ): void {
    // Find a free object
    const obj = this.pool.find((o) => !o.inUse);
    if (!obj) return; // pool exhausted, skip this event

    const name =
      event.type === "syscall_enter" || event.type === "syscall_exit"
        ? ((event.payload.name as string) ?? "unknown")
        : event.type;

    const color = EVENT_COLORS[name] ?? 0x8b949e;

    obj.graphics.tint = color;
    obj.graphics.x = area.x + Math.random() * area.w;
    obj.graphics.y = area.y + Math.random() * area.h;
    obj.graphics.scale.set(0.1);
    obj.graphics.alpha = 1;
    obj.graphics.visible = true;

    obj.inUse = true;
    obj.timer = 0;
    obj.duration = ANIMATION_DURATION;
  }

  /**
   * Update all active objects. Call once per frame.
   * @param dt Delta time in seconds.
   */
  tick(dt: number): void {
    for (const obj of this.pool) {
      if (!obj.inUse) continue;

      obj.timer += dt;
      const progress = obj.timer / obj.duration;

      if (progress >= 1) {
        // Animation complete — return to pool
        obj.inUse = false;
        obj.graphics.visible = false;
        continue;
      }

      if (progress < 0.2) {
        // Scale up phase (0-20%)
        const t = progress / 0.2;
        obj.graphics.scale.set(0.1 + 0.9 * t);
        obj.graphics.alpha = 1;
      } else {
        // Fade out phase (20-100%)
        const t = (progress - 0.2) / 0.8;
        obj.graphics.scale.set(1);
        obj.graphics.alpha = 1 - t;
      }
    }
  }
}
