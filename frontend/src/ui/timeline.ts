/**
 * Timeline — playback controls (play/pause, speed, scrub).
 *
 * position_ns from server is elapsed nanoseconds from trace start (relative).
 * duration_ns is total trace duration. Both are relative, making slider math simple.
 */

import type { Connection } from "../connection";
import type { PlaybackMessage } from "../types";
import type { PanelRefs } from "./layout";

export class Timeline {
  private conn: Connection;
  private refs: PanelRefs;
  private playing = false;
  private durationNs = BigInt(0);
  private positionNs = BigInt(0);
  private currentSpeed = 1.0;
  private scrubbing = false;

  constructor(conn: Connection, refs: PanelRefs) {
    this.conn = conn;
    this.refs = refs;
    this._bindEvents();
  }

  handlePlayback(msg: PlaybackMessage): void {
    this.playing = msg.status === "playing";
    this.currentSpeed = msg.speed;
    this.durationNs = BigInt(msg.duration_ns);
    this.positionNs = BigInt(msg.position_ns);

    // Update play button
    this.refs.playBtn.textContent = this.playing ? "Pause" : "Play";

    // Update speed buttons
    this.refs.speedBtns.forEach((btn) => {
      const speed = parseFloat(btn.dataset.speed ?? "1");
      btn.classList.toggle("active", speed === this.currentSpeed);
    });

    // Update scrub slider (unless user is dragging)
    if (!this.scrubbing && this.durationNs > 0n) {
      const ratio = Number((this.positionNs * 1000n) / this.durationNs);
      this.refs.scrubSlider.value = String(Math.min(1000, ratio));
    }

    // Update position text
    this._updatePositionText();
  }

  private _updatePositionText(): void {
    const posSec = Number(this.positionNs) / 1e9;
    const durSec = Number(this.durationNs) / 1e9;
    this.refs.timelinePosition.textContent =
      `${posSec.toFixed(3)}s / ${durSec.toFixed(3)}s`;
  }

  private _bindEvents(): void {
    // Play/Pause
    this.refs.playBtn.addEventListener("click", () => {
      if (this.playing) {
        this.conn.send({ cmd: "pause" });
      } else {
        this.conn.send({ cmd: "play" });
      }
    });

    // Speed buttons
    this.refs.speedBtns.forEach((btn) => {
      btn.addEventListener("click", () => {
        const speed = parseFloat(btn.dataset.speed ?? "1");
        this.conn.send({ cmd: "speed", multiplier: speed });
      });
    });

    // Scrub slider
    this.refs.scrubSlider.addEventListener("input", () => {
      this.scrubbing = true;
    });

    this.refs.scrubSlider.addEventListener("change", () => {
      const ratio = parseInt(this.refs.scrubSlider.value) / 1000;
      // Send relative offset (duration * ratio)
      const seekNs = BigInt(Math.floor(Number(this.durationNs) * ratio));
      this.conn.send({ cmd: "seek", position_ns: seekNs.toString() });
      this.scrubbing = false;
    });
  }
}
