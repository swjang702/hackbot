/**
 * Panel layout manager — exposes DOM references and handles resize.
 */

export interface PanelRefs {
  gameContainer: HTMLDivElement;
  eventLog: HTMLDivElement;
  pidFilters: HTMLDivElement;
  typeFilters: HTMLDivElement;
  playBtn: HTMLButtonElement;
  speedBtns: NodeListOf<HTMLButtonElement>;
  scrubSlider: HTMLInputElement;
  timelinePosition: HTMLSpanElement;
  connectionStatus: HTMLDivElement;
}

export function getPanelRefs(): PanelRefs {
  return {
    gameContainer: document.getElementById(
      "game-canvas-container",
    ) as HTMLDivElement,
    eventLog: document.getElementById("event-log") as HTMLDivElement,
    pidFilters: document.getElementById("pid-filters") as HTMLDivElement,
    typeFilters: document.getElementById("type-filters") as HTMLDivElement,
    playBtn: document.getElementById("play-btn") as HTMLButtonElement,
    speedBtns: document.querySelectorAll(
      ".speed-btn",
    ) as NodeListOf<HTMLButtonElement>,
    scrubSlider: document.getElementById("scrub-slider") as HTMLInputElement,
    timelinePosition: document.getElementById(
      "timeline-position",
    ) as HTMLSpanElement,
    connectionStatus: document.getElementById(
      "connection-status",
    ) as HTMLDivElement,
  };
}
