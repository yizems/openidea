本仓库是 fork 的别人的仓库,
我的分支是 me。`main` 由我手动与官方同步,不需要配置 upstream / 拉取官方。
需要依次执行以下任务(任一步失败则停止并汇报,不执行后续步骤):

0. 前置检查
   - 当前分支必须是 `me`, 工作区必须干净(`sync.md` 是未跟踪文件, 不要提交它, 也不要因它中断)。
1. `git rebase main`(使用本地 `main`, 不 fetch 官方)
2. 解决冲突
   - 原则: 保留 `me` 分支的定制内容, 同时合入 `main` 的新改动。
   - `Cargo.lock`、`pnpm-lock.yaml` 冲突时, 以 `main` 版本为基础重新生成, 不手工合并。
   - 汇报冲突文件及取舍。
3. 执行编译命令, 如果报错则修复
   - 仅做最小修复(只改让构建通过所必需的代码), 修复内容单独提交到 `me`。
   - 依赖有变化时先 `pnpm install --frozen-lockfile`。
   - 构建成功后确认产物 `target/aarch64-apple-darwin/release/bundle/macos/OpenIDEA.app` 存在。
   - 多次修复仍失败时停止并汇报, 不做大范围改动。

```
pnpm tauri build --target aarch64-apple-darwin
```

4. 压缩文件, 如果目标文件已存在, 则先删除(`rm -f OpenIDEA.zip`), 并确认 zip 非空

```
ditto -c -k --sequesterRsrc --keepParent \
  target/aarch64-apple-darwin/release/bundle/macos/OpenIDEA.app \
  OpenIDEA.zip
```

5. 调用 nodejs 脚本, 这个脚本会上传文件到我的 openlist(脚本写死了 zip 路径 `/Users/yize/projects/openidea/OpenIDEA.zip`)
   - 以退出码和输出判断是否成功; 失败则停止, 不打 tag、不推送。

```
node /Users/yize/projects/help-script-node-js/src/openlist/openidea.js
```

6. 上传成功后, 创建并推送 tag
   - `package.json` 中的 `version`(如 0.1.32)是官方版本号。**不要修改代码中任何版本号**(package.json / tauri.conf.json / Cargo.toml 保持三段式)。
   - tag 命名为 `v官方版本.序号`, 例如 `v0.1.32.1`。
   - 同时查询本地和远程(`git tag --list` + `git ls-remote --tags origin`)已有的 `v官方版本.*` tag:
     没有则用 `.1`; 有则取最大序号 +1, 保证不重复(当前已有 v0.1.32.1、v0.1.32.2, 下一个为 v0.1.32.3)。
   - 官方版本号升级后(如 0.1.33), 序号从 `.1` 重新开始。
   - tag 打在 rebase 及构建修复后的 `me` 最终提交上。
7. 推送
   - `git push --force-with-lease origin me`(已授权强制推送)。
   - `git push origin <新tag>`。
8. 汇报: 冲突文件及取舍、构建修复内容、新 tag、上传结果。

不需要: 添加 upstream、备份分支、固化成脚本、修改版本号。
