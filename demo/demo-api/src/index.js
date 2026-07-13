const http = require('http');

const port = process.env.PORT || 3000;
const env = process.env.NODE_ENV || 'development';

const routes = {
  '/health': { status: 'ok', env },
  '/items': [{ id: 1, name: 'Widget A' }, { id: 2, name: 'Widget B' }],
};

console.log(`\n  demo-api  v0.1.0  ready\n`);
console.log(`  ➜  Local:   http://localhost:${port}/`);
console.log(`  ➜  Mode:    ${env}\n`);

const server = http.createServer((req, res) => {
  const body = routes[req.url];
  if (body) {
    res.writeHead(200, { 'Content-Type': 'application/json' });
    res.end(JSON.stringify(body));
  } else {
    res.writeHead(404);
    res.end('Not found');
  }
});

server.on('error', (err) => {
  if (err.code === 'EADDRINUSE') {
    console.error(`  Port ${port} already in use`);
  }
});

server.listen(port, '127.0.0.1');
