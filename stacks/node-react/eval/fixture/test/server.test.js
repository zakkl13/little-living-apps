const test = require("node:test");
const assert = require("node:assert/strict");
const server = require("../server.js");

async function withServer(fn) {
  await new Promise((resolve) => server.listen(0, resolve));
  const base = `http://127.0.0.1:${server.address().port}`;
  try {
    await fn(base);
  } finally {
    await new Promise((resolve) => server.close(resolve));
  }
}

test("root serves the React page", async () => {
  await withServer(async (base) => {
    const res = await fetch(`${base}/`);
    assert.equal(res.status, 200);
    const html = await res.text();
    assert.match(html, /id="root"/);
    assert.match(html, /Lilapp is running/);
  });
});

test("greet greets by name", async () => {
  await withServer(async (base) => {
    const res = await fetch(`${base}/greet?name=Zakk`);
    assert.equal(res.status, 200);
    assert.match(await res.text(), /Hello, Zakk!/);
  });
});
