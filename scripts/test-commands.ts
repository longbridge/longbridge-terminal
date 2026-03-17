#!/usr/bin/env bun
/**
 * Smoke-test all read-only CLI commands.
 *
 * Usage:
 *   bun scripts/test-commands.ts                                    # cargo run -q --
 *   LONGBRIDGE_BIN=./target/debug/longbridge bun scripts/test-commands.ts
 *
 * Mutating commands (buy, sell, cancel, replace, watchlist mutations)
 * are intentionally OMITTED — they must never run automatically.
 */

// ── Command fixture ───────────────────────────────────────────────────────────

interface Command {
  section: string;
  example: string;   // displayed label and default args (split by space)
  args?: string[];   // override test args
  dynamic?: string;  // key for runtime ID resolution
}

const COMMANDS: Command[] = [
  // ── Diagnostics ────────────────────────────────────────────────────────────
  { section: "Diagnostics", example: "check" },
  { section: "Diagnostics", example: "check --format json" },

  // ── Quote ──────────────────────────────────────────────────────────────────
  { section: "Quote", example: "quote TSLA.US 700.HK" },
  { section: "Quote", example: "depth TSLA.US" },
  { section: "Quote", example: "brokers 700.HK" },
  { section: "Quote", example: "trades TSLA.US --count 10" },
  { section: "Quote", example: "intraday TSLA.US" },
  { section: "Quote", example: "kline TSLA.US --period day --count 10" },
  { section: "Quote", example: "kline TSLA.US --period 1h --count 10" },
  { section: "Quote", example: "kline-history TSLA.US --start 2024-01-01 --end 2024-03-31" },
  { section: "Quote", example: "static TSLA.US 700.HK" },
  { section: "Quote", example: "calc-index TSLA.US --index pe,pb,eps" },
  { section: "Quote", example: "capital-flow TSLA.US" },
  { section: "Quote", example: "capital-dist TSLA.US" },
  { section: "Quote", example: "market-temp HK" },
  { section: "Quote", example: "market-temp US" },
  { section: "Quote", example: "trading-session" },
  { section: "Quote", example: "trading-days HK" },
  { section: "Quote", example: "security-list US" },
  { section: "Quote", example: "participants" },
  { section: "Quote", example: "subscriptions" },

  // ── Options & Warrants ─────────────────────────────────────────────────────
  { section: "Options & Warrants", example: "option-chain AAPL.US" },
  { section: "Options & Warrants", example: "option-chain AAPL.US --date <expiry>", dynamic: "option-chain-date" },
  { section: "Options & Warrants", example: "option-quote <symbol>", dynamic: "option-quote" },
  { section: "Options & Warrants", example: "warrant-list 700.HK" },
  { section: "Options & Warrants", example: "warrant-quote <symbol>", dynamic: "warrant-quote" },
  { section: "Options & Warrants", example: "warrant-issuers" },

  // ── News ───────────────────────────────────────────────────────────────────
  { section: "News", example: "news TSLA.US --count 5" },
  { section: "News", example: "news-detail <id>", dynamic: "news-detail" },
  { section: "News", example: "filings AAPL.US --count 5" },
  { section: "News", example: "topics TSLA.US --count 5" },
  { section: "News", example: "topic-detail <id>", dynamic: "topic-detail" },

  // ── Watchlist ──────────────────────────────────────────────────────────────
  { section: "Watchlist", example: "watchlist" },

  // ── Account ────────────────────────────────────────────────────────────────
  { section: "Account", example: "orders" },
  { section: "Account", example: "orders --history" },
  { section: "Account", example: "executions" },
  { section: "Account", example: "executions --history" },
  { section: "Account", example: "balance" },
  { section: "Account", example: "cash-flow" },
  { section: "Account", example: "positions" },
  { section: "Account", example: "fund-positions" },
  { section: "Account", example: "margin-ratio TSLA.US" },
  { section: "Account", example: "max-qty TSLA.US --side buy --price 200" },
];

// ── Binary invocation ─────────────────────────────────────────────────────────

const LONGBRIDGE_BIN = process.env.LONGBRIDGE_BIN;
const [bin, ...binPrefix] = LONGBRIDGE_BIN
  ? [LONGBRIDGE_BIN]
  : ["cargo", "run", "-q", "--"];

// ── Colors ────────────────────────────────────────────────────────────────────

