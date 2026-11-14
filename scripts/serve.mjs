// Serves src/ for UI work in a normal browser (the mock backend kicks in there).
// Usage: node scripts/serve.mjs [port]
import { createServer } from 'node:http';
import { readFile } from 'node:fs/promises';
import { extname, join, normalize } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../src/', import.meta.url));
const port = Number(process.argv[2]) || 8000;
const types = { '.html': 'text/html', '.css': 'text/css', '.js': 'text/javascript', '.svg': 'image/svg+xml', '.png': 'image/png' };

createServer(async (req, res) => {
  const path = normalize(decodeURIComponent(new URL(req.url, 'http://x').pathname)).replace(/^([\\/]\.\.)+/, '');
  try {
    const body = await readFile(join(root, path.endsWith('/') || path === '\\' ? 'index.html' : path));
    res.writeHead(200, { 'content-type': types[extname(path)] || 'text/html' });
    res.end(body);
  } catch {
    res.writeHead(404).end('not found');
  }
}).listen(port, '127.0.0.1', () => console.log(`UI preview on http://localhost:${port}`));
