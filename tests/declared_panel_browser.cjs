// Delay host acknowledgements to exercise panel switching in a real browser.
const assert = require("node:assert/strict");
const path = require("node:path");
const {chromium} = require(process.env.PLAYWRIGHT_CORE || "playwright-core");
const root = path.join(__dirname, "../studio/orchestrator/macro/graph");
function gate() {
  let release;
  const wait = new Promise(resolve => { release = resolve; });
  return {wait,release};
}
(async()=>{
  const browser = await chromium.launch({channel:"chrome",headless:true});
  try {
    const page = await browser.newPage();
    const viewed = gate(), firstWrite = gate();
    let draft, requests = 0, response, evidenceDecision;
    const card = {id:"card",status:"pending",routes:[{operation:"preview",source:{input:"source"},origin:"task-input:source",destination:"studio"}]};
    const covered = {id:"covered",intent:card.id,operation:"preview",scope:["source"],details:[]};
    const evidence = {id:"evidence",operation:"preview",scope:["evidence"],details:[]};
    const pkg = {files:{source:{
      "notes.unknown":Buffer.from("<em>literal preview</em>").toString("base64"),
      "binary.md":Buffer.from([0xff,0x00]).toString("base64")
    }},assets:{},declaration:{decision:{}},response:{properties:{append:{type:"string"}}},
      html:'<input id="custom"><script>addEventListener("an-ready",()=>{custom.value=an.draft.append;custom.oninput=()=>an.saveDraft({append:custom.value})})</script>'};
    await page.route("http://studio.test/**",async route=>{
      const name = new URL(route.request().url()).pathname.split("/").pop();
      let json = {};
      switch(name) {
        case "config.json": json={settings:{poll_ms:60000,panel_timeout_ms:5000},strings:{}};break;
        case "view": json={task:"task",nodes:{ask:{}},activity:{ask:{id:"current",status:"waiting"}},intent:card,permissions:evidenceDecision ? [covered] : [covered,evidence],authorized:true};break;
        case "decide": evidenceDecision=route.request().postDataJSON();break;
        case "panel": requests++;json={...pkg,draft};break;
        case "viewed": await viewed.wait;break;
        case "draft": await firstWrite.wait;draft=route.request().postDataJSON().value;break;
        case "respond": response=route.request().postDataJSON().value;break;
        default: return route.fulfill({path:path.join(root,name || "index.html")});
      }
      await route.fulfill({json});
    });
    const ack = page.waitForRequest("**/api/viewed");
    await page.goto("http://studio.test/");
    await ack;
    const field = page.getByLabel("append"), submit = page.getByRole("button",{name:"panel.submit"});
    assert.deepEqual(await page.locator("#content pre").allTextContents(), ["<em>literal preview</em>"]);
    assert.equal(await page.locator("#content em").count(), 0);
    assert(await field.isVisible());
    assert(await submit.isDisabled());
    viewed.release();
    await field.fill("first");
    await field.fill("latest");
    await page.getByRole("button",{name:"panel.custom"}).click();
    assert.equal(requests,1);
    firstWrite.release();
    const custom = page.frameLocator("iframe").locator("#custom");
    await custom.waitFor();
    assert.equal(await custom.inputValue(),"latest");
    await custom.fill("reverse");
    await page.getByRole("button",{name:"panel.basic"}).click();
    await field.waitFor();
    assert.equal(await field.inputValue(),"reverse");
    await submit.click();
    assert.deepEqual(response,{append:"reverse"});
    const individual = page.locator("#permissions article");
    assert.equal(await individual.count(),1);
    await individual.getByRole("button",{name:"permission.allow_once"}).click();
    await individual.waitFor({state:"detached"});
    assert.deepEqual(evidenceDecision,{request:"evidence",decision:"once"});
    assert(await page.locator("#intent button").isVisible());
  } finally { await browser.close(); }
})().catch(e=>{console.error(e);process.exitCode=1;});
