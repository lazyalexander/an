const {chromium} = require(process.env.PLAYWRIGHT_CORE || "playwright-core");
const fs = require("node:fs");
const assert = require("node:assert/strict");
(async () => {
  const browser = await chromium.launch({channel:"chrome", headless:true});
  try {
    const [choice, directory] = process.argv.slice(2);
    const page = await browser.newPage();
    await page.goto(fs.readFileSync(0,"utf8").trim());
    const timeline = page.locator("#timeline");
    await timeline.getByLabel("choice").waitFor();
    await page.screenshot({path:directory+"/desktop.png"});
    await page.setViewportSize({width:390,height:844});
    await page.reload();
    await timeline.getByLabel("choice").selectOption(JSON.stringify(choice));
    assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth <= innerWidth));
    await page.screenshot({path:directory+"/mobile.png"});
    await timeline.getByRole("button",{name:"提交决定"}).click();
    await timeline.getByRole("link",{name:/revised.md/}).waitFor();
    await page.reload();
    await timeline.getByRole("link",{name:/changes.diff/}).waitFor();
  } finally { await browser.close(); }
})().catch(e=>{console.error(e);process.exitCode=1;});
