// Optional live-browser leg of graph_studio_smoke.py, using the installed Chrome.
const fs = require("node:fs");
const assert = require("node:assert/strict");
const {chromium} = require(process.env.PLAYWRIGHT_CORE || "playwright-core");
const [mode, experiment, directory] = process.argv.slice(2);
(async()=>{
  const browser = await chromium.launch({channel:"chrome",headless:true});
  try{
    const page=await browser.newPage();
    await page.goto(fs.readFileSync(0,"utf8").trim());
    const panel=page.frameLocator("iframe");
    const bridge=experiment==="bridge";
    if(mode==="respond"){
      const draftSaved=page.waitForResponse(r=>r.url().endsWith("/api/draft") && r.ok() && (bridge?r.request().postDataJSON().value.note==="edge":r.request().postDataJSON().value.seconds===2));
      if(bridge){
        await panel.locator("#figB img").click();
        await panel.locator("#note").fill("edge");
      }else{
        await panel.locator("#speed").focus();
        await panel.locator("#speed").press("Home");
        await panel.locator("#speed").press("ArrowRight");
      }
      await draftSaved;
      await page.reload();
      if(bridge){
        assert.equal(await panel.locator("#note").inputValue(),"edge");
        assert.equal(await panel.locator("#B").isChecked(),true);
      }else assert.equal(await panel.locator("#speed").inputValue(),"2");
      await page.screenshot({path:directory+"/panel.png"});
      await page.setViewportSize({width:390,height:844});
      await page.screenshot({path:directory+"/mobile.png"});
      await panel.locator(bridge?"#send":"#apply").click();
      await page.getByText(bridge?"等待文档保存":"已完成",{exact:true}).first().waitFor();
    }else{
      if(bridge){
        assert.match(await panel.locator("#note").innerText(),/edge/);
        const annotated=await panel.locator("#picture").getAttribute("src");
        await panel.locator("#original").click();
        assert.notEqual(await panel.locator("#picture").getAttribute("src"),annotated);
        await panel.locator("#annotated").click();
        assert.equal(await panel.locator("#picture").getAttribute("src"),annotated);
      }
      else await page.getByRole("link",{name:/turntable.svg/}).waitFor();
      await page.locator("#sources summary").first().click();
      await page.locator("#trace summary").first().waitFor();
      await page.screenshot({path:directory+"/delivery.png"});
    }
  }finally{await browser.close();}
})().catch(error=>{console.error(error);process.exitCode=1;});
