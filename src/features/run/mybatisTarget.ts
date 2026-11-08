/**
 * Opens a MyBatis statement restored from the run console in a query console,
 * formatted the way the SQL editor lays it out.
 *
 * The data source is the one last used for this, else the one selected in the
 * Database tool window, else the only one; with several to choose from the
 * click offers them. Without any, the statement goes to the clipboard.
 */
import { tx } from "@/platform/i18n";
import { sourceLabel } from "@/features/database/model";
import { showContextMenu } from "@/ui";
import { notifications } from "@/workbench/contributions";
import { formatMybatisSql } from "./mybatis";

const SOURCE_KEY = "openidea.mybatis.source";

function rememberedSource(): string | null {
  try {
    return localStorage.getItem(SOURCE_KEY);
  } catch {
    return null; // storage blocked (private window)
  }
}

function rememberSource(id: string) {
  try {
    localStorage.setItem(SOURCE_KEY, id);
  } catch {
    // storage blocked: the choice just is not remembered
  }
}

/** `sql` opened in the query console of a data source, as a statement of its own. */
export async function openMybatisSql(sql: string, at?: { clientX: number; clientY: number }): Promise<void> {
  const store = await import("@/features/database/store");
  if (!store.useDatabase.getState().loaded) await store.loadSources().catch(() => undefined);
  const text = `${formatMybatisSql(sql)};`;
  const open = (id: string) => {
    rememberSource(id);
    void store.openConsoleWith(id, text);
  };
  const { sources, selectedSource } = store.useDatabase.getState();
  const target = rememberedSource() ?? selectedSource ?? (sources.length === 1 ? sources[0]!.id : null);
  if (target) {
    open(target);
    return;
  }
  if (sources.length > 1 && at) {
    showContextMenu(
      at,
      sources
        .filter((s) => !!s.id)
        .map((s) => {
          const id = s.id as string;
          return { label: sourceLabel(s), icon: "database" as const, run: () => open(id) };
        }),
    );
    return;
  }
  void navigator.clipboard?.writeText(text).catch(() => undefined);
  notifications().notify({
    level: "warning",
    title: tx("Cannot open the query console"),
    body: tx("The MyBatis SQL was copied to the clipboard; add a data source to run it."),
  });
}
