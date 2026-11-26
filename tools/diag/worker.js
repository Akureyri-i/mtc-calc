// The statistics receiver: a Cloudflare Worker with a D1 database, served on its own domain
// (workers.dev is not reachable from mainland China) at https://stats.mtc-calc.dev/.
//
// POST / with newline-delimited JSON records ("kind": "shot" or "run"). Each line is checked for
// its shape and size; shots are stored one row each, runs replace their earlier copy (a run is
// posted every 10 minutes and at exit). Stored: the record and its day only. Not stored: the IP
// (used in memory to limit the rate), the arrival time, which lines came in one request.
// A daily cron deletes rows older than 12 months.
//
//   wrangler d1 execute mtc-stats --file tools/diag/schema.sql
//   wrangler deploy tools/diag/worker.js   (binding: DB = mtc-stats; cron: daily)

const MAX_BODY = 256 * 1024;
const MAX_LINE = 16 * 1024;
const MAX_LINES = 500;
const PER_MINUTE = 20;
const KEEP_DAYS = 365;

const seen = new Map(); // ip -> [minute, count], this isolate only

function limited(ip) {
  const minute = Math.floor(Date.now() / 60000);
  const [m, n] = seen.get(ip) || [minute, 0];
  const count = m === minute ? n + 1 : 1;
  seen.set(ip, [minute, count]);
  if (seen.size > 10000) seen.clear();
  return count > PER_MINUTE;
}

function valid(record) {
  if (typeof record !== "object" || record === null || Array.isArray(record)) return false;
  if (record.kind !== "shot" && record.kind !== "run") return false;
  if (!Number.isInteger(record.v) || record.v < 1 || record.v > 1000) return false;
  if (typeof record.run !== "string" || !/^[0-9a-f]{16}$/.test(record.run)) return false;
  if (typeof record.day !== "string" || !/^\d{4}-\d{2}-\d{2}$/.test(record.day)) return false;
  return true;
}

export default {
  async fetch(request, env) {
    if (request.method !== "POST") return new Response("POST statistics here", { status: 405 });
    const ip = request.headers.get("CF-Connecting-IP") || "";
    if (limited(ip)) return new Response("too many", { status: 429 });
    const body = await request.text();
    if (body.length > MAX_BODY) return new Response("too large", { status: 413 });
    const lines = body.split("\n").filter((l) => l.trim().length > 0).slice(0, MAX_LINES);
    const statements = [];
    for (const line of lines) {
      if (line.length > MAX_LINE) continue;
      let record;
      try {
        record = JSON.parse(line);
      } catch {
        continue;
      }
      if (!valid(record)) continue;
      if (record.kind === "shot") {
        statements.push(env.DB.prepare("INSERT INTO shots (day, v, record) VALUES (?, ?, ?)").bind(record.day, record.v, line));
      } else {
        statements.push(env.DB.prepare("INSERT INTO runs (run, day, v, record) VALUES (?, ?, ?, ?) ON CONFLICT(run) DO UPDATE SET record = excluded.record").bind(record.run, record.day, record.v, line));
      }
    }
    if (statements.length > 0) await env.DB.batch(statements);
    return new Response(null, { status: 204 });
  },

  async scheduled(event, env) {
    const cutoff = new Date(Date.now() - KEEP_DAYS * 86400000).toISOString().slice(0, 10);
    await env.DB.batch([env.DB.prepare("DELETE FROM shots WHERE day < ?").bind(cutoff), env.DB.prepare("DELETE FROM runs WHERE day < ?").bind(cutoff)]);
  },
};
