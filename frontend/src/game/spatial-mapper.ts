/**
 * Compute 2D positions for processes using a tree layout.
 *
 * Root process at top-center, children below, arranged horizontally.
 * Deterministic layout matching pstree mental model.
 */

import type { ProcessInfo } from "../types";

export interface ProcessLayout {
  x: number;
  y: number;
  width: number;
  height: number;
}

const ROOM_WIDTH = 160;
const ROOM_HEIGHT = 100;
const H_GAP = 24;
const V_GAP = 40;
const PADDING = 40;

interface TreeNode {
  process: ProcessInfo;
  children: TreeNode[];
}

export function computeLayout(
  processes: ProcessInfo[],
): Map<number, ProcessLayout> {
  const result = new Map<number, ProcessLayout>();
  if (processes.length === 0) return result;

  // Filter out PID 0 (kernel) — it's not a real process room
  const visible = processes.filter((p) => p.pid !== 0);
  if (visible.length === 0) return result;

  // Build tree from parent-child relationships
  const byPid = new Map(visible.map((p) => [p.pid, p]));
  const childrenOf = new Map<number, ProcessInfo[]>();

  for (const p of visible) {
    const parentPid = p.parent_pid;
    if (parentPid !== null && byPid.has(parentPid)) {
      const existing = childrenOf.get(parentPid) ?? [];
      existing.push(p);
      childrenOf.set(parentPid, existing);
    }
  }

  // Find roots (processes whose parent is not in the visible set)
  const roots = visible.filter(
    (p) => p.parent_pid === null || !byPid.has(p.parent_pid),
  );

  // Build tree recursively
  function buildTree(proc: ProcessInfo): TreeNode {
    const children = (childrenOf.get(proc.pid) ?? [])
      .sort((a, b) => a.pid - b.pid)
      .map(buildTree);
    return { process: proc, children };
  }

  const trees = roots.sort((a, b) => a.pid - b.pid).map(buildTree);

  // Compute subtree widths (for horizontal centering)
  function subtreeWidth(node: TreeNode): number {
    if (node.children.length === 0) return ROOM_WIDTH;
    const childWidths = node.children.map(subtreeWidth);
    return Math.max(
      ROOM_WIDTH,
      childWidths.reduce((a, b) => a + b, 0) +
        (node.children.length - 1) * H_GAP,
    );
  }

  // Layout: assign x,y to each node
  function layoutNode(node: TreeNode, x: number, y: number): void {
    // Center this node over its subtree
    const sw = subtreeWidth(node);
    const nodeX = x + (sw - ROOM_WIDTH) / 2;

    result.set(node.process.pid, {
      x: nodeX,
      y,
      width: ROOM_WIDTH,
      height: ROOM_HEIGHT,
    });

    // Layout children below
    let childX = x;
    for (const child of node.children) {
      const cw = subtreeWidth(child);
      layoutNode(child, childX, y + ROOM_HEIGHT + V_GAP);
      childX += cw + H_GAP;
    }
  }

  // Layout all root trees side by side
  let startX = PADDING;
  for (const tree of trees) {
    layoutNode(tree, startX, PADDING);
    startX += subtreeWidth(tree) + H_GAP * 2;
  }

  return result;
}
