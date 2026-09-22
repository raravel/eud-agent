import type {
  MapLayer,
  RowSpan,
  SelectionMask,
  SelectionOperation,
  SelectionRole,
  TileRect,
} from "./mapProtocol";

export interface TilePoint {
  x: number;
  y: number;
}

const cellKey = ({ x, y }: TilePoint) => `${x},${y}`;

export function pointFromCellKey(key: string): TilePoint {
  const [x, y] = key.split(",").map(Number);
  return { x, y };
}

export function connectGridCells(from: TilePoint, to: TilePoint): TilePoint[] {
  let x = from.x;
  let y = from.y;
  const dx = Math.abs(to.x - from.x);
  const dy = Math.abs(to.y - from.y);
  const sx = from.x < to.x ? 1 : -1;
  const sy = from.y < to.y ? 1 : -1;
  let error = dx - dy;
  const cells: TilePoint[] = [];
  while (true) {
    cells.push({ x, y });
    if (x === to.x && y === to.y) break;
    const doubled = error * 2;
    if (doubled > -dy) {
      error -= dy;
      x += sx;
    }
    if (doubled < dx) {
      error += dx;
      y += sy;
    }
  }
  return cells;
}

export function rectangleCells(
  start: TilePoint,
  end: TilePoint,
  width: number,
  height: number,
): Set<string> {
  const left = Math.max(0, Math.min(start.x, end.x));
  const right = Math.min(width - 1, Math.max(start.x, end.x));
  const top = Math.max(0, Math.min(start.y, end.y));
  const bottom = Math.min(height - 1, Math.max(start.y, end.y));
  const cells = new Set<string>();
  for (let y = top; y <= bottom; y += 1) {
    for (let x = left; x <= right; x += 1) cells.add(`${x},${y}`);
  }
  return cells;
}

function pointInsidePolygon(point: TilePoint, polygon: TilePoint[]): boolean {
  let inside = false;
  for (let current = 0, previous = polygon.length - 1; current < polygon.length; previous = current++) {
    const a = polygon[current];
    const b = polygon[previous];
    const crosses =
      a.y > point.y !== b.y > point.y &&
      point.x < ((b.x - a.x) * (point.y - a.y)) / (b.y - a.y || Number.EPSILON) + a.x;
    if (crosses) inside = !inside;
  }
  return inside;
}

export function freeMaskCells(
  samples: TilePoint[],
  width: number,
  height: number,
): Set<string> {
  const cells = new Set<string>();
  if (samples.length === 0) return cells;
  const bounded = samples.map((sample) => ({
    x: Math.max(0, Math.min(width - 1, sample.x)),
    y: Math.max(0, Math.min(height - 1, sample.y)),
  }));
  for (let index = 1; index < bounded.length; index += 1) {
    for (const cell of connectGridCells(bounded[index - 1], bounded[index])) {
      cells.add(cellKey(cell));
    }
  }
  cells.add(cellKey(bounded[0]));

  const first = bounded[0];
  const last = bounded[bounded.length - 1];
  const closed =
    bounded.length >= 3 &&
    Math.max(Math.abs(first.x - last.x), Math.abs(first.y - last.y)) <= 1;
  if (!closed) return cells;

  const polygon = bounded.map(({ x, y }) => ({ x: x + 0.5, y: y + 0.5 }));
  const left = Math.max(0, Math.min(...bounded.map(({ x }) => x)));
  const right = Math.min(width - 1, Math.max(...bounded.map(({ x }) => x)));
  const top = Math.max(0, Math.min(...bounded.map(({ y }) => y)));
  const bottom = Math.min(height - 1, Math.max(...bounded.map(({ y }) => y)));
  for (let y = top; y <= bottom; y += 1) {
    for (let x = left; x <= right; x += 1) {
      if (pointInsidePolygon({ x: x + 0.5, y: y + 0.5 }, polygon)) {
        cells.add(`${x},${y}`);
      }
    }
  }
  return cells;
}

export function combineCells(
  current: Set<string>,
  incoming: Set<string>,
  operation: SelectionOperation,
): Set<string> {
  if (operation === "clear") return new Set();
  if (operation === "replace") return new Set(incoming);
  const result = new Set(current);
  if (operation === "add") {
    for (const cell of incoming) result.add(cell);
  } else if (operation === "subtract") {
    for (const cell of incoming) result.delete(cell);
  } else {
    for (const cell of incoming) {
      if (result.has(cell)) result.delete(cell);
      else result.add(cell);
    }
  }
  return result;
}

export function selectionCellsForGesture(options: {
  baseCells: Set<string>;
  start: TilePoint;
  end: TilePoint;
  samples: TilePoint[];
  moved: boolean;
  shape: "rectangle" | "free";
  operation: SelectionOperation;
  width: number;
  height: number;
}): Set<string> {
  if (!options.moved) {
    return options.baseCells.has(cellKey(options.end))
      ? new Set(options.baseCells)
      : new Set();
  }
  const incoming =
    options.shape === "free"
      ? freeMaskCells([...options.samples, options.end], options.width, options.height)
      : rectangleCells(options.start, options.end, options.width, options.height);
  return combineCells(options.baseCells, incoming, options.operation);
}

