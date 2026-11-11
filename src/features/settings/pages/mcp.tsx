/**
 * Tools › MCP Server: an HTTP MCP server that runs inside the IDE, so AI agents
 * outside it (Claude Code, Codex…) can manage this window's project — its run
 * configurations ("services") and database queries — through MCP tools.
 *
 * The stdio variant needs no settings: `openidea --mcp --project <path>`. This
 * page therefore shows the two client configurations side by side and lets the
 * HTTP server be switched on for the project of the calling window.
 */

import { useEffect, useState } from "react";
import { tx } from "@/platform/i18n";
import { call, commands, isTauri, type McpServerStatus } from "@/platform/ipc";
import { Button, Checkbox, Input, T, copyText } from "@/ui";
import { Row } from "../components/Form";

/** `mcpServers` entry of an MCP client (Claude Code, Cursor…) over HTTP. */
function httpConfig(status: McpServerStatus): string | null {
  if (!status.running || !status.token) return null;
  return JSON.stringify(
    {
      mcpServers: {
        openidea: {
          type: "http",
          url: `http://127.0.0.1:${status.port}/mcp`,
          headers: { Authorization: `Bearer ${status.token}` },
        },
      },
    },
    null,
    2,
  );
}

/** `mcpServers` entry that starts the executable (`--mcp`) over stdio. */
function stdioConfig(status: McpServerStatus): string | null {
  // 旧二进制可能不带这个字段：缺了就只隐藏 stdio 配置，不让整页抛
  // `undefined is not iterable`。
  const [command, ...args] = status.stdioCommand ?? [];
  if (!command) return null;
  return JSON.stringify({ mcpServers: { openidea: { type: "stdio", command, args } } }, null, 2);
}

export function McpServerSettings() {
  const [status, setStatus] = useState<McpServerStatus | null>(null);
  const [port, setPort] = useState("");
  const [busy, setBusy] = useState(false);
  const [copied, setCopied] = useState<string | null>(null);
  // 后端不可达时（旧二进制 / 绑定没生成）不能白屏，把原因摆出来。
  const [loadError, setLoadError] = useState<string | null>(null);

  const apply = (next: Partial<Pick<McpServerStatus, "enabled" | "allowWrite">>, nextPort?: number) => {
    if (!status || busy) return;
    setBusy(true);
    // 必须走 call()：bindings 的命令返回 { status, data | error } 外壳，
    // 直接把外壳塞进 state 会让 status.stdioCommand / status.port 全是 undefined。
    void call(
      commands.mcpServerApply(
        next.enabled ?? status.enabled,
        nextPort ?? status.port,
        next.allowWrite ?? status.allowWrite,
      ),
    )
      .then(
        (fresh) => {
          setStatus(fresh);
          setPort(String(fresh.port));
          setLoadError(null);
        },
        (e: unknown) => setLoadError(String((e as { message?: string })?.message ?? e)),
      )
      .finally(() => setBusy(false));
  };

  useEffect(() => {
    if (!isTauri()) return;
    void call(commands.mcpServerStatus()).then(
      (fresh) => {
        setStatus(fresh);
        setPort(String(fresh.port));
      },
      (e: unknown) => setLoadError(String((e as { message?: string })?.message ?? e)),
    );
  }, []);

  useEffect(() => {
    if (!copied) return;
    const timer = setTimeout(() => setCopied(null), 2000);
    return () => clearTimeout(timer);
  }, [copied]);

  if (!status) {
    return (
      <Row
        label="Status"
        hint="MCP settings did not load. Open a project in this window first; otherwise see the error below."
      >
        <span role="alert">{loadError ?? "Loading…"}</span>
      </Row>
    );
  }
  const copy = (which: string, text: string | null) => {
    if (!text) return;
    void copyText(text).then(
      () => setCopied(which),
      () => undefined,
    );
  };
  const http = httpConfig(status);
  const stdio = stdioConfig(status);
  const state = status.running
    ? tx`Running on port ${status.port}`
    : status.error
      ? tx`Failed to start: ${status.error}`
      : loadError
        ? tx`Failed to apply: ${loadError}`
        : tx("Not running");
  return (
    <>
      <Row
        label="Enable MCP server"
        hint="HTTP only, on 127.0.0.1, with the token below. Agents see this window's project: its services and data sources."
      >
        <Checkbox
          checked={status.enabled}
          disabled={busy}
          onChange={(on) => apply({ enabled: on })}
          label={<T>Enable MCP server</T>}
        />
      </Row>
      <Row label="Port" hint="Needs a free port; a running server restarts on a new one.">
        <Input
          mono
          value={port}
          aria-label="Port"
          onChange={(e) => setPort(e.target.value.replace(/[^0-9]/g, ""))}
          onBlur={() => {
            const value = Number(port);
            if (value > 0 && value < 65536 && value !== status.port) apply({}, value);
            else setPort(String(status.port));
          }}
        />
      </Row>
      <Row label="Allow writing SQL" hint="Otherwise execute_query accepts read-only statements only.">
        <Checkbox
          checked={status.allowWrite}
          disabled={busy || !status.running}
          onChange={(on) => apply({ allowWrite: on })}
          label={<T>Allow writing SQL</T>}
        />
      </Row>
      <Row
        label="Status"
        hint={!status.project ? "Open a project in this window before enabling the MCP server." : undefined}
      >
        <span role="status">{state}</span>
      </Row>
      <Row label="Endpoint" hint={status.project ? undefined : "Open a project to get its endpoint."}>
        <Input
          mono
          readOnly
          value={status.running ? `http://127.0.0.1:${status.port}/mcp` : ""}
          placeholder={status.running ? "" : tx("Not running")}
          aria-label="Endpoint"
        />
      </Row>
      <Row
        label="Client configuration"
        stacked
        hint="HTTP works while the server runs; stdio starts a separate OpenIDEA process for the project instead (no server needed)."
      >
        <div>
          <Button size="small" disabled={!http} onClick={() => copy("http", http)}>
            <T>Copy HTTP Config</T>
          </Button>{" "}
          <Button size="small" disabled={!stdio} onClick={() => copy("stdio", stdio)}>
            <T>Copy Stdio Config</T>
          </Button>{" "}
          {copied && (
            <span role="status">
              <T>Copied to the clipboard</T>
            </span>
          )}
        </div>
      </Row>
    </>
  );
}
