# An

在 Krita 里画。背景进程按你的一句话，把新笔触收成彼此分开的 `an/` 图层。框住一块再改时，它先认出那是哪一层，只重画那一层。

后端三选一：内置的 `code`、`dsh headless`、`codex exec`。失败就停，不会偷偷换一个。模型只交一份图层计划（名字、颜色、对应哪一块笔触）。像素由这个程序自己填。

## 启动

在 `An/` 目录先启动代理，再开 Krita：

```sh
cargo run -- serve
```

它只听 `127.0.0.1`，把端口写进 `state/port`。根目录可以改成 `cargo run -- serve /别的/目录`。

## 安装插件

把插件目录和 desktop 文件链到 Krita 的 pykrita。不要复制，链过去之后端口文件才还在 `An/state/port`。

```sh
ln -s /Users/alexandermichael/Desktop/an-agent/An/krita/an_paint \
  "$HOME/Library/Application Support/krita/pykrita/an_paint"
ln -s /Users/alexandermichael/Desktop/an-agent/An/krita/an_paint.desktop \
  "$HOME/Library/Application Support/krita/pykrita/an_paint.desktop"
```

重启 Krita，在设置里启用 An Paint。面板在右侧，标题是 An。

文档要有一层名叫 `strokes` 的绘图层，色彩是 8 位 RGBA。`strokes` 一直留着，代理只替换名字以 `an/` 开头的图层。

## 手工闭环

1. 新建文档，加上绘图层 `strokes`，左右各画一团，两团不要连在一起。
2. 后端选 `code`，句子写「红色轮子，黄铜握把」，按「渲染」。出现 `an/轮子` 和 `an/握把`，`strokes` 还在。
3. 框住右边那团，句子写「蓝色握把」，按「改这里」。状态行写出认出的是握把。左边那层看起来不变。
4. 停掉 `an` 再启动。如果上一次没确认，状态行会要求按「放弃本次」。画布不会自动重画。
5. 把后端改成 `dsh` 或 `codex` 再渲染一次。计划不合法时，修订和 `an/` 图层保持第 3 步的样子。

句子栏空着时，面板先问「要渲染成什么样子」，你取消或留空就不会发请求。「渲染」不带选区。「改这里」必须有一块落在画布里的矩形选区。

写图层中途失败时，面板会 `POST /v1/abort`，并删掉这一次新加的 `an/` 图层。全部图层挂上之后才 `POST /v1/commit`。确认成功，修订才加一。

## 后端

`backends.toml` 里是 `dsh` 和 `codex` 的命令与超时。`code` 不启动子进程，按颜色词断句：红、蓝、绿、黄、黑、白、黄铜、钢、青绿。句子用 `，` `、` `,` `；` `;` 切开。段数和笔触块数不一致时会问你，不会猜。

`dsh` 和 `codex` 的工作目录是本次 attempt。提示词要求它们把 `plan.json` 写在那里。程序先读 `plan.json`，没有再从标准输出里取第一段 JSON。

## 测试

```sh
cargo test
python3 -m unittest discover -s krita/tests
```

测试不调用真的 `dsh` 或 `codex`。Krita 面板没有自动点击测试。

## 不做

不追每一笔的压感和顺序。这一次比上次确认多出来的像素就是笔触。选区重叠分不出胜负时不挑物体。不接图像生成。
