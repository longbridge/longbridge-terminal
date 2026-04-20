/**
 * Longbridge API mock server (Bun)
 *
 * Accepts all HTTP and WebSocket requests, logs headers, and returns minimal
 * valid responses so the SDK doesn't crash on startup.
 *
 * Usage:
 *   bun scripts/mock-server.ts
 *
 * Then point the CLI at it:
 *   LONGBRIDGE_HTTP_URL=http://localhost:8080 \
 *   LONGBRIDGE_QUOTE_WS_URL=ws://localhost:8080/v2 \
 *   LONGBRIDGE_TRADE_WS_URL=ws://localhost:8080/v2 \
 *   longbridge <command>
 */

const PORT = parseInt(process.env.PORT ?? "8081");

// ANSI colours
const R = "\x1b[0m";
const BOLD = "\x1b[1m";
const DIM = "\x1b[2m";
const GREEN = "\x1b[32m";
const YELLOW = "\x1b[33m";
const MAGENTA = "\x1b[35m";
const CYAN = "\x1b[36m";

// Headers worth highlighting
const INTERESTING = new Set([
  "x-cli-cmd",
  "x-channel-id",
  "user-agent",
  "accept-language",
  "authorization",
  "upgrade",
  "connection",
]);

function printHeaders(headers: Headers) {
  for (const [key, value] of headers.entries()) {
    const hi = INTERESTING.has(key.toLowerCase());
    const display = key.toLowerCase() === "authorization"
      ? value.slice(0, 20) + "…"
      : value;
    console.log(
      `  ${hi ? YELLOW + BOLD : DIM}${key}${R}: ${hi ? BOLD : DIM}${display}${R}`
    );
  }
}

const server = Bun.serve({
  port: PORT,

  fetch(req, server) {
    const url = new URL(req.url);
    const isUpgrade = req.headers.get("upgrade")?.toLowerCase() === "websocket";

    if (isUpgrade) {
      console.log(`\n${MAGENTA}${BOLD}▶ WS UPGRADE${R}  ${CYAN}${url.pathname}${R}`);
      printHeaders(req.headers);
      const ok = server.upgrade(req);
      if (!ok) {
        return new Response("WebSocket upgrade failed", { status: 400 });
      }
      return; // upgraded
    }

    console.log(`\n${GREEN}${BOLD}▶ HTTP ${req.method}${R}  ${CYAN}${url.pathname}${R}`);
    printHeaders(req.headers);

    return new Response(JSON.stringify({ code: 0, message: "", data: {} }), {
      headers: { "content-type": "application/json" },
    });
  },

  websocket: {
    open(ws) {
      console.log(`  ${DIM}↑ WS open${R}`);
    },
    message(_ws, _msg) {
      // Silently drop protobuf frames — we only care about upgrade headers
    },
    close(_ws, code) {
      console.log(`  ${DIM}↓ WS closed (${code})${R}`);
    },
  },
});

console.log(`${BOLD}Longbridge mock server${R}  →  http://localhost:${PORT}`);
console.log();
console.log(`${DIM}Run CLI with:${R}`);
console.log(
  `  ${YELLOW}LONGBRIDGE_HTTP_URL${R}=http://localhost:${PORT} \\`
);
console.log(
  `  ${YELLOW}LONGBRIDGE_QUOTE_WS_URL${R}=ws://localhost:${PORT}/v2 \\`
);
console.log(
  `  ${YELLOW}LONGBRIDGE_TRADE_WS_URL${R}=ws://localhost:${PORT}/v2 \\`
);
console.log(`  longbridge <command>`);
console.log();
