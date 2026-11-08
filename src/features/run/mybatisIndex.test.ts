import { describe, expect, it } from "vitest";
import type { ConsoleLine } from "./console";
import { MybatisIndex } from "./mybatisIndex";

const line = (id: number, text: string): ConsoleLine => ({ id, kind: "stdout", text, spans: [{ text }] });

const PREPARING = line(1, "12:00 DEBUG - ==> Preparing: select * from t_user where name = ? and age > ?");
const PARAMETERS = line(2, "12:00 DEBUG - ==> Parameters: hello(String), 123(Integer)");

describe("MybatisIndex", () => {
  it("marks the statement line and its parameters line", () => {
    const index = new MybatisIndex();
    index.update([PREPARING, PARAMETERS]);
    expect(index.mark(1)?.role).toBe("sql");
    expect(index.mark(2)?.role).toBe("params");
    expect(index.mark(2)?.paramsAt).toBe(PARAMETERS.text.indexOf("hello"));
    expect(index.mark(1)?.entryId).toBe(1);
    expect(index.mark(2)?.entryId).toBe(1);
  });

  it("leaves other output alone", () => {
    const index = new MybatisIndex();
    index.update([line(1, "Started Application in 3.2 seconds"), line(2, "==>  Total: 1")]);
    expect(index.mark(1)).toBeUndefined();
    expect(index.mark(2)).toBeUndefined();
  });

  it("completes the statement when the parameters arrive later", () => {
    const index = new MybatisIndex();
    index.update([PREPARING]);
    expect(index.entry(1)?.params).toHaveLength(0);
    expect(index.entry(1)?.complete).toBe(false);
    index.update([PREPARING, PARAMETERS]);
    expect(index.entry(1)?.sql).toBe("select * from t_user where name = 'hello' and age > 123");
    expect(index.entry(1)?.complete).toBe(true);
    // The statement line's mark does not change, so an already rendered row is kept.
    expect(index.mark(1)).toBe(index.mark(1));
  });

  it("colours keywords at their offsets within the whole line", () => {
    const index = new MybatisIndex();
    index.update([PREPARING]);
    const tokens = index.mark(1)!.tokens!;
    const keyword = tokens.find(([s, e]) => PREPARING.text.slice(s, e) === "select");
    expect(keyword?.[2]).toBe("keyword");
    const from = tokens.find(([s, e]) => PREPARING.text.slice(s, e) === "from");
    expect(from?.[2]).toBe("keyword");
  });

  it("marks p6spy output, whose statement is already bound", () => {
    const index = new MybatisIndex();
    const bound = line(1, "Execute SQL：select * from t where id = 1");
    index.update([bound]);
    expect(index.mark(1)?.role).toBe("sql");
    expect(index.entry(1)?.sql).toBe("select * from t where id = 1");
  });

  it("drops the statements of a cleared console", () => {
    const index = new MybatisIndex();
    index.update([PREPARING, PARAMETERS]);
    index.update([]);
    expect(index.mark(1)).toBeUndefined();
    expect(index.entry(1)).toBeUndefined();
  });

  it("keeps working after the console buffer is trimmed", () => {
    const index = new MybatisIndex();
    index.update([line(1, "boot"), line(2, "==> Preparing: select 1"), line(3, "==> Parameters:")]);
    expect(index.entry(2)?.complete).toBe(true);
    // Trimming drops the oldest lines: the buffer no longer starts at the first id.
    index.update([line(40, "==> Preparing: select * from t where a = ?"), line(41, "==> Parameters: 1(Integer)")]);
    expect(index.entry(40)?.sql).toBe("select * from t where a = 1");
    expect(index.mark(41)?.role).toBe("params");
    expect(index.entry(2)).toBeUndefined();
  });
});
