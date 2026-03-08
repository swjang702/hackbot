/**
 * Process room — a labeled rectangle representing one process.
 *
 * Activity indicator: border color shifts from dim to bright
 * based on recent event frequency.
 */

import { Container, Graphics, Text } from "pixi.js";
import type { ProcessInfo } from "../types";

// Colors by process status
const STATUS_COLORS: Record<string, number> = {
  running: 0x3fb950,
  sleeping: 0x8b949e,
  exited: 0x484f58,
};

// Activity intensity → border color interpolation
const BORDER_DIM = 0x30363d;
const BORDER_BRIGHT = 0x58a6ff;

export class ProcessRoom {
  readonly container: Container;
  readonly pid: number;

  private bg: Graphics;
  private border: Graphics;
  private label: Text;
  private statsText: Text;

  private _activity = 0; // 0..1 — decays over time, spikes on events
  private _width: number;
  private _height: number;

  constructor(
    info: ProcessInfo,
    x: number,
    y: number,
    width: number,
    height: number,
  ) {
    this.pid = info.pid;
    this._width = width;
    this._height = height;

    this.container = new Container();
    this.container.x = x;
    this.container.y = y;

    // Background fill
    this.bg = new Graphics();
    this._drawBg();
    this.container.addChild(this.bg);

    // Border (drawn separately so we can change its color for activity)
    this.border = new Graphics();
    this._drawBorder(BORDER_DIM);
    this.container.addChild(this.border);

    // Label: PID + comm
    this.label = new Text({
      text: `${info.comm} [${info.pid}]`,
      style: {
        fontFamily: "monospace",
        fontSize: 12,
        fill: 0xe6edf3,
        fontWeight: "bold",
      },
    });
    this.label.x = 8;
    this.label.y = 6;
    this.container.addChild(this.label);

    // Stats line
    this.statsText = new Text({
      text: "",
      style: {
        fontFamily: "monospace",
        fontSize: 10,
        fill: 0x8b949e,
      },
    });
    this.statsText.x = 8;
    this.statsText.y = 22;
    this.container.addChild(this.statsText);

    // Status indicator dot
    this.updateStatus(info.status);
  }

  /** Call on each frame to decay activity. */
  tick(dt: number): void {
    if (this._activity > 0) {
      this._activity = Math.max(0, this._activity - dt * 2);
      this._drawBorder(lerpColor(BORDER_DIM, BORDER_BRIGHT, this._activity));
    }
  }

  /** Spike activity (called when events hit this process). */
  addEvent(): void {
    this._activity = Math.min(1, this._activity + 0.15);
    this._drawBorder(lerpColor(BORDER_DIM, BORDER_BRIGHT, this._activity));
  }

  /** Update process info display. */
  updateInfo(info: ProcessInfo): void {
    this.label.text = `${info.comm} [${info.pid}]`;
    this.statsText.text = `sys:${info.syscall_count} gpu:${info.gpu_submit_count}`;
    this.updateStatus(info.status);
  }

  updateStatus(status: string): void {
    const color = STATUS_COLORS[status] ?? STATUS_COLORS.sleeping;
    // Tint the background slightly based on status
    this.bg.tint = color;
  }

  /** Get the center point of the room (for positioning syscall objects). */
  getEventArea(): { x: number; y: number; w: number; h: number } {
    return {
      x: this.container.x + 8,
      y: this.container.y + 38,
      w: this._width - 16,
      h: this._height - 46,
    };
  }

  private _drawBg(): void {
    this.bg
      .rect(0, 0, this._width, this._height)
      .fill({ color: 0x161b22, alpha: 0.8 });
  }

  private _drawBorder(color: number): void {
    this.border.clear();
    this.border.rect(0, 0, this._width, this._height).stroke({
      color,
      width: 2,
      alpha: 1,
    });
  }
}

/** Linearly interpolate between two RGB hex colors. */
function lerpColor(a: number, b: number, t: number): number {
  const ar = (a >> 16) & 0xff,
    ag = (a >> 8) & 0xff,
    ab = a & 0xff;
  const br = (b >> 16) & 0xff,
    bg = (b >> 8) & 0xff,
    bb = b & 0xff;
  const r = Math.round(ar + (br - ar) * t);
  const g = Math.round(ag + (bg - ag) * t);
  const bv = Math.round(ab + (bb - ab) * t);
  return (r << 16) | (g << 8) | bv;
}
