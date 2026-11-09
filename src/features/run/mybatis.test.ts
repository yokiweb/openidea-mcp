import { describe, expect, it } from "vitest";
import {
  assembleMybatisSql,
  formatMybatisSql,
  mybatisLiteral,
  mybatisTokens,
  parseMybatisLog,
  parseMybatisParameters,
} from "./mybatis";

const log = (text: string) => text.split("\n").map((t, i) => ({ id: i + 1, text: t }));

/** 一条真实的长 CTE 日志：块注释 + 嵌套子查询 + Oracle 语法（G4 回归）。 */
const LONG_CTE =
  "WITH well AS ( /* 井号/单位过滤统一在这里解析成 jhdm 集合 */ SELECT j.jhdm FROM znsfqxmk.dj_jcxx j WHERE (j.jh IN ( ? ) OR j.jhdm IN ( ? )) /* 井代码直配分支：仅在未传 jbdm 时需要（传了 jbdm 时原 SQL 也要求 jcxx 行存在） */ UNION SELECT ? AS jhdm FROM dual ), production AS ( SELECT s.rq, s.id, s.jhdm, s.pzl, s.scsj, s.zsl, ROW_NUMBER() OVER (ORDER BY s.rq DESC, s.id DESC) AS row_number FROM znsfqxmk.sj_rd_scsj s WHERE s.jhdm IN (SELECT jhdm FROM well) ), treatment AS ( SELECT g.jhdm, g.qzyy, g.zlcs, ROW_NUMBER() OVER ( PARTITION BY g.jhdm ORDER BY g.zlsj DESC NULLS LAST, g.ny DESC NULLS LAST, g.id DESC ) AS row_number FROM znsfq_cygc.sj_bdq_qzzlxggz g WHERE g.jhdm IN (SELECT jhdm FROM well) ), record AS ( SELECT p.*, t.qzyy, t.zlcs FROM production p LEFT JOIN treatment t ON t.jhdm = p.jhdm AND t.row_number = 1 WHERE p.row_number = 1 ), conclusion AS ( SELECT 1 AS sort_no, '欠注井，欠注水量' AS label, CAST(ROUND(GREATEST( COALESCE(pzl, 0) * COALESCE(scsj, 0) / 24 - COALESCE(zsl, 0), 0 ), 2) AS VARCHAR) || ' m³/d' AS value FROM record UNION ALL SELECT 2 AS sort_no, '欠注原因' AS label, COALESCE(NULLIF(TRIM(qzyy), ''), '暂无') AS value FROM record UNION ALL SELECT 3 AS sort_no, '治理措施' AS label, COALESCE(NULLIF(TRIM(zlcs), ''), '暂无') AS value FROM record ) SELECT label, value FROM conclusion ORDER BY sort_no";

describe("parseMybatisParameters", () => {
  it("splits on known type anchors even with commas in values", () => {
    const ps = parseMybatisParameters("hello, world(String), 2026-01-01 00:00:00.0(Timestamp), 7(Integer)");
    expect(ps.map((p) => p.value)).toEqual(["hello, world", "2026-01-01 00:00:00.0", "7"]);
    expect(ps[0]!.ty).toBe("String");
    expect(ps[2]!.ty).toBe("Integer");
  });

  it("values with parentheses are not mistaken for types", () => {
    const ps = parseMybatisParameters("f(a,b)(String), 1(Integer)");
    expect(ps[0]!.value).toBe("f(a,b)");
    expect(ps[0]!.ty).toBe("String");
    expect(ps[1]!.literal).toBe("1");
  });

  it("prefix types (Time inside Timestamp) do not confuse the anchor", () => {
    const ps = parseMybatisParameters("2026-01-01 00:00:00.0(Timestamp), 2(Time)");
    expect(ps[0]!.ty).toBe("Timestamp");
    expect(ps[1]!.ty).toBe("Time");
  });
});

describe("mybatisLiteral", () => {
  it("maps types to SQL literals", () => {
    expect(mybatisLiteral("null", "String")).toBe("NULL");
    expect(mybatisLiteral("", "String")).toBe("NULL");
    expect(mybatisLiteral("2026-01-02 03:04:05.0", "Timestamp")).toBe("'2026-01-02 03:04:05.0'");
    expect(mybatisLiteral("-1.5", "Double")).toBe("-1.5");
    expect(mybatisLiteral("true", "Boolean")).toBe("true");
    expect(mybatisLiteral("o'brien", "String")).toBe("'o''brien'");
    expect(mybatisLiteral("it''s", "String")).toBe("'it''s'");
    expect(mybatisLiteral("42", "SomeType")).toBe("42");
    expect(mybatisLiteral("abc", "SomeType")).toBe("'abc'");
  });
});

