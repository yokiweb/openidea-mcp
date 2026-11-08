/**
 * MyBatis log recognition for the Run console: pairs `==> Preparing:` /
 * `==> Parameters:` lines, substitutes the `?` placeholders with typed
 * literals, and hands the SQL to the database feature's own lexer and
 * formatter, so the console and a query console agree on how SQL looks.
 *
 * The format is MyBatis's standard output logger (`org.apache.ibatis.logging.
 * stdout`), which MyBatis-Plus prints unchanged; p6spy's `Execute SQL：` line
 * is recognised too, since its SQL is already bound.
 */
import { isReserved } from "@/features/database/sql/dialect";
import { formatted } from "@/features/database/sql/format";
import { lex } from "@/features/database/sql/lexer";
import type { DbDriver } from "@/platform/ipc";

export interface MybatisParam {
  /** The printed value (`it''s`, `2026-01-01 00:00:00.0`). */
  value: string;
  /** The Java type as printed (`String`, `Integer`, `Timestamp`, …). */
  ty: string;
  /** The SQL literal the value becomes. */
  literal: string;
}

export interface MybatisEntry {
  /** Console line id of the statement (`Preparing:` or p6spy `Execute SQL：`). */
  lineId: number;
  /** Console line id of the `Parameters:` line, when one followed. */
  paramsLineId: number | null;
  /** Offset of the SQL within its line (after the `Preparing:` prefix). */
  sqlStart: number;
  /** Offset of the printed parameters within their line, when there are any. */
  paramsStart: number | null;
  /** The raw SQL with `?` placeholders. */
  preparing: string;
  params: MybatisParam[];
  /** Executable SQL (substituted, unformatted). */
  sql: string;
  /** Every `?` got a parameter and no parameter was left over. */
  complete: boolean;
}

/** Types MyBatis prints as bare numeric or boolean values. */
const NUMERIC_TYPES = new Set([
  "Integer", "Long", "Short", "Byte", "Double", "Float", "BigDecimal", "BigInteger", "int", "long",
  "short", "byte", "double", "float", "Boolean", "boolean",
]);
/** Types MyBatis prints quoted (unknown types fall through to a guess). */
const TEXTUAL_TYPES = new Set([
  "String", "Character", "char", "Date", "Timestamp", "Time", "LocalDateTime", "LocalDate",
  "LocalTime",
]);

const PREPARING = "Preparing:";
const PARAMETERS = "Parameters:";
/** p6spy: `Execute SQL：select …` (full-width colon in its default format). */
const EXECUTE_SQL = /Execute SQL\s*[:：]\s*/;

const isNumber = (v: string) => {
  const t = v.startsWith("-") ? v.slice(1) : v;
  return t !== "" && /^[0-9.]*$/.test(t) && (t.match(/\./g)?.length ?? 0) <= 1;
};

/** The SQL literal a printed `value(Type)` becomes. */
export function mybatisLiteral(value: string, ty: string): string {
  const v = value.trim();
  if (!v || v.toLowerCase() === "null") return "NULL";
  if (NUMERIC_TYPES.has(ty)) return v;
  if (TEXTUAL_TYPES.has(ty)) return quote(v);
  // Unknown type: keep obvious numbers and booleans bare, quote the rest.
  return isNumber(v) || v === "true" || v === "false" ? v : quote(v);
}

function quote(v: string): string {
  // A value already containing `''` (as some log setups print it) is treated
  // as pre-escaped; otherwise single quotes are doubled.
  return v.includes("''") ? `'${v}'` : `'${v.replace(/'/g, "''")}'`;
}

/** If `text[i]` opens a `(Type)`: the type and the index past the `)`. */
function typeAt(text: string, i: number): [string, number] | null {
  const rest = text.slice(i + 1);
  for (const t of [...NUMERIC_TYPES, ...TEXTUAL_TYPES]) {
    if (rest.startsWith(t) && rest[t.length] === ")") return [t, i + 1 + t.length + 1];
  }
  const j = rest.indexOf(")");
  if (j < 0) return null;
  const ty = rest.slice(0, j);
  if (!ty || !/^[\w.[\]]+$/.test(ty)) return null;
  // Only a token-final `(Type)` counts (followed by `,` or the end), so a
  // value like `f(a,b)` is not mistaken for a type.
  const after = text[i + 1 + j + 1];
  return after === undefined || after === "," ? [ty, i + 1 + j + 1] : null;
}

