// Dev-only: lets the settings UI run in a normal browser against real agents,
// for browser QA (`make ui-lab`). Never part of a build (`apply: "serve"`) and
// off unless GLIDEDESK_DEV_AGENTS names the agents' sockets:
//   GLIDEDESK_DEV_AGENTS="server=/lab/server/agent.sock,client=/lab/client/agent.sock"
// Routes: POST /__gd/<agent>/request (one IPC request) and
//         GET  /__gd/<agent>/events  (Server-Sent Events of the agent's events).
import { createConnection } from "node:net";
import type { IncomingMessage, ServerResponse } from "node:http";
import type { Plugin } from "vite";

type Agents = Record<string, string>;

function agents(): Agents {
  const out: Agents = {};
  for (const part of (process.env.GLIDEDESK_DEV_AGENTS ?? "").split(",")) {
    const [name, path] = part.split("=");
    if (name && path) out[name.trim()] = path.trim();
  }
  return out;
}

/** Sends one request line and resolves with the matching response. */
function request(path: string, req: Record<string, unknown>): Promise<{ ok: boolean; data?: unknown; error?: string }> {
  return new Promise((resolve, reject) => {
    const sock = createConnection(path);
    let buf = "";
    const timer = setTimeout(() => {
      sock.destroy();
      reject(new Error("agent did not answer"));
    }, 30_000);
    sock.on("connect", () => sock.write(JSON.stringify({ seq: 1, ...req }) + "\n"));
    sock.on("data", (d) => {
      buf += d.toString("utf8");
      let nl: number;
      while ((nl = buf.indexOf("\n")) >= 0) {
        const line = buf.slice(0, nl);
        buf = buf.slice(nl + 1);
        const msg = JSON.parse(line) as { type: string; id?: number; ok?: boolean; data?: unknown; error?: string };
        if (msg.type === "response" && msg.id === 1) {
          clearTimeout(timer);
          sock.end();
          resolve({ ok: !!msg.ok, data: msg.data, error: msg.error });
        }
      }
    });
    sock.on("error", (e) => {
      clearTimeout(timer);
      reject(e);
    });
  });
}

function body(req: IncomingMessage): Promise<string> {
  return new Promise((resolve, reject) => {
    let s = "";
    req.on("data", (c) => (s += c));
    req.on("end", () => resolve(s));
    req.on("error", reject);
  });
}

function events(path: string, res: ServerResponse) {
  res.writeHead(200, { "content-type": "text/event-stream", "cache-control": "no-cache", connection: "keep-alive" });
  const sock = createConnection(path);
  let buf = "";
  sock.on("connect", () => sock.write(JSON.stringify({ seq: 1, cmd: "subscribe" }) + "\n"));
  sock.on("data", (d) => {
    buf += d.toString("utf8");
    let nl: number;
    while ((nl = buf.indexOf("\n")) >= 0) {
      const line = buf.slice(0, nl);
      buf = buf.slice(nl + 1);
      if (line.includes('"type":"event"')) res.write(`data: ${line}\n\n`);
    }
  });
  const end = () => {
    sock.destroy();
    res.end();
  };
  sock.on("error", end);
  sock.on("close", end);
  res.on("close", () => sock.destroy());
}

export function agentBridge(): Plugin {
  return {
    name: "glidedesk-agent-bridge",
    apply: "serve",
    configureServer(server) {
      const known = agents();
      if (Object.keys(known).length === 0) return;
      server.middlewares.use("/__gd/agents", (_req, res) => {
        res.setHeader("content-type", "application/json");
        res.end(JSON.stringify(Object.keys(known)));
      });
      server.middlewares.use(async (req, res, next) => {
        const m = /^\/__gd\/([\w-]+)\/(request|events)$/.exec(req.url ?? "");
        const path = m && known[m[1]];
        if (!m || !path) return next();
        if (m[2] === "events") return events(path, res);
        try {
          const out = await request(path, JSON.parse(await body(req)) as Record<string, unknown>);
          res.setHeader("content-type", "application/json");
          res.end(JSON.stringify(out));
        } catch (e) {
          res.statusCode = 502;
          res.end(JSON.stringify({ ok: false, error: String(e) }));
        }
      });
    },
  };
}
