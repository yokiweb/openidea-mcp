import { beforeEach, describe, expect, it, vi } from "vitest";
import type { RunInfo } from "@/platform/ipc";

const ipc = vi.hoisted(() => ({
  fsReadRange: vi.fn(),
  runList: vi.fn(async () => [] as RunInfo[]),
  runStop: vi.fn(async () => ({ status: "ok" as const, data: null })),
}));
vi.mock("@/platform/ipc", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/platform/ipc")>()),
  isTauri: () => true,
  commands: ipc,
}));
vi.mock("@tauri-apps/api/path", () => ({
  appLocalDataDir: async () => "/local",
  join: async (...parts: string[]) => parts.join("/"),
}));

import { adoptExternalRuns, forgetAdoptedLogs, onRunExited, runtime, useRun } from "./store";
import { syncExternalRuns, useExternalRuns } from "./services";

const b64 = (s: string) => Buffer.from(s, "utf8").toString("base64");
const info = (id: number): RunInfo => ({
  id,
  configId: "app:main",
  name: "Main",
  pid: 1000 + id,
  running: true,
  pty: false,
});
const ok = (offset: number, text: string, total = offset + Buffer.byteLength(text)) => ({
  status: "ok" as const,
  data: { offset, data: b64(text), total },
});

beforeEach(() => {
  useRun.setState({ sessions: [], configs: [], activeSession: null });
  useExternalRuns.setState({ runs: [] });
  forgetAdoptedLogs();
  ipc.fsReadRange.mockReset();
  ipc.runList.mockReset();
  ipc.runList.mockResolvedValue([]);
  ipc.fsReadRange.mockResolvedValue(ok(0, ""));
});

describe("adopting backend-owned runs (in-app MCP)", () => {
  it("creates a real session with a console fed from the shared run log", async () => {
    ipc.fsReadRange.mockResolvedValue(ok(0, "server started\n"));
    await adoptExternalRuns([info(7)]);
    const [session] = useRun.getState().sessions;
    expect(session).toMatchObject({ runId: 7, status: "running", stdinClosed: true });
    expect(session!.config.id).toBe("app:main");
    expect(runtime(session!.id)).toBeDefined();
    expect(ipc.fsReadRange).toHaveBeenCalledWith("/local/run-logs/7.log", 0, 262_144);
    await vi.waitFor(() => expect(runtime(session!.id)!.console.lines[0]?.text).toContain("server started"));
  });

  it("does not adopt a run twice or one already owned by the frontend", async () => {
    await adoptExternalRuns([info(7), info(7)]);
    expect(useRun.getState().sessions).toHaveLength(1);
    await adoptExternalRuns([info(7)]);
    expect(useRun.getState().sessions).toHaveLength(1);
  });

  it("keeps UTF-8 characters intact across chunk boundaries", async () => {
    const bytes = Buffer.from("中文日志\n", "utf8");
    ipc.fsReadRange.mockImplementation(async (_path: string, offset: number) => {
      if (offset === 0)
        return {
          status: "ok" as const,
          data: { offset, data: bytes.subarray(0, 2).toString("base64"), total: bytes.length },
        };
      return {
        status: "ok" as const,
        data: { offset, data: bytes.subarray(offset).toString("base64"), total: bytes.length },
      };
    });
    await adoptExternalRuns([info(8)]);
    const [session] = useRun.getState().sessions;
    await vi.waitFor(() =>
      expect(
        runtime(session!.id)!
          .console.lines.map((l) => l.text)
          .join(""),
      ).toContain("中文日志"),
    );
  });

  it("stops tailing once an exited run's log is drained", async () => {
    ipc.fsReadRange.mockResolvedValue(ok(0, ""));
    await adoptExternalRuns([info(9)]);
    onRunExited(9, 0);
    await vi.waitFor(() => expect(useRun.getState().sessions[0]!.status).toBe("finished"));
    for (let i = 0; i < 4; i++) await adoptExternalRuns([]);
    const calls = ipc.fsReadRange.mock.calls.length;
    await adoptExternalRuns([]);
    expect(ipc.fsReadRange.mock.calls.length).toBe(calls);
  });

  it("syncExternalRuns adopts and drops the fallback state mirror", async () => {
    ipc.runList.mockResolvedValue([info(10)]);
    ipc.fsReadRange.mockResolvedValue(ok(0, "up\n"));
    await syncExternalRuns();
    expect(useRun.getState().sessions.map((s) => s.runId)).toContain(10);
    expect(useExternalRuns.getState().runs).toEqual([]);
  });
});
