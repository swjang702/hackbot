/**
 * Camera — pan (drag) and zoom (scroll) for the Pixi.js stage.
 *
 * Applies transforms to a world container, not the stage itself.
 * Zoom is centered on cursor position. Clamp between 0.1x and 5x.
 */

import type { Container } from "pixi.js";

const MIN_ZOOM = 0.1;
const MAX_ZOOM = 5.0;
const ZOOM_FACTOR = 0.1;

export class Camera {
  private worldContainer: Container;
  private canvas: HTMLCanvasElement;
  private dragging = false;
  private lastX = 0;
  private lastY = 0;
  private _zoom = 1.0;

  constructor(worldContainer: Container, canvas: HTMLCanvasElement) {
    this.worldContainer = worldContainer;
    this.canvas = canvas;
    this._bindEvents();
  }

  get zoom(): number {
    return this._zoom;
  }

  /** Center the view on a specific world coordinate. */
  centerOn(worldX: number, worldY: number): void {
    const canvasRect = this.canvas.getBoundingClientRect();
    this.worldContainer.x = canvasRect.width / 2 - worldX * this._zoom;
    this.worldContainer.y = canvasRect.height / 2 - worldY * this._zoom;
  }

  /** Reset zoom and position. */
  reset(): void {
    this._zoom = 1.0;
    this.worldContainer.scale.set(1.0);
    this.worldContainer.x = 0;
    this.worldContainer.y = 0;
  }

  private _bindEvents(): void {
    // Pan: mouse drag
    this.canvas.addEventListener("pointerdown", (e: PointerEvent) => {
      if (e.button === 0) {
        // left click
        this.dragging = true;
        this.lastX = e.clientX;
        this.lastY = e.clientY;
        this.canvas.style.cursor = "grabbing";
      }
    });

    window.addEventListener("pointermove", (e: PointerEvent) => {
      if (!this.dragging) return;
      const dx = e.clientX - this.lastX;
      const dy = e.clientY - this.lastY;
      this.lastX = e.clientX;
      this.lastY = e.clientY;
      this.worldContainer.x += dx;
      this.worldContainer.y += dy;
    });

    window.addEventListener("pointerup", () => {
      if (this.dragging) {
        this.dragging = false;
        this.canvas.style.cursor = "grab";
      }
    });

    // Zoom: scroll wheel centered on cursor
    this.canvas.addEventListener(
      "wheel",
      (e: WheelEvent) => {
        e.preventDefault();

        const direction = e.deltaY < 0 ? 1 : -1;
        const newZoom = Math.max(
          MIN_ZOOM,
          Math.min(MAX_ZOOM, this._zoom * (1 + direction * ZOOM_FACTOR)),
        );

        if (newZoom === this._zoom) return;

        // Get cursor position relative to canvas
        const rect = this.canvas.getBoundingClientRect();
        const cursorX = e.clientX - rect.left;
        const cursorY = e.clientY - rect.top;

        // World position under cursor before zoom
        const worldX = (cursorX - this.worldContainer.x) / this._zoom;
        const worldY = (cursorY - this.worldContainer.y) / this._zoom;

        // Apply new zoom
        this._zoom = newZoom;
        this.worldContainer.scale.set(this._zoom);

        // Adjust position so the point under cursor stays put
        this.worldContainer.x = cursorX - worldX * this._zoom;
        this.worldContainer.y = cursorY - worldY * this._zoom;
      },
      { passive: false },
    );

    // Set initial cursor style
    this.canvas.style.cursor = "grab";
  }
}
