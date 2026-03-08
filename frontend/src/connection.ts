/**
 * WebSocket client with auto-reconnect and message dispatch.
 */

import type {
  ClientCommand,
  EventsMessage,
  PlaybackMessage,
  ServerMessage,
  WorldStateMessage,
} from "./types";

type MessageHandler<T> = (msg: T) => void;

export class Connection {
  private ws: WebSocket | null = null;
  private url: string;
  private reconnectDelay = 1000;
  private maxReconnectDelay = 30000;
  private shouldReconnect = true;

  private worldStateHandlers: MessageHandler<WorldStateMessage>[] = [];
  private eventsHandlers: MessageHandler<EventsMessage>[] = [];
  private playbackHandlers: MessageHandler<PlaybackMessage>[] = [];

  constructor(url: string) {
    this.url = url;
  }

  connect(): void {
    this.shouldReconnect = true;
    this._connect();
  }

  disconnect(): void {
    this.shouldReconnect = false;
    this.ws?.close();
    this.ws = null;
  }

  send(cmd: ClientCommand): void {
    if (this.ws?.readyState === WebSocket.OPEN) {
      this.ws.send(JSON.stringify(cmd));
    }
  }

  onWorldState(handler: MessageHandler<WorldStateMessage>): void {
    this.worldStateHandlers.push(handler);
  }

  onEvents(handler: MessageHandler<EventsMessage>): void {
    this.eventsHandlers.push(handler);
  }

  onPlayback(handler: MessageHandler<PlaybackMessage>): void {
    this.playbackHandlers.push(handler);
  }

  get connected(): boolean {
    return this.ws?.readyState === WebSocket.OPEN;
  }

  private _connect(): void {
    this.ws = new WebSocket(this.url);

    this.ws.onopen = () => {
      this.reconnectDelay = 1000;
      console.log("[ws] connected");
    };

    this.ws.onmessage = (event) => {
      this._dispatch(event.data as string);
    };

    this.ws.onclose = () => {
      console.log("[ws] disconnected");
      if (this.shouldReconnect) {
        setTimeout(() => this._connect(), this.reconnectDelay);
        this.reconnectDelay = Math.min(
          this.reconnectDelay * 2,
          this.maxReconnectDelay,
        );
      }
    };

    this.ws.onerror = () => {
      this.ws?.close();
    };
  }

  private _dispatch(raw: string): void {
    let msg: ServerMessage;
    try {
      msg = JSON.parse(raw) as ServerMessage;
    } catch {
      console.warn("[ws] invalid JSON:", raw.slice(0, 200));
      return;
    }

    switch (msg.msg) {
      case "world_state":
        for (const h of this.worldStateHandlers) h(msg);
        break;
      case "events":
        for (const h of this.eventsHandlers) h(msg);
        break;
      case "playback":
        for (const h of this.playbackHandlers) h(msg);
        break;
    }
  }
}
