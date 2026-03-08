/**
 * App orchestrator — wires Connection, GameWorld, and UI together.
 */

import { Connection } from "./connection";
import { GameWorld } from "./game/world";
import { Controls } from "./ui/controls";
import { EventLog } from "./ui/event-log";
import { getPanelRefs } from "./ui/layout";
import { Timeline } from "./ui/timeline";

export class App {
  private conn: Connection;
  private world: GameWorld;
  private timeline: Timeline;
  private eventLog: EventLog;
  private controls: Controls;
  private startNsSet = false;

  constructor() {
    const refs = getPanelRefs();

    // Determine WebSocket URL (same host, /ws path)
    const wsProtocol = location.protocol === "https:" ? "wss:" : "ws:";
    const wsUrl = `${wsProtocol}//${location.host}/ws`;

    this.conn = new Connection(wsUrl);
    this.world = new GameWorld();
    this.timeline = new Timeline(this.conn, refs);
    this.eventLog = new EventLog(refs.eventLog);
    this.controls = new Controls(this.conn, refs);

    // Wire up message handlers
    this.conn.onWorldState((msg) => {
      this.world.handleWorldState(msg);
      this.controls.handleWorldState(msg);

    });

    this.conn.onEvents((msg) => {
      this.world.handleEvents(msg);
      this.eventLog.handleEvents(msg);
    });

    this.conn.onPlayback((msg) => {
      this.timeline.handlePlayback(msg);
      // Use start_ns from playback for event log relative timestamps
      if (!this.startNsSet && msg.start_ns) {
        this.eventLog.setStartTimestamp(msg.start_ns);
        this.startNsSet = true;
      }
    });

    // Connection status indicator
    const statusEl = refs.connectionStatus;
    const checkStatus = () => {
      if (this.conn.connected) {
        statusEl.textContent = "connected";
        statusEl.className = "connected";
      } else {
        statusEl.textContent = "disconnected";
        statusEl.className = "";
      }
    };
    setInterval(checkStatus, 1000);

    // Store refs for init
    this._gameContainer = refs.gameContainer;
  }

  private _gameContainer: HTMLDivElement;

  async init(): Promise<void> {
    await this.world.init(this._gameContainer);
    this.conn.connect();
  }
}
