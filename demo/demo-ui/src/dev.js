// Minimal dev server — mimics Vite's startup output without Vite itself.
const http = require('http');

const port = 5173;
const apiUrl = process.env.VITE_API_URL || 'http://localhost:3000';

console.log(`\n  VITE v5.4.0  ready in 312 ms\n`);
console.log(`  ➜  Local:   http://localhost:${port}/`);
console.log(`  ➜  Network: use --host to expose`);
console.log(`  ➜  API:     ${apiUrl}\n`);

const server = http.createServer((req, res) => {
  res.writeHead(200, { 'Content-Type': 'text/html' });
  res.end(`<!doctype html><html><body><h1>demo-ui</h1><p>API: ${apiUrl}</p></body></html>`);
});

server.on('error', (err) => {
  if (err.code === 'EADDRINUSE') {
    console.error(`  Port ${port} already in use`);
  }
});

server.listen(port, '127.0.0.1');