const c = {
  green:  (s: string) => `\x1b[32m${s}\x1b[0m`,
  red:    (s: string) => `\x1b[31m${s}\x1b[0m`,
  yellow: (s: string) => `\x1b[33m${s}\x1b[0m`,
  dim:    (s: string) => `\x1b[2m${s}\x1b[0m`,
};

// ── Dynamic ID resolution ─────────────────────────────────────────────────────

const cache = new Map<string, string>();

async function runJson(args: string[]): Promise<unknown> {
  const proc = Bun.spawn([bin, ...binPrefix, ...args, "--format", "json"], {
    stdout: "pipe",
    stderr: "pipe",
  });
  await proc.exited;
  if (proc.exitCode !== 0) return null;
  try {
    return JSON.parse(await new Response(proc.stdout).text());
  } catch {
    return null;
  }
}

async function resolve(key: string): Promise<string> {
  if (cache.has(key)) return cache.get(key)!;

  let val = "";
  switch (key) {
    case "option-chain-date": {
      const data = await runJson(["option-chain", "AAPL.US"]);
      val = Array.isArray(data) ? (data[0]?.expiry_date ?? "") : "";
      break;
    }
    case "option-quote": {
      const expiry = await resolve("option-chain-date");
      if (expiry) {
        const data = await runJson(["option-chain", "AAPL.US", "--date", expiry]);
        val = Array.isArray(data)
          ? (data.find((r: any) => r.call_symbol)?.call_symbol ?? "")
          : "";
      }
      break;
    }
    case "warrant-quote": {
      const data = await runJson(["warrant-list", "700.HK"]);
      val = Array.isArray(data) ? (data[0]?.symbol ?? "") : "";
      break;
    }
    case "news-detail": {
      const data = await runJson(["news", "TSLA.US", "--count", "1"]);
      val = Array.isArray(data) ? String(data[0]?.id ?? "") : "";
      break;
    }
    case "topic-detail": {
      const data = await runJson(["topics", "TSLA.US", "--count", "1"]);
      val = Array.isArray(data) ? String(data[0]?.id ?? "") : "";
      break;
    }
  }

  cache.set(key, val);
  return val;
}

// ── Test runner ───────────────────────────────────────────────────────────────

let pass = 0, fail = 0, skip = 0;

async function run(label: string, args: string[]) {
  process.stdout.write(`  ${label.padEnd(54)}`);
  const start = Date.now();
  const proc = Bun.spawn([bin, ...binPrefix, ...args], { stdout: "pipe", stderr: "pipe" });
  await proc.exited;
  const elapsed = Date.now() - start;
  if (proc.exitCode === 0) {
    console.log(`${c.green("OK")}  ${c.dim(`${elapsed}ms`)}`);
    const out = await new Response(proc.stdout).text();
    const lines = out.split("\n").filter((l) => l.trim());
    const shown = lines.slice(0, 5);
    const hidden = lines.length - shown.length;
    shown.forEach((l) => console.log(`    ${c.dim(l)}`));
    if (hidden > 0) console.log(`    ${c.dim(`… ${hidden} more lines hidden`)}`);
    pass++;
  } else {
    const err = await new Response(proc.stderr).text();
    console.log(c.red("FAIL"));
    err.split("\n").slice(0, 3).forEach((l) => console.log(`    ${l}`));
    fail++;
  }
}

function skipCmd(label: string, reason: string) {
  console.log(`  ${label.padEnd(54)}${c.yellow("SKIP")}  ${c.dim(reason)}`);
  skip++;
}

// ── Main ──────────────────────────────────────────────────────────────────────

let currentSection = "";

for (const cmd of COMMANDS) {
  if (cmd.section !== currentSection) {
    if (currentSection) console.log();
    currentSection = cmd.section;
    console.log(currentSection);
  }

  let label = cmd.example;
  let args = cmd.args ?? cmd.example.split(" ");

  if (cmd.dynamic) {
    const val = await resolve(cmd.dynamic);
    if (!val) { skipCmd(label, `could not resolve ${cmd.dynamic}`); continue; }
    args = args.map((a) => (a.startsWith("<") ? val : a));
    label = label.replace(/<[^>]+>/, val);
  }

  await run(label, args);
}

console.log();
console.log("─".repeat(54));
const total = pass + fail + skip;
console.log(`  Total: ${total}   ${c.green(`Pass: ${pass}`)}   ${c.red(`Fail: ${fail}`)}   ${c.yellow(`Skip: ${skip}`)}`);

process.exit(fail > 0 ? 1 : 0);
