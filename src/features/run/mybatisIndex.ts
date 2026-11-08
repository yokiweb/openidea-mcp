/**
 * Incremental index of the MyBatis statements in a console buffer, plus the
 * per-line marks the console renders: syntax tokens on a `Preparing:` line and
 * the clickable range on its `Parameters:` line.
 *
 * The console appends a line at a time, so a full rescan per append would be
 * O(console). Only the statement that can still change — the trailing
 * `Preparing:` without parameters yet — is rescanned; marks are created once
 * per line and kept, so a memoised row is not re-rendered by new output.
 */
import type { ConsoleLine } from "./console";
import { mybatisTokens, parseMybatisLog, type MybatisEntry } from "./mybatis";

const PREPARING = "Preparing:";

/** How one console line takes part in a MyBatis statement. */
export interface MybatisMark {
  /** `sql` on the statement line, `params` on its `Parameters:` line. */
  role: "sql" | "params";
  /** Console line id of the statement (`Preparing:` / p6spy `Execute SQL：`). */
  entryId: number;
  /** `sql`: highlight tokens over the whole line (UTF-16 offsets). */
  tokens?: [number, number, string][];
  /** `params`: where the clickable value starts on the line. */
  paramsAt?: number;
}

export class MybatisIndex {
  /** First line whose statement may still change on the next update. */
  private pendingFrom = 0;
  /** Id of the buffer's first line, to spot a clear or a trim. */
  private firstId = -1;
  /** Statement line id → entry. */
  private readonly entries = new Map<number, MybatisEntry>();
  /** `Parameters:` line id → statement line id. */
  private readonly byParamsLine = new Map<number, number>();
  /** Line id → mark, created once so row memoisation survives new output. */
  private readonly marks = new Map<number, MybatisMark>();

  private reset() {
    this.pendingFrom = 0;
    this.firstId = -1;
    this.entries.clear();
    this.byParamsLine.clear();
    this.marks.clear();
  }

  /** Rescans the statements that new output could have completed or extended. */
  update(lines: readonly ConsoleLine[]): void {
    if (!lines.length) {
      this.reset();
      return;
    }
    // Line ids only grow, so another first line means the head was dropped.
    if (this.firstId !== lines[0]!.id) this.reset();
    this.firstId = lines[0]!.id;
    const from = Math.min(this.pendingFrom, lines.length - 1);
    const first = lines[from]!;
    // A cleared or trimmed console makes the kept entries point at other lines.
    for (const [id, entry] of this.entries) {
      if (id >= first.id) {
        this.entries.delete(id);
        if (entry.paramsLineId !== null) this.byParamsLine.delete(entry.paramsLineId);
      }
    }
    for (const entry of parseMybatisLog(lines.slice(from))) {
      this.entries.set(entry.lineId, entry);
      if (entry.paramsLineId !== null) this.byParamsLine.set(entry.paramsLineId, entry.lineId);
    }
    // Only the last `Preparing:` can still be waiting for its `Parameters:`.
    this.pendingFrom = lines.length;
    for (let i = lines.length - 1; i >= from; i--) {
      if (!lines[i]!.text.includes(PREPARING)) continue;
      const entry = this.entries.get(lines[i]!.id);
      if (!entry || entry.paramsLineId === null) this.pendingFrom = i;
      break;
    }
  }

  /** The mark for a console line, or `undefined` when it is not MyBatis output. */
  mark(lineId: number): MybatisMark | undefined {
    const cached = this.marks.get(lineId);
    if (cached) return cached;
    const statement = this.entries.get(lineId);
    if (statement) {
      const at = statement.sqlStart;
      const mark: MybatisMark = {
        role: "sql",
        entryId: lineId,
        tokens: mybatisTokens(statement.preparing).map(([s, e, k]) => [s + at, e + at, k]),
      };
      this.marks.set(lineId, mark);
      return mark;
    }
    const entryId = this.byParamsLine.get(lineId);
    if (entryId === undefined) return undefined;
    const entry = this.entries.get(entryId)!;
    const mark: MybatisMark = { role: "params", entryId, paramsAt: entry.paramsStart ?? 0 };
    this.marks.set(lineId, mark);
    return mark;
  }

  /** The statement, with its parameters bound, for opening in a query console. */
  entry(entryId: number): MybatisEntry | undefined {
    return this.entries.get(entryId);
  }
}
