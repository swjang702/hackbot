/**
 * Game world — Pixi.js Application managing process rooms and event animations.
 *
 * On receiving world_state: creates/updates process rooms.
 * On receiving events batch: triggers syscall object animations.
 */

import { Application, Container } from "pixi.js";
import type {
  EventsMessage,
  WorldStateMessage,
} from "../types";
import { Camera } from "./camera";
import { EventParticleSystem } from "./event-particle";
import { ProcessRoom } from "./process-room";
import { computeLayout } from "./spatial-mapper";
import { SyscallObjectPool } from "./syscall-object";

// Threshold: if a process receives more than this many events in one batch,
// trigger a particle burst
const BURST_THRESHOLD = 5;

export class GameWorld {
  private app: Application;
  private worldContainer: Container;
  private _camera: Camera | null = null;
  private rooms: Map<number, ProcessRoom> = new Map();
  private syscallPool: SyscallObjectPool;
  private particles: EventParticleSystem;
  private initialized = false;

  constructor() {
    this.app = new Application();
    this.worldContainer = new Container();
    this.syscallPool = new SyscallObjectPool();
    this.particles = new EventParticleSystem();
  }

  get zoom(): number {
    return this._camera?.zoom ?? 1;
  }

  async init(container: HTMLDivElement): Promise<void> {
    const width = container.clientWidth;
    const height = container.clientHeight;

    await this.app.init({
      width,
      height,
      backgroundColor: 0x0d1117,
      antialias: true,
      resolution: window.devicePixelRatio || 1,
      autoDensity: true,
    });

    container.appendChild(this.app.canvas);
    this.app.stage.addChild(this.worldContainer);

    // Syscall objects and particles render on top of rooms
    this.worldContainer.addChild(this.syscallPool.container);
    this.worldContainer.addChild(this.particles.container);

    // Camera (pan/zoom)
    this._camera = new Camera(
      this.worldContainer,
      this.app.canvas as HTMLCanvasElement,
    );

    // Animation ticker
    this.app.ticker.add((ticker) => {
      const dt = ticker.deltaTime / 60; // convert to seconds
      this.syscallPool.tick(dt);
      this.particles.tick(dt);
      for (const room of this.rooms.values()) {
        room.tick(dt);
      }
    });

    // Handle resize
    const observer = new ResizeObserver(() => {
      const w = container.clientWidth;
      const h = container.clientHeight;
      this.app.renderer.resize(w, h);
    });
    observer.observe(container);

    this.initialized = true;
  }

  handleWorldState(msg: WorldStateMessage): void {
    if (!this.initialized) return;

    const processes = msg.processes;
    const layout = computeLayout(processes);

    // Create/update rooms
    const activePids = new Set<number>();
    for (const proc of processes) {
      if (proc.pid === 0) continue; // skip kernel pseudo-process
      activePids.add(proc.pid);

      const pos = layout.get(proc.pid);
      if (!pos) continue;

      let room = this.rooms.get(proc.pid);
      if (!room) {
        room = new ProcessRoom(proc, pos.x, pos.y, pos.width, pos.height);
        this.rooms.set(proc.pid, room);
        // Insert rooms below syscall/particle layers
        this.worldContainer.addChildAt(room.container, 0);
      } else {
        room.updateInfo(proc);
        room.container.x = pos.x;
        room.container.y = pos.y;
      }
    }

    // Remove rooms for processes no longer in world state
    for (const [pid, room] of this.rooms) {
      if (!activePids.has(pid)) {
        this.worldContainer.removeChild(room.container);
        this.rooms.delete(pid);
      }
    }
  }

  handleEvents(msg: EventsMessage): void {
    if (!this.initialized) return;

    // Count events per PID for burst detection
    const pidCounts = new Map<number, number>();

    for (const event of msg.batch) {
      const room = this.rooms.get(event.pid);
      if (!room) continue;

      room.addEvent();
      pidCounts.set(event.pid, (pidCounts.get(event.pid) ?? 0) + 1);

      // Spawn syscall object in the room's event area
      const area = room.getEventArea();
      this.syscallPool.spawn(event, area);
    }

    // Trigger particle burst for high-activity rooms
    for (const [pid, count] of pidCounts) {
      if (count >= BURST_THRESHOLD) {
        const room = this.rooms.get(pid);
        if (room) {
          const area = room.getEventArea();
          const cx = area.x + area.w / 2;
          const cy = area.y + area.h / 2;
          this.particles.burst(cx, cy, 0x58a6ff);
        }
      }
    }
  }
}
