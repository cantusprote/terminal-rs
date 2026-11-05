#!/usr/bin/env node
// Dependency-free MCP stdio adapter over the Terminal-rs native CLI.
// Transport is local only, tools are allowlisted, shell execution is forbidden.
import { spawn } from 'node:child_process';
import { existsSync, readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import readline from 'node:readline';

const root = dirname(fileURLToPath(import.meta.url));
const project = dirname(root);
const manifest = JSON.parse(readFileSync(join(root, 'addon.json'), 'utf8'));
const tools = manifest.tools.map(t => ({
  name: t.name,
  description: t.description,
  inputSchema: JSON.parse(readFileSync(join(root, t.schema), 'utf8'))
}));
const byName = new Map(tools.map(t => [t.name, t]));
const binary = process.env.TERMINAL_CLI_BIN || [
  join(project, 'target/release/terminal-cli'),
  join(project, 'target/debug/terminal-cli')
].find(existsSync);
const children = new Set();
const maxBytes = 240000;
const toolsArgs = {
  terminal_status: async () => {
    const doctor = await invoke(['doctor']);
    if (doctor.data?.app_running) {
      const status = await invoke(['app', 'status']);
      return { ...doctor.data, app: status.data };
    }
    return doctor.data;
  },
  terminal_quote: x => invoke(['quote', ...x.symbols]),
  terminal_history: x => invoke(['history', x.symbol, x.timeframe || 'D1', String(x.limit || 50)]),
  terminal_search: x => invoke(['search', x.query]),
  terminal_book: x => invoke(['book', x.symbol, String(x.depth || 20)]),
  terminal_watchlist: x => invoke(['app', ({list:'watchlist', add:'add', remove:'remove'})[x.operation],
    ...(x.operation === 'list' ? [] : [requireSymbol(x)])]),
  terminal_screen: x => invoke(['app', 'screen', x.expression]),
  terminal_alert: x => invoke(['app', x.kind === 'price' ? 'alert' : 'technical-alert', x.expression]),
  terminal_open_chart: x => invoke(['app', 'chart', x.symbol]),
  terminal_app_quotes: x => invoke(['app', 'quotes', ...x.symbols])
};
function requireSymbol(x) {
  if (!x.symbol) throw new Error('symbol required for watchlist add/remove');
  return x.symbol;
}
function validate(tool, args) {
  if (!args || typeof args !== 'object' || Array.isArray(args)) throw new Error('arguments must be an object');
  const schema = tool.inputSchema;
  for (const key of schema.required || []) if (!(key in args)) throw new Error('missing: ' + key);
  for (const [key, value] of Object.entries(args)) {
    const property = schema.properties[key];
    if (!property) throw new Error('unknown argument: ' + key);
    if (property.type === 'string') {
      if (typeof value !== 'string' || value.length < (property.minLength || 0) ||
          value.length > (property.maxLength || 1e6)) throw new Error('invalid ' + key);
    } else if (property.type === 'integer') {
      if (!Number.isInteger(value) || value < property.minimum || value > property.maximum)
        throw new Error('invalid ' + key);
    } else if (property.type === 'array') {
      if (!Array.isArray(value) || value.length < property.minItems || value.length > property.maxItems ||
          value.some(v => typeof v !== 'string' || !v.length || v.length > 96))
        throw new Error('invalid ' + key);
    }
    if (property.enum && !property.enum.includes(value)) throw new Error('invalid ' + key);
  }
}
async function invoke(argv) {
  if (!binary || !existsSync(binary)) {
    throw new Error('terminal-cli not built; run cargo build --release -p terminal');
  }
  return new Promise((resolve, reject) => {
    const child = spawn(binary, argv, {
      cwd: project,
      env: { ...process.env, TR_APPLE_STOCKS_CACHE: process.env.TR_APPLE_STOCKS_CACHE || '1' },
      stdio: ['ignore','pipe','pipe']
    });
    children.add(child);
    let output = '', errors = '', ended = false;
    const done = (error, result) => {
      if (ended) return;
      ended = true;
      clearTimeout(timer);
      children.delete(child);
      if (error) reject(error); else resolve(result);
    };
    const timer = setTimeout(() => { child.kill('SIGKILL'); done(new Error('terminal-cli timeout')); }, 48000);
    child.stdout.on('data', data => {
      output += data.toString();
      if (output.length > maxBytes) { child.kill('SIGKILL'); done(new Error('terminal-cli response too large')); }
    });
    child.stderr.on('data', data => { errors = (errors + data.toString()).slice(-1600); });
    child.on('error', error => done(error));
    child.on('close', code => {
      let result;
      try { result = JSON.parse(output); }
      catch { return done(new Error('invalid terminal-cli JSON: ' + (errors || output).slice(-900))); }
      if (code !== 0 || !result.ok) done(new Error(result.error || errors || 'CLI exited ' + code));
      else done(null, result);
    });
  });
}

function send(message) {
  process.stdout.write(JSON.stringify(message) + '\n');
}
const respond = (id, result) => send({ jsonrpc: '2.0', id, result });
const fail = (id, error) => send({ jsonrpc: '2.0', id, error: { code: -32603, message: String(error?.message || error) } });
async function onMessage(m) {
  if (!m || m.jsonrpc !== '2.0' || typeof m.method !== 'string') return;
  if (m.id === undefined) return; // notification
  try {
    if (m.method === 'initialize') {
      respond(m.id, {
        protocolVersion: m.params?.protocolVersion || '2025-06-18',
        capabilities: { tools: { listChanged: false } },
        serverInfo: { name: 'terminal-rs-personal', version: manifest.version }
      });
    } else if (m.method === 'ping') {
      respond(m.id, {});
    } else if (m.method === 'tools/list') {
      respond(m.id, { tools });
    } else if (m.method === 'tools/call') {
      const tool = byName.get(m.params?.name);
      if (!tool) throw new Error('unknown tool');
      const args = m.params.arguments || {};
      validate(tool, args);
      try {
        const result = await toolsArgs[tool.name](args);
        respond(m.id, { content: [{ type: 'text', text: JSON.stringify(result) }] });
      } catch (error) {
        respond(m.id, { isError: true, content: [{ type: 'text', text: JSON.stringify({
          ok: false, error: String(error?.message || error)
        }) }] });
      }
    } else {
      fail(m.id, 'unsupported MCP request: ' + m.method);
    }
  } catch (error) {
    fail(m.id, error);
  }
}

const input = readline.createInterface({ input: process.stdin, crlfDelay: Infinity });
input.on('line', line => {
  try { void onMessage(JSON.parse(line)); }
  catch (error) { process.stderr.write('invalid MCP request: ' + String(error) + '\n'); }
});
function stop() {
  for (const p of children) p.kill('SIGTERM');
  process.exit(0);
}
process.on('SIGTERM', stop);
process.stdin.on('end', stop);
