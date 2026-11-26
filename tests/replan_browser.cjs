const {chromium} = require(process.env.PLAYWRIGHT_CORE || "playwright-core");
const fs = require("node:fs");
const assert = require("node:assert/strict");

(async () => {
  const browser = await chromium.launch({channel:"chrome", headless:true});
  try {
    const page = await browser.newPage();
    const report = {confirmations:[],files:{}};
    const save = () => fs.writeFileSync(process.argv[3],JSON.stringify(report));
    page.on("response",r=>{
      if (r.ok() && r.url().endsWith("/api/decide")) {
        report.confirmations.push(r.request().postDataJSON());
        save();
      }
    });
    await page.goto(fs.readFileSync(0, "utf8").trim());
    await page.locator("#intent button").click({timeout:180000});
    const choice = page.locator("#timeline").getByLabel("choice");
    await choice.waitFor({timeout:240000});
    await page.locator("#instruction").fill("保留原文，把两种意见整理成给作者的说明。");
    await page.locator("#instruct button").click();
    await choice.waitFor({state:"detached",timeout:240000});
    const deadline = Date.now()+180000;
    while (Date.now()<deadline) {
      const req = page.locator("#permissions article").first();
      if (await req.count()) {
        const deny = process.argv[2]==="deny" && (await req.innerText()).includes("publish");
        await req.getByRole("button",{name:deny?"拒绝":"允许一次",exact:true}).click();
        if (deny) {
          await req.waitFor({state:"detached"});
          return;
        }
      }
      if (await page.locator("#timeline a").count()) {
        assert.equal(process.argv[2],"allow");
        for(const link of await page.locator("#timeline a").all()) {
          const name=await link.getAttribute("download");
          const url=await link.getAttribute("href");
          report.files[name]=require("node:crypto").createHash("sha256")
            .update(Buffer.from(url.split(",")[1],"base64")).digest("hex");
        }
        save();
        return;
      }
      await page.waitForTimeout(400);
    }
    throw Error("Timed out waiting for the revised delivery");
  } finally { await browser.close(); }
})().catch(e=>{console.error(e);process.exitCode=1;});
