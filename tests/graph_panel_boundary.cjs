// Exercise the actual Studio response headers and bootstrap with synthetic API data.
const assert = require("node:assert/strict");
const http = require("node:http");
const {spawn} = require("node:child_process");
const {once} = require("node:events");
const {createInterface} = require("node:readline");
const {chromium} = require(process.env.PLAYWRIGHT_CORE || "playwright-core");

async function main() {
  let requests = 0;
  const sink = http.createServer((_req, res) => { requests++; res.end("test"); });
  await new Promise(resolve => sink.listen(0, "127.0.0.1", resolve));
  const server = spawn("python3", ["-u", "-m", "orchestrator", "--task", "fixture", "--port", "0"],
    {cwd: "studio", stdio: ["ignore","pipe","inherit"]});
  let browser;
  try {
    const [line] = await once(createInterface({input:server.stdout}), "line");
    const browserUrl = line.slice("macro UI: ".length);
    browser = await chromium.launch({channel:"chrome",headless:true});
    const page = await browser.newPage();
    const sequence = [];
    const view = {task:"fixture", nodes:{n:{id:"n"}}, activity:{n:{id:"attempt",status:"waiting"}},
      authorized:true, permissions:[], deliveries:[]};
    const url = 'http://127.0.0.1:'+sink.address().port+'/fixed-test-page';
    await page.route("**/api/*", async route => {
      const action = new URL(route.request().url()).pathname.slice(5);
      sequence.push(action);
      const pkg = {html:'<a id="link" href="'+url+'">Open test page</a><script>addEventListener("an-ready",()=>an.respond({ok:true}))</script>',
        files:{}, assets:{}, draft:null};
      await route.fulfill({json:action==="view"?view:action==="panel"?pkg:{status:"waiting"}});
    });
    await Promise.all([page.waitForResponse(r=>r.url().endsWith("/api/respond")), page.goto(browserUrl)]);
    assert(sequence.indexOf("viewed") >= 0 && sequence.indexOf("viewed") < sequence.indexOf("respond"));
    const violation = page.waitForEvent("console", {predicate:m=>m.text().includes("frame-src")});
    await page.frameLocator("iframe").locator("#link").click();
    await violation;
    assert.equal(requests, 0);
    console.log("Ready response ordered; fixed test navigation blocked before network access");
  } finally {
    await browser?.close();
    server.kill();
    sink.close();
  }
}
main().catch(e=>{console.error(e);process.exitCode=1;});