/** Splits `v(T), v(T), …` into typed parameters (values may contain commas/parens). */
export function parseMybatisParameters(text: string): MybatisParam[] {
  const out: MybatisParam[] = [];
  let start = 0; // where the current value begins
  let scan = 0; // where the next `(` search begins
  for (;;) {
    const i = text.indexOf("(", scan);
    if (i < 0) break;
    const at = typeAt(text, i);
    if (at) {
      const [ty, end] = at;
      const value = text.slice(start, i).trim();
      if (value) out.push({ value, ty, literal: mybatisLiteral(value, ty) });
      let k = end;
      if (text[k] === ",") k++;
      start = scan = k + (text.slice(k).length - text.slice(k).trimStart().length);
    } else {
      scan = i + 1; // a `(` inside a value; keep scanning, value start unchanged
    }
  }
  // No typed tokens at all (`Parameters: null`, or a bare value): one untyped param.
  if (!out.length) {
    const rest = text.trim();
    if (rest) out.push({ value: rest, ty: "", literal: mybatisLiteral(rest, "") });
  }
  return out;
}

/** Substitutes each `?` (in order) with the parameters' literals. */
export function assembleMybatisSql(
  preparing: string,
  params: readonly MybatisParam[],
): { sql: string; complete: boolean } {
  let sql = "";
  let replaced = 0;
  let missing = false;
  for (const c of preparing) {
    if (c === "?") {
      const p = params[replaced];
      if (p) {
        sql += p.literal;
        replaced++;
      } else {
        sql += "?";
        missing = true;
      }
    } else sql += c;
  }
  return { sql, complete: !missing && replaced === params.length };
}

/** Where `prefix` ends in `text`, skipping the whitespace after it. */
function after(text: string, at: number): number {
  let i = at;
  while (i < text.length && (text[i] === " " || text[i] === "\t")) i++;
  return i;
}

/**
 * Pairs every `Preparing:` line with the next `Parameters:` line (whose value
 * may wrap over plain continuation lines) and substitutes the placeholders.
 * p6spy's `Execute SQL：` lines become entries of their own.
 */
export function parseMybatisLog(lines: readonly { id: number; text: string }[]): MybatisEntry[] {
  const out: MybatisEntry[] = [];
  for (let n = 0; n < lines.length; n++) {
    const text = lines[n]!.text;
    const bound = EXECUTE_SQL.exec(text);
    if (bound) {
      const sqlStart = bound.index + bound[0].length;
      const sql = text.slice(sqlStart).trim();
      if (sql) {
        out.push({
          lineId: lines[n]!.id,
          paramsLineId: null,
          sqlStart: after(text, sqlStart),
          paramsStart: null,
          preparing: sql,
          params: [],
          sql,
          // p6spy prints the statement with its parameters already bound.
          complete: true,
        });
      }
      continue;
    }
    const at = text.lastIndexOf(PREPARING);
    if (at < 0) continue;
    const sqlStart = after(text, at + PREPARING.length);
    const preparing = text.slice(sqlStart).trim();
    if (!preparing) continue;
    let paramsText = "";
    let paramsLineId: number | null = null;
    let paramsStart: number | null = null;
    for (let m = n + 1; m < lines.length; m++) {
      const t = lines[m]!.text;
      const pAt = t.lastIndexOf(PARAMETERS);
      if (pAt >= 0 && t.lastIndexOf(PREPARING) < 0) {
        paramsLineId = lines[m]!.id;
        paramsStart = after(t, pAt + PARAMETERS.length);
        paramsText += t.slice(paramsStart).trim();
        // A wrapped parameter list continues over plain lines.
        for (let k = m + 1; k < lines.length && k <= m + 64; k++) {
          const c = lines[k]!.text;
          const ct = c.trim();
          if (!ct || c.includes(PREPARING) || c.includes(PARAMETERS) || c.includes("==> ") || c.includes("<== ")) break;
          paramsText += " " + ct;
        }
        break;
      }
      if (t.includes(PREPARING)) break; // a new statement before its parameters
    }
    const params = parseMybatisParameters(paramsText);
    const { sql, complete } = assembleMybatisSql(preparing, params);
    out.push({
      lineId: lines[n]!.id,
      paramsLineId,
      sqlStart,
      paramsStart: params.length ? paramsStart : null,
      preparing,
      params,
      sql,
      complete,
    });
  }
  return out;
}

/** `sql` formatted the way the SQL console lays it out. */
export function formatMybatisSql(sql: string, driver: DbDriver = "sqlite"): string {
  return formatted(sql, driver);
}

/** Highlight tokens over `sql`: keyword/string/number/comment, UTF-16 offsets. */
export function mybatisTokens(sql: string, driver: DbDriver = "sqlite"): [number, number, string][] {
  const { tokens, comments } = lex(sql, driver);
  const out: [number, number, string][] = comments.map((c) => [c.from, c.to, "comment"]);
  for (const t of tokens) {
    if (t.kind === "word") {
      if (isReserved(t.text)) out.push([t.from, t.to, "keyword"]);
    } else if (t.kind === "number") out.push([t.from, t.to, "number"]);
    else if (t.kind === "string") out.push([t.from, t.to, "string"]);
  }
  return out.sort((a, b) => a[0] - b[0]);
}
