# cercis-core（内核）

Cercis 内核：**行业普适的"文件夹即数据库"引擎**。

- 只做两件事：**数据存取**（表/记录/文件/历史/查询原语）与 **AI 友好**（数据契约/机器清单/JSON/权限）
- 不认识任何学科概念；字段语义由工具包模板提供
- 用 Rust 编写；目标：单二进制、全平台、离线可用

## 当前状态（S1–S11 已完成）

- [x] 项目创建与打开（`project.yaml` + 目录骨架 + AGENTS.md 生成）
- [x] 表读写（CSV：UTF-8 无 BOM、LF、列名小写 ASCII 下划线）
- [x] 原子写入（临时文件 + 改名；断电不产生半写文件）；单行追加与**批量追加**（一次事务、一条流水）
- [x] 逐次存档（内容寻址对象库）+ 检查点（文件级备份，默认保留 50 个）
- [x] 变更流水（谁/设备/时间/动作/对象/前后指纹）
- [x] 更正机制（不覆盖原值）
- [x] 模板机制（表结构与字段语义来自模板，内核不含学科词）
- [x] 查询原语（组合筛选 + 排序 + 分页，结果可复现）
- [x] 引用完整性与字段校验
- [x] 机器清单 + AI 契约（`AGENTS.md` / `CLAUDE.md` / `manifest.json` / `ai_contract.json`）
- [x] **Agent 技能包**（`_labnote/skill/cercis/`）：装到 Codex / Claude Code 用的操作手册，与契约同源生成
- [x] 插件协议（清单、权限、界面扩展点、信任绑定内容指纹）
- [x] **加密共享与分权**（S11）：加密区域（磁盘即密文）、账号口令解锁、成员密钥封装、
  换密钥重加密、历史快照一并加密、解密导出留痕（`cercis share …`）
- [x] 命令行（`cercis`，含 `--json`）

## 构建与测试（作者自测）

```bash
cd kernel
cargo test          # 运行测试（133 个：100 单元 + 33 端到端/集成，其中 S11 加密 10 个）
cargo build         # 构建内核与命令行
cargo run -p cercis-cli -- --help
```

> 状态（2026-09-20）：构建通过，测试全绿（0 warning）。

## 常用命令速查

```bash
cercis init <目录> --name <项目名>            # 建项目
cercis install-template <目录> --name example-study
cercis append <目录> --table records --set record_id=R-1 --set date=2026-03-15
cercis append-many <目录> --table records --from 数据.csv --source-ref imports/数据.csv
cercis append-many <目录> --table records --from 数据.csv --dry-run   # 只校验不写入
cercis query <目录> --table records --filter value_num gt 10 --sort date:desc
cercis validate <目录>                        # 引用完整性 + 字段规则
cercis correct <目录> --table records --target R-1 --field value_num --old 1 --new 2 --reason 誊写笔误
cercis log <目录> --limit 20                  # 谁/哪台设备/何时改了什么
cercis note <目录> --action export --summary "导出 3 行到 /tmp/x.csv"   # 插件记录导出等动作
cercis agents <目录>                          # 刷新 AGENTS.md / 契约 / 清单 / Agent 技能包
cercis skill <目录> --install codex           # 把 Agent 技能包装到 ~/.codex/skills/
cercis plugins <目录> / cercis trust <目录> --id <插件ID>

# 加密共享与分权（S11）：区域内的文件在磁盘上是密文，解锁后透明读写
cercis share member-add <目录> --id alice --name 张老师        # 建成员（私钥用他的口令加密）
cercis share zone-create <目录> --name secret --path 'methods/配方/**' --member alice:admin
cercis share unlock <目录> --member alice                      # 解锁（会话在用户主目录，0600，不进项目）
cercis share status <目录>                                     # 区域、密钥指纹、明文残留、解不开的密文
cercis share export <目录> --zone secret                       # 解密导出到 exports/ 给 R / AI 用（留痕）
cercis share revoke <目录> --zone secret --member bob --as alice  # 移除成员：换密钥 + 重新加密
cercis share lock <目录>                                       # 上锁（--forget 连钥匙串口令一起删）
```

> 加密细节（算法、文件格式、密钥世代、历史快照迁移）见需求文档 §12.5.1 与 D47–D49；
> 一句话：**拿到文件夹 ≠ 拿得到数据**，忘记口令无法找回。

> 批量追加是**导入的写入原语**：整批要么全写入、要么全不写，且只产生一条流水
> （带 `batch_id` 与 `source_ref`）。导入的映射、预览、报告由基础工具包负责。

## 红线

1. 数据只增不改（更正写 `corrections.csv`；原值由 `history/objects/` 永久保留）
2. 写入必须原子
3. 不用本工具，仅靠 CSV/媒体文件也能理解并使用数据