export function cellsToRows(cells: Set<string>): RowSpan[] {
  const byRow = new Map<number, number[]>();
  for (const key of cells) {
    const { x, y } = pointFromCellKey(key);
    const row = byRow.get(y) ?? [];
    row.push(x);
    byRow.set(y, row);
  }
  return Array.from(byRow.entries())
    .sort(([left], [right]) => left - right)
    .map(([y, values]) => {
      const xs = Array.from(new Set(values)).sort((left, right) => left - right);
      const spans: [number, number][] = [];
      for (const x of xs) {
        const previous = spans.at(-1);
        if (previous && previous[1] === x) previous[1] = x + 1;
        else spans.push([x, x + 1]);
      }
      return { y, spans };
    });
}

export function rowsToCells(rows: RowSpan[]): Set<string> {
  const cells = new Set<string>();
  for (const row of rows) {
    for (const [left, right] of row.spans) {
      for (let x = left; x < right; x += 1) cells.add(`${x},${row.y}`);
    }
  }
  return cells;
}

export interface OutlineSegment {
  x0: number;
  y0: number;
  x1: number;
  y1: number;
}

function normalizeSpans(spans: [number, number][]): [number, number][] {
  const sorted = spans
    .filter(([left, right]) => right > left)
    .map(([left, right]): [number, number] => [left, right])
    .sort(([a], [b]) => a - b);
  const merged: [number, number][] = [];
  for (const [left, right] of sorted) {
    const previous = merged.at(-1);
    if (previous && left <= previous[1]) previous[1] = Math.max(previous[1], right);
    else merged.push([left, right]);
  }
  return merged;
}

function subtractSpans(
  [left, right]: [number, number],
  spans: [number, number][],
): [number, number][] {
  const remaining: [number, number][] = [];
  let cursor = left;
  for (const [a, b] of spans) {
    if (b <= cursor) continue;
    if (a >= right) break;
    if (a > cursor) remaining.push([cursor, a]);
    cursor = Math.max(cursor, b);
    if (cursor >= right) break;
  }
  if (cursor < right) remaining.push([cursor, right]);
  return remaining;
}

/**
 * Boundary edges of a row-span mask in tile units: an edge exists only where a
 * selected tile meets an unselected one, so contiguous tiles share no line.
 */
export function rowSpanOutline(rows: RowSpan[]): OutlineSegment[] {
  const byY = new Map<number, [number, number][]>();
  for (const row of rows) {
    byY.set(row.y, normalizeSpans([...(byY.get(row.y) ?? []), ...row.spans]));
  }
  const segments: OutlineSegment[] = [];
  for (const [y, spans] of byY) {
    const above = byY.get(y - 1) ?? [];
    const below = byY.get(y + 1) ?? [];
    for (const span of spans) {
      const [left, right] = span;
      segments.push({ x0: left, y0: y, x1: left, y1: y + 1 });
      segments.push({ x0: right, y0: y, x1: right, y1: y + 1 });
      for (const [a, b] of subtractSpans(span, above)) segments.push({ x0: a, y0: y, x1: b, y1: y });
      for (const [a, b] of subtractSpans(span, below)) {
        segments.push({ x0: a, y0: y + 1, x1: b, y1: y + 1 });
      }
    }
  }
  return segments;
}

export function selectionBounds(cells: Set<string>): TileRect | null {
  if (cells.size === 0) return null;
  const points = Array.from(cells, pointFromCellKey);
  return {
    left: Math.min(...points.map(({ x }) => x)),
    top: Math.min(...points.map(({ y }) => y)),
    right: Math.max(...points.map(({ x }) => x)) + 1,
    bottom: Math.max(...points.map(({ y }) => y)) + 1,
  };
}

export function buildSelectionMask(options: {
  id: string;
  label: string;
  sourceRevision: string;
  role: SelectionRole;
  layers: MapLayer[];
  cells: Set<string>;
  width: number;
  height: number;
}): SelectionMask {
  const points = Array.from(options.cells, pointFromCellKey);
  if (
    points.length === 0 ||
    points.some(({ x, y }) => x < 0 || y < 0 || x >= options.width || y >= options.height)
  ) {
    throw new Error("선택 마스크가 비어 있거나 맵 범위를 벗어났습니다.");
  }
  const bounds = selectionBounds(options.cells);
  if (!bounds) throw new Error("빈 선택은 저장할 수 없습니다.");
  return {
    id: options.id,
    label: options.label.trim() || "영역",
    sourceRevision: options.sourceRevision,
    role: options.role,
    layers: Array.from(new Set(options.layers)).sort(),
    bounds,
    selectedCells: options.cells.size,
    rows: cellsToRows(options.cells),
  };
}
