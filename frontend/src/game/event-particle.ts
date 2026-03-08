/**
 * Event particle — burst effect for high-frequency events.
 *
 * When many events hit the same process room in a short window,
 * triggers a particle burst as a visual indicator of high activity.
 */

import { Container, Graphics } from "pixi.js";

const MAX_PARTICLES = 100;
const BURST_SIZE = 8;
const PARTICLE_RADIUS = 2;
const PARTICLE_LIFETIME = 0.6; // seconds
const PARTICLE_SPEED = 60; // pixels per second

interface Particle {
  graphics: Graphics;
  inUse: boolean;
  timer: number;
  vx: number;
  vy: number;
}

export class EventParticleSystem {
  private particles: Particle[] = [];
  readonly container: Container;

  constructor() {
    this.container = new Container();

    for (let i = 0; i < MAX_PARTICLES; i++) {
      const g = new Graphics();
      g.circle(0, 0, PARTICLE_RADIUS).fill({ color: 0xffffff });
      g.visible = false;
      this.container.addChild(g);
      this.particles.push({ graphics: g, inUse: false, timer: 0, vx: 0, vy: 0 });
    }
  }

  /**
   * Trigger a burst of particles at the given world position.
   */
  burst(x: number, y: number, color: number): void {
    let spawned = 0;
    for (const p of this.particles) {
      if (p.inUse) continue;
      if (spawned >= BURST_SIZE) break;

      const angle = Math.random() * Math.PI * 2;
      const speed = PARTICLE_SPEED * (0.5 + Math.random() * 0.5);

      p.graphics.x = x;
      p.graphics.y = y;
      p.graphics.tint = color;
      p.graphics.alpha = 1;
      p.graphics.visible = true;
      p.vx = Math.cos(angle) * speed;
      p.vy = Math.sin(angle) * speed;
      p.timer = 0;
      p.inUse = true;
      spawned++;
    }
  }

  /**
   * Update all active particles. Call once per frame.
   */
  tick(dt: number): void {
    for (const p of this.particles) {
      if (!p.inUse) continue;

      p.timer += dt;
      if (p.timer >= PARTICLE_LIFETIME) {
        p.inUse = false;
        p.graphics.visible = false;
        continue;
      }

      p.graphics.x += p.vx * dt;
      p.graphics.y += p.vy * dt;
      p.graphics.alpha = 1 - p.timer / PARTICLE_LIFETIME;
    }
  }
}
