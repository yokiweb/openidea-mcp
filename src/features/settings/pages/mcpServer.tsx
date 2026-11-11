/** IDE MCP Server settings: upstream page id/group + local persisted configuration. */
import type { McpServerStatus } from "@/platform/ipc";
import type { PageSpec } from "./spec";
import { McpServerSettings } from "./mcp";

/** `mcpServers` entry of an MCP client configuration (Claude Code, Cursor…). */
export function httpConfig(status: McpServerStatus): string | null {
  if (!status.url || !status.token) return null;
  const server = { type: "http", url: status.url, headers: { Authorization: `Bearer ${status.token}` } };
  return JSON.stringify({ mcpServers: { openidea: server } }, null, 2);
}

/** The same through the stdio bridge: no token, works for as long as OpenIDEA runs. */
export function stdioConfig(status: McpServerStatus): string | null {
  const [command, ...args] = status.stdioCommand ?? [];
  if (!command) return null;
  return JSON.stringify({ mcpServers: { openidea: { type: "stdio", command, args } } }, null, 2);
}

/** 上游页签元数据 + 本地 `mcp.json` 配置控件（固定端口 / 稳定 token / allowWrite）。 */
export const MCP_SERVER_PAGE: PageSpec = {
  id: "agents.mcpServer",
  title: "IDE MCP Server",
  group: "Agents",
  order: 470,
  keywords: [
    "mcp",
    "model context protocol",
    "ai",
    "agent",
    "claude",
    "codex",
    "tools",
    "stdio",
    "http",
    "port",
    "token",
  ],
  sections: [
    {
      options: [
        {
          kind: "custom",
          label: "Enable MCP server",
          keywords: ["Port", "Status", "Endpoint", "Client configuration", "Copy HTTP Config", "Copy Stdio Config"],
          component: McpServerSettings,
        },
      ],
    },
  ],
};