describe("parseMybatisLog", () => {
  it("restores simple pairs", () => {
    const entries = parseMybatisLog(
      log(
        "12:00 DEBUG sql1 - ==> Preparing: select * from t_user where name = ? and age > ?\n" +
          "12:00 DEBUG sql1 - ==> Parameters: hello(String), 123(Integer)",
      ),
    );
    expect(entries).toHaveLength(1);
    expect(entries[0]!.sql).toBe("select * from t_user where name = 'hello' and age > 123");
    expect(entries[0]!.complete).toBe(true);
    expect(entries[0]!.lineId).toBe(1);
  });

  it("handles pre-escaped quotes, commas in values and null", () => {
    const entries = parseMybatisLog(
      log(
        "==> Preparing: update t_user set name = ?, remark = ? where id = ?\n" +
          "==> Parameters: it''s(String), a, b(String), null(Long)",
      ),
    );
    expect(entries[0]!.sql).toBe("update t_user set name = 'it''s', remark = 'a, b' where id = NULL");
    expect(entries[0]!.params).toHaveLength(3);
    expect(entries[0]!.complete).toBe(true);
  });

  it("parameters may wrap over continuation lines", () => {
    const entries = parseMybatisLog(
      log("==> Preparing: insert into t(a,b,c) values (?,?,?)\n==> Parameters: 1(Integer),\nxx(String),\n3.5(Double)"),
    );
    expect(entries[0]!.params.map((p) => p.literal)).toEqual(["1", "'xx'", "3.5"]);
    expect(entries[0]!.sql).toBe("insert into t(a,b,c) values (1,'xx',3.5)");
  });

  it("missing parameters leave placeholders and break completeness", () => {
    const entries = parseMybatisLog(
      log("==> Preparing: select * from t where a = ? and b = ?\n==> Parameters: 1(Integer)"),
    );
    expect(entries[0]!.sql).toBe("select * from t where a = 1 and b = ?");
    expect(entries[0]!.complete).toBe(false);
  });

  it("statements without parameters stay unsubstituted", () => {
    const entries = parseMybatisLog(log("==> Preparing: select * from t_log order by id asc\n==> Parameters:"));
    expect(entries[0]!.sql).toBe("select * from t_log order by id asc");
    expect(entries[0]!.complete).toBe(true);
  });

  it("skips statements whose parameters never appear before the next Preparing", () => {
    const entries = parseMybatisLog(log("==> Preparing: a ?\n==> Preparing: b ?\n==> Parameters: 1(Integer)"));
    expect(entries.map((e) => e.sql)).toEqual(["a ?", "b 1"]);
    expect(entries[0]!.complete).toBe(false);
  });
});

describe("assembleMybatisSql", () => {
  it("substitutes in order", () => {
    const params = parseMybatisParameters("1(Integer), x(String)");
    expect(assembleMybatisSql("a = ? and b = ?", params)).toEqual({ sql: "a = 1 and b = 'x'", complete: true });
  });
});

describe("formatMybatisSql", () => {
  it("lays the statement out the way the SQL editor does", () => {
    expect(formatMybatisSql("select a,b from t where x='it''s a test' and y>1 order by a desc")).toBe(
      "select a,\n       b\nfrom t\nwhere x = 'it''s a test'\n  and y > 1\norder by a desc",
    );
    expect(formatMybatisSql("update t_user set name = 'x', age = 3 where id = 1")).toBe(
      "update t_user\nset name = 'x',\n    age = 3\nwhere id = 1",
    );
  });

  it("keeps parenthesis spacing, numbers and joins", () => {
    expect(formatMybatisSql("select x from t where id in (1,2,3)")).toBe("select x\nfrom t\nwhere id in (1, 2, 3)");
    expect(formatMybatisSql("select * from a left join b on a.id = b.id where a.x = 1")).toContain(
      "\n         left join b on a.id = b.id",
    );
  });

  it("keeps a long single-line CTE query readable", () => {
    const entries = parseMybatisLog(
      log(`==> Preparing: ${LONG_CTE}\n==> Parameters: NHT165X4(String), NHT165X4(String), NHT165X4(String)`),
    );
    expect(entries[0]!.complete).toBe(true);
    const f = formatMybatisSql(entries[0]!.sql);
    const lines = f.split("\n");
    // 超长单行里嵌套的 query frame 曾把缩进逐层累加到 386 列，SQL 被推到屏幕外。
    expect(Math.max(...lines.map((l) => l.length - l.trimStart().length))).toBeLessThanOrEqual(48);
    expect(f.length).toBeLessThan(3000);
    expect(f).toContain("FROM conclusion");
    expect(f).toContain("ORDER BY sort_no");
    // 块注释后的 SELECT 仍是一个子查询，不能和它的 FROM 挤在同一行。
    expect(lines.find((l) => l.includes("SELECT j.jhdm"))).not.toContain("FROM znsfqxmk.dj_jcxx");
  });
});

describe("offsets recorded for the console", () => {
  it("points at the SQL and the parameters on their own lines", () => {
    const lines = log(
      "12:00 DEBUG - ==> Preparing: select * from t where a = ?\n12:00 DEBUG - ==> Parameters: 1(Integer)",
    );
    const [entry] = parseMybatisLog(lines);
    expect(lines[0]!.text.slice(entry!.sqlStart)).toBe("select * from t where a = ?");
    expect(lines[1]!.text.slice(entry!.paramsStart!)).toBe("1(Integer)");
    expect(entry!.paramsLineId).toBe(2);
  });

  it("reads p6spy's already-bound Execute SQL line", () => {
    const entries = parseMybatisLog(log("Consume Time：15 ms\nExecute SQL：select * from t where id = 1"));
    expect(entries).toHaveLength(1);
    expect(entries[0]!.sql).toBe("select * from t where id = 1");
    expect(entries[0]!.lineId).toBe(2);
    expect(entries[0]!.complete).toBe(true);
  });
});

describe("mybatisTokens", () => {
  it("covers keywords, strings, numbers and comments", () => {
    const ts = mybatisTokens("select a, 1 from t -- note");
    expect(ts).toContainEqual([0, 6, "keyword"]);
    expect(ts).toContainEqual([10, 11, "number"]);
    expect(ts.some(([, , k]) => k === "comment")).toBe(true);
    const sql = "select 'it''s' from t";
    expect(mybatisTokens(sql).some(([s, e, k]) => k === "string" && sql.slice(s, e) === "'it''s'")).toBe(true);
  });

  it("is total over odd input", () => {
    for (const s of ["'", "''", "select", "中文", ""]) expect(mybatisTokens(s)).toBeDefined();
  });
});
