//! Cercis 命令行（`cercis`）：内核能力的第一个使用者。
//!
//! 设计目标：**人能用、脚本能用、AI 能用**。
//! - 所有命令支持 `--json`（稳定输出，带 schema 版本）
//! - 只读默认；写入命令明确列出，且全部经由内核通道留痕

use std::io::Read;
use std::path::{Path, PathBuf};

use clap::{Args, Parser, Subcommand};

use cercis_core::query::{Op, Query, SortDir};
use cercis_core::store::Identity;
use cercis_core::table::Row;
use cercis_core::{CercisError, Project};
use cercis_core::history::ChangeEntry;

/// Cercis —— 愿瑟希斯捍卫你的思想。
#[derive(Parser, Debug)]
#[command(
    name = "cercis",
    version,
    about = "Cercis 内核命令行（数据存取 · AI 友好）"
)]
struct Cli {
    /// 以 JSON 输出（机器/AI 读；schema 版本随输出携带）
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// 新建项目
    Init(InitArgs),
    /// 打开项目并体检
    Doctor(ProjectArg),
    /// 列出某张表的内容
    Show(ShowArgs),
    /// 追加一行（写入）
    Append(AppendArgs),
    /// 批量追加多行（一次事务、一条流水；导入用）
    AppendMany(AppendManyArgs),
    /// 记录一条更正（写入；不覆盖原值）
    Correct(CorrectArgs),
    /// 查看变更流水
    Log(LogArgs),
    /// 记一条流水（插件记录导出等动作；不改动数据文件）
    Note(NoteArgs),
    /// 创建检查点（文件级备份）
    Checkpoint(CheckpointArgs),
    /// 还原某个历史对象到文件（回滚 = 新变更）
    Restore(RestoreArgs),
    /// 查看项目模板（表结构与字段定义）
    Template(TemplateArgs),
    /// 装入模板（示例研究 / 通用）
    InstallTemplate(InstallTemplateArgs),
    /// 查看或切换术语包（只改显示名，不动数据）
    Terms(TermsArgs),
    /// 按条件查询某张表（组合筛选 + 排序）
    Query(QueryArgs),
    /// 校验项目（引用完整性 + 字段规则）
    Validate(ProjectArg),
    /// 列出值类型与约束（内核唯一定义；插件与 AI 照用）
    ValueTypes,
    /// 看某列该用什么类型（候选 + 置信度 + 反例；只建议，不改模板）
    InferTypes(InferTypesArgs),
    /// 改模板里的列类型/约束（写入；默认先给影响预览）
    SetFieldType(SetFieldTypeArgs),
    /// 刷新机器清单与 AI 材料（AGENTS.md / CLAUDE.md / ai_contract.json / 技能包）
    Agents(ProjectArg),
    /// 生成 AI Agent 技能包（装到 Codex / Claude Code 用）
    Skill(SkillArgs),
    /// 生成机器清单
    ManifestCmd(ProjectArg),
    /// 列出插件（全局与项目内；项目内默认未信任）
    Plugins(PluginsArgs),
    /// 信任项目内插件（绑定内容指纹）
    Trust(TrustArgs),
    /// 删掉一张**空表**（换模板后残留的空表清理；有数据会拒绝）
    DropTable(DropTableArgs),
    /// 插件市场：看预置市场、校验清单、从已下载的目录安装、卸载
    Market(MarketArgs),
    /// 加密共享与分权：成员、加密区域、解锁、解密导出（S11）
    Share(ShareArgs),
    /// 打包带走：全量/增量/分卷（加密区域保持密文；S12）
    Pack(PackArgs),
    /// 合并一个包：并集 + 冲突副本 + 校验先过（S12）
    Merge(MergeArgs),
    /// 分享守卫：扫一遍"能不能发出去"（白名单 + 真实数据拦截 + 隐私清单；S12）
    Guard(GuardArgs),
    /// 共存体检：云盘 / Git 目录里用会不会分叉，有没有已经分叉的迹象（FR-I06）
    Sync(SyncArgs),
    /// 差异对比（FR-M05）：一条记录改了什么，或两份快照多了/少了哪些行
    Diff(DiffArgs),
    /// NAS / 网络位置：登记、体检、写租约、凭据进钥匙串（FR-N01–N06）
    Remote(RemoteArgs),
    /// 工作副本：本地离线记录，联网后双向合并（FR-N03 / FR-N07）
    Workcopy(WorkcopyArgs),
    /// 多人账户与权限分级（FR-N08/N09）：账户、登录、角色
    Account(AccountArgs),
    /// 内核信息
    Version,
}

#[derive(Args, Debug)]
struct TemplateArgs {
    path: PathBuf,
    /// 只显示某张表
    #[arg(long)]
    table: Option<String>,
    /// 显示**未经术语包覆盖**的原始模板
    #[arg(long)]
    raw: bool,
}

#[derive(Args, Debug)]
struct InstallTemplateArgs {
    path: PathBuf,
    /// 模板名：generic / example-study
    #[arg(long, conflicts_with = "file")]
    name: Option<String>,
    /// 从模板文件装入（科研工具包提供的模板走这条路）
    #[arg(long, value_name = "FILE")]
    file: Option<PathBuf>,
    /// 代哪个插件登记（D37：登记后，停用该插件就能把模板还原回去）
    #[arg(long, value_name = "PLUGIN")]
    plugin: Option<String>,
}

#[derive(Args, Debug)]
struct TermsArgs {
    /// 项目目录
    path: PathBuf,
    /// 装入术语包文件（写入 <项目>/terms.yaml；不动数据）
    #[arg(long, value_name = "FILE")]
    file: Option<PathBuf>,
    /// 代哪个插件登记（D37：登记后，停用该插件就能把术语包还原回去）
    #[arg(long, value_name = "PLUGIN")]
    plugin: Option<String>,
}

#[derive(Args, Debug)]
struct InferTypesArgs {
    /// 项目目录（扫项目里的表）；只看一个数据文件时改用 --file
    #[arg(value_name = "PROJECT", required_unless_present = "file")]
    path: Option<PathBuf>,
    /// 直接扫一个数据文件（CSV/TSV/JSON，或 `-` 读标准输入）；不需要项目
    #[arg(long, value_name = "FILE|-", conflicts_with = "path")]
    file: Option<String>,
    /// 只看某张表（默认扫描模板里所有已存在的表）
    #[arg(long)]
    table: Option<String>,
    /// 只看某一列
    #[arg(long)]
    column: Option<String>,
    /// 强建议门槛（默认 0.95，即 95% 的格子匹配才给建议）
    #[arg(long, default_value_t = 0.95)]
    min_confidence: f64,
    /// 只列出"建议改类型"的列
    #[arg(long)]
    only_change: bool,
}

#[derive(Args, Debug)]
struct SetFieldTypeArgs {
    /// 项目目录
    path: PathBuf,
    /// 表名
    #[arg(long)]
    table: String,
    /// 列名
    #[arg(long)]
    field: String,
    /// 改成哪个值域：text / integer / number / bool / date / datetime / time
    #[arg(long = "type", value_name = "TYPE")]
    field_type: Option<String>,
    /// 设为必填
    #[arg(long, conflicts_with = "optional")]
    required: bool,
    /// 设为可选（取消必填）
    #[arg(long)]
    optional: bool,
    /// 计量单位（给空串即清空）
    #[arg(long)]
    unit: Option<String>,
    /// 建议范围（数值列）：如 `0,250`；`0,` 表示只设下限，`,250` 表示只设上限
    #[arg(long, value_name = "MIN,MAX", conflicts_with = "clear_range")]
    range: Option<String>,
    /// 清掉建议范围
    #[arg(long)]
    clear_range: bool,
    /// 允许值（分号分隔；给空串即清空）
    #[arg(long)]
    allowed_values: Option<String>,
    /// 正则模式（text 列；整格匹配）
    #[arg(long)]
    pattern: Option<String>,
    /// 小数位数（number 列；超出只提醒）
    #[arg(long, conflicts_with = "clear_precision")]
    precision: Option<u8>,
    /// 清掉小数位数
    #[arg(long)]
    clear_precision: bool,
    /// 引用 `表名.列名`（给空串即清空）
    #[arg(long)]
    reference: Option<String>,
    /// 更新这一列的说明
    #[arg(long)]
    notes: Option<String>,
    /// 只预览影响、不写模板
    #[arg(long)]
    dry_run: bool,
    /// 明知会新增校验错误也照改（数据可以随后补）
    #[arg(long)]
    force: bool,
    /// 写进变更流水的一句话（留空自动生成）
    #[arg(long, default_value = "")]
    summary: String,
}

#[derive(Args, Debug)]
struct QueryArgs {
    path: PathBuf,
    #[arg(long)]
    table: String,
    /// 筛选：col op value（如 --filter value_num ge 12）；可重复
    #[arg(long = "filter", value_name = "COL OP VAL", num_args = 3)]
    filters: Vec<String>,
    /// 排序：col 或 col:desc
    #[arg(long = "sort")]
    sort: Option<String>,
    #[arg(long, default_value_t = 50)]
    limit: usize,
    #[arg(long, default_value_t = 0)]
    offset: usize,
}

#[derive(Args, Debug)]
struct InitArgs {
    /// 项目目录（不存在则创建）
    path: PathBuf,
    /// 项目名称
    #[arg(long)]
    name: String,
    /// 用户名（默认取系统环境；仅记录用）
    #[arg(long, default_value = "local-user")]
    user: String,
    /// 是否记录设备信息
    #[arg(long)]
    record_device: bool,
}

#[derive(Args, Debug)]
struct ProjectArg {
    /// 项目目录
    path: PathBuf,
}

#[derive(Args, Debug)]
struct PluginsArgs {
    path: PathBuf,
    /// 全局插件目录（可选）
    #[arg(long)]
    global_dir: Option<PathBuf>,
    #[command(subcommand)]
    action: Option<PluginsAction>,
}

/// 插件相关动作（不写就是"列出"）。
#[derive(Subcommand, Debug)]
enum PluginsAction {
    /// 列出注册台账（谁往项目里放了什么）
    Registered(RegisteredArgs),
    /// 登记一条（插件对项目结构的增改；停用时靠它回滚）
    Register(RegisterArgs),
    /// 撤销某个插件的全部登记（倒序；先加 --dry-run 看会撤什么）
    Unregister(UnregisterArgs),
    /// 停用插件：记进停用名单，并（默认）回滚它的登记
    Disable(DisableArgs),
    /// 重新启用插件
    Enable(EnableArgs),
    /// 屏蔽插件（比停用更彻底：不加载、不在界面出现、不执行命令）
    Block(EnableArgs),
    /// 取消屏蔽（不自动启用，要不要用由你决定）
    Unblock(EnableArgs),
    /// 看"期望状态与实际状态"的差异（plugins/desired.yaml）
    Desired(DesiredArgs),
    /// 按期望状态对齐：改停用/屏蔽；装/删给出清单（给 --from 就顺手把本地目录里的装上）
    Reconcile(ReconcileArgs),
}

#[derive(Args, Debug)]
struct DesiredArgs {
    /// 写入/替换期望状态文件（YAML；不给就只对账）
    #[arg(long, value_name = "FILE")]
    write: Option<PathBuf>,
}

#[derive(Args, Debug)]
struct ReconcileArgs {
    /// 只看会对齐什么，不动任何东西
    #[arg(long)]
    dry_run: bool,
    /// 已下载好的市场仓库目录（给了就按 desired.yaml 里的 market 找对应清单并安装）
    #[arg(long)]
    from: Option<PathBuf>,
}

#[derive(Args, Debug)]
struct RegisteredArgs {
    /// 只看某个插件
    #[arg(long)]
    plugin: Option<String>,
    /// 连已撤回 / 被拦下的一起列出来
    #[arg(long)]
    all: bool,
}

#[derive(Args, Debug)]
struct RegisterArgs {
    /// 插件 ID（谁登记的）
    #[arg(long)]
    plugin: String,
    /// 种类：table / column / view / terms / template / cache / note
    #[arg(long)]
    kind: String,
    /// 目标：表名 / 表.列 / views/x.yaml / 包名 / 一句话
    #[arg(long)]
    target: String,
    /// 补充信息（可重复，形如 --detail type=number）
    #[arg(long = "detail", value_name = "K=V")]
    details: Vec<String>,
}

#[derive(Args, Debug)]
struct UnregisterArgs {
    /// 插件 ID
    #[arg(long)]
    plugin: String,
    /// 只看会撤什么，不动文件
    #[arg(long)]
    dry_run: bool,
}

#[derive(Args, Debug)]
struct DisableArgs {
    /// 插件 ID
    #[arg(long)]
    id: String,
    /// 不动它的登记（默认会回滚）
    #[arg(long)]
    keep_registrations: bool,
    /// 只看会撤什么，不动文件
    #[arg(long)]
    dry_run: bool,
}

#[derive(Args, Debug)]
struct EnableArgs {
    /// 插件 ID
    #[arg(long)]
    id: String,
}

#[derive(Args, Debug)]
struct SkillArgs {
    /// 项目目录（技能包按它的模板与契约生成）
    path: PathBuf,
    /// 写到指定目录（默认写进项目 `_labnote/skill/`）
    #[arg(long)]
    out: Option<PathBuf>,
    /// 直接装到 AI 宿主的技能目录：codex（~/.codex/skills）或 claude（~/.claude/skills）
    #[arg(long)]
    install: Option<String>,
}

#[derive(Args, Debug)]
struct TrustArgs {
    path: PathBuf,
    /// 插件 ID
    #[arg(long)]
    id: String,
    /// 撤销信任（默认是授予信任）
    #[arg(long)]
    revoke: bool,
    /// **允许这个插件联网**（清单里声明了网络时，必须显式点这个头；不点就一直拒绝运行）
    #[arg(long)]
    allow_network: bool,
    /// 取消"允许联网"
    #[arg(long)]
    deny_network: bool,
}

#[derive(Args, Debug)]
struct DropTableArgs {
    /// 项目目录
    path: PathBuf,
    /// 表名（不含 .csv）
    #[arg(long)]
    table: String,
    /// 只看会删什么
    #[arg(long)]
    dry_run: bool,
}

#[derive(Args, Debug)]
struct MarketArgs {
    #[command(subcommand)]
    action: MarketAction,
    /// 用户级插件目录（默认 ~/.cercis/plugins）
    #[arg(long, global = true, value_name = "DIR")]
    global_dir: Option<PathBuf>,
}

#[derive(Subcommand, Debug)]
enum MarketAction {
    /// 列出市场：预置的（基础包随包不可删 / 科研包预添加可删）与已安装的插件
    List,
    /// 添加一个市场（自建 / 私有仓库；鉴权在 app 侧）
    Add {
        /// git 仓库地址（用 --preset 找回预置市场时可省）
        #[arg(long, default_value = "")]
        repository: String,
        /// 市场 id（留空按仓库名推一个）
        #[arg(long, default_value = "")]
        id: String,
        /// 显示名
        #[arg(long, default_value = "")]
        label: String,
        /// 清单在仓库里的相对路径
        #[arg(long, default_value = "marketplace.json")]
        path: String,
        /// 默认分支 / 标签
        #[arg(long = "ref", default_value = "main")]
        git_ref: String,
        /// 把某个预置市场找回来（如 research）
        #[arg(long, default_value = "")]
        preset: String,
    },
    /// 移除一个市场（随包的基础市场不可移除；移除不会自动卸载它装的插件）
    Remove {
        /// 市场 id
        #[arg(long)]
        id: String,
    },
    /// 校验一份市场清单（文件路径；下载由 app 负责，内核不联网）
    Validate {
        /// marketplace.json（或 .yaml）
        #[arg(long)]
        file: PathBuf,
    },
    /// 从**已经下载好的**目录安装一个插件（断网可用；app 下载完就调它）
    Install {
        /// 下载到本地的仓库目录（里面含 marketplace.json）
        #[arg(long)]
        from: PathBuf,
        /// 装哪个插件（市场清单里的 id）
        #[arg(long)]
        plugin: String,
        /// 市场 id（记进安装台账）
        #[arg(long, default_value = "local")]
        market: String,
        /// 分支 / 标签 / 提交（记进安装台账）
        #[arg(long = "ref", default_value = "")]
        git_ref: String,
        /// 只看会装什么，不落盘
        #[arg(long)]
        dry_run: bool,
    },
    /// 删除插件（**随包插件也能删**：记墓碑，升级不会偷偷装回来）
    Uninstall {
        /// 插件 ID
        #[arg(long)]
        id: String,
        /// 只看会删什么
        #[arg(long)]
        dry_run: bool,
        /// 记一句"为什么不用它了"（可选）
        #[arg(long, default_value = "")]
        note: String,
    },
    /// 检查更新：对比已装与市场清单（**只提示、不自动升级**；权限变化要重新确认）
    Check {
        /// 已下载好的市场仓库目录（app 的下载缓存；内核不联网）
        #[arg(long)]
        from: PathBuf,
        /// 只看"有更新的"（默认全列）
        #[arg(long)]
        only_updates: bool,
    },
    /// 找回被删掉的插件（清墓碑；给 --from 就顺手装回来）
    Restore {
        /// 插件 ID
        #[arg(long)]
        id: String,
        /// 已下载好的仓库目录（给了就顺手装回来）
        #[arg(long)]
        from: Option<PathBuf>,
        /// 市场 id（记进安装台账）
        #[arg(long, default_value = "local")]
        market: String,
        /// 分支 / 标签 / 提交
        #[arg(long = "ref", default_value = "")]
        git_ref: String,
    },
}

#[derive(Args, Debug)]
struct AccountArgs {
    #[command(subcommand)]
    action: AccountAction,
}

#[derive(Subcommand, Debug)]
enum AccountAction {
    /// 新建账户（第一个必须是管理员；口令只存加盐哈希）
    Add {
        path: PathBuf,
        #[arg(long)]
        id: String,
        #[arg(long, default_value = "")]
        name: String,
        /// viewer / editor / admin
        #[arg(long, default_value = "editor")]
        role: String,
        #[arg(long)]
        password_stdin: bool,
        #[arg(long)]
        password_file: Option<PathBuf>,
        #[arg(long)]
        password_env: Option<String>,
        #[arg(long, default_value = "")]
        note: String,
    },
    /// 列出账户（看不到口令，只有角色）
    List { path: PathBuf },
    /// 登录（登录态存本机用户目录，不进项目）
    Login {
        path: PathBuf,
        #[arg(long)]
        id: String,
        #[arg(long)]
        password_stdin: bool,
        #[arg(long)]
        password_file: Option<PathBuf>,
        #[arg(long)]
        password_env: Option<String>,
        /// 登录有效小时数（默认 12）
        #[arg(long, default_value_t = 12)]
        ttl: i64,
    },
    /// 我现在是谁、什么角色
    Whoami { path: PathBuf },
    /// 退出登录（清掉本机登录态）
    Logout { path: PathBuf },
    /// 改口令（自己知道旧口令就行）
    Passwd {
        path: PathBuf,
        #[arg(long)]
        id: String,
        #[arg(long)]
        old_password_file: Option<PathBuf>,
        #[arg(long)]
        old_password_env: Option<String>,
        #[arg(long)]
        new_password_file: Option<PathBuf>,
        #[arg(long)]
        new_password_env: Option<String>,
    },
    /// 改角色（要管理员）
    Role {
        path: PathBuf,
        #[arg(long)]
        id: String,
        /// viewer / editor / admin
        #[arg(long)]
        role: String,
        /// 以哪位管理员的名义（没登录时用）
        #[arg(long = "as")]
        actor: Option<String>,
    },
    /// 删账户（要管理员；不能删掉最后一个管理员）
    Remove {
        path: PathBuf,
        #[arg(long)]
        id: String,
        #[arg(long = "as")]
        actor: Option<String>,
    },
}

#[derive(Args, Debug)]
struct RemoteArgs {
    #[command(subcommand)]
    action: RemoteAction,
}

#[derive(Subcommand, Debug)]
enum RemoteAction {
    /// 登记一个网络位置（填**挂载后的本地路径**；SMB/WebDAV/SFTP 由系统或 app 挂载）
    Add {
        /// 项目目录
        path: PathBuf,
        #[arg(long)]
        name: String,
        /// 挂载后的本地路径，如 /Volumes/lab-nas
        #[arg(long = "path", value_name = "挂载路径")]
        path_on_nas: String,
        /// 方式：smb / webdav / sftp / nfs / local（留空按路径猜）
        #[arg(long, default_value = "")]
        kind: String,
        #[arg(long, default_value = "")]
        note: String,
    },
    /// 列出已登记的网络位置
    List { path: PathBuf },
    /// 移除登记（不动任何文件；钥匙串里的凭据一起删）
    Remove {
        path: PathBuf,
        #[arg(long)]
        name: String,
    },
    /// 网络目录体检（FR-N06）：可达、可写、原子改名、大小写、中文名、速度
    Test {
        /// 项目目录
        path: PathBuf,
        /// 已登记的远程名（或直接给一个路径）
        #[arg(long)]
        name: Option<String>,
    },
    /// 取写租约：告诉别的设备"我在写"
    Claim {
        path: PathBuf,
        /// 谁在哪台设备上写，如 wenhuan@laptop
        #[arg(long)]
        who: String,
        #[arg(long, default_value_t = 30)]
        minutes: i64,
    },
    /// 释放写租约（顺手清掉已过期的）
    Release {
        path: PathBuf,
        #[arg(long)]
        who: String,
    },
    /// 凭据管理：存进系统钥匙串，**绝不写进项目**
    Credential {
        #[command(subcommand)]
        action: CredentialAction,
    },
}

#[derive(Subcommand, Debug)]
enum CredentialAction {
    /// 存入（口令来源：--password-env / --password-file / 标准输入）
    Set {
        path: PathBuf,
        #[arg(long)]
        name: String,
        #[arg(long)]
        password_stdin: bool,
        #[arg(long)]
        password_file: Option<PathBuf>,
        #[arg(long)]
        password_env: Option<String>,
    },
    /// 看有没有存过
    Has {
        path: PathBuf,
        #[arg(long)]
        name: String,
    },
    /// 删掉
    Delete {
        path: PathBuf,
        #[arg(long)]
        name: String,
    },
}

#[derive(Args, Debug)]
struct WorkcopyArgs {
    #[command(subcommand)]
    action: WorkcopyAction,
}

#[derive(Subcommand, Debug)]
enum WorkcopyAction {
    /// 从主副本拉一份工作副本到本地（离线也能记）
    Create {
        /// 主副本（NAS 上的项目）
        main: PathBuf,
        /// 本地副本目录（必须为空或不存在）
        #[arg(long)]
        to: PathBuf,
        /// 用哪个远程名（可留空）
        #[arg(long, default_value = "")]
        name: String,
    },
    /// 同步状态：本地/主副本各有多少新变更、上次同步、冲突（FR-N07）
    Status {
        /// 本地副本目录
        copy: PathBuf,
    },
    /// 双向同步：拉回来 + 推上去（断网会明确报离线）
    Sync {
        /// 本地副本目录
        copy: PathBuf,
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Args, Debug)]
struct DiffArgs {
    /// 项目目录
    path: PathBuf,
    /// 记录级对比：表名（如 records）
    #[arg(long)]
    table: Option<String>,
    /// 记录级对比：记录编号（如 R-1）
    #[arg(long)]
    id: Option<String>,
    /// 快照级对比：老那份历史对象的指纹
    #[arg(long)]
    from: Option<String>,
    /// 快照级对比：新那份历史对象的指纹
    #[arg(long)]
    to: Option<String>,
    /// 快照级对比：只显示变化，未变的行不打印
    #[arg(long)]
    only_changes: bool,
}

#[derive(Args, Debug)]
struct SyncArgs {
    /// 项目目录（也可以在云盘/Git 目录里）
    path: PathBuf,
}

#[derive(Args, Debug)]
struct GuardArgs {
    /// 要扫描的目录（工具仓库、准备分享的文件夹……）
    path: PathBuf,
    /// 顺手按白名单打出分享包（有拒绝项就直接不干）
    #[arg(long)]
    pack: Option<PathBuf>,
    /// 手动放行的路径规则（可重复，如 --allow 'examples/**'）
    #[arg(long = "allow", value_name = "规则")]
    allow: Vec<String>,
    /// 自定义敏感关键词（可重复，如 --keyword 未公开配方）
    #[arg(long = "keyword", value_name = "词")]
    keywords: Vec<String>,
    /// 跳过隐私文本扫描
    #[arg(long)]
    no_privacy: bool,
}

#[derive(Args, Debug)]
struct PackArgs {
    /// 项目目录
    path: PathBuf,
    /// 输出目录（**不能放在项目里面**，否则会把包自己装进包里）
    #[arg(long)]
    out: PathBuf,
    /// 每卷上限（MB，默认 20，适配微信；0 = 整包一卷）
    #[arg(long, default_value_t = 20)]
    part_size_mb: u64,
    /// 增量打包：上一次的 pack.json（或包含它的目录）
    #[arg(long)]
    since: Option<PathBuf>,
    /// 不装历史快照（history/objects；默认装，跨设备也能回溯）
    #[arg(long)]
    no_history: bool,
    /// 连检查点一起装（history/checkpoints；默认不装，与对象库重复）
    #[arg(long)]
    with_checkpoints: bool,
    /// 只看会装什么，不落盘
    #[arg(long)]
    dry_run: bool,
}

#[derive(Args, Debug)]
struct MergeArgs {
    /// 包目录（里面有 pack.json 与 part-* 卷）
    pack: PathBuf,
    /// 合并到哪个项目（本地文件永不被覆盖）
    #[arg(long)]
    into: PathBuf,
    /// 用哪个成员的会话（合并加密区域里的表要用；不给就用当前解锁的成员）
    #[arg(long)]
    member: Option<String>,
    /// 只看会发生什么，不动磁盘
    #[arg(long)]
    dry_run: bool,
}

#[derive(Args, Debug)]
struct ShareArgs {
    #[command(subcommand)]
    action: ShareAction,
    /// 会话目录（默认 ~/.cercis/share；测试或多用户机器上可改）
    #[arg(long, global = true, value_name = "DIR")]
    home: Option<PathBuf>,
}

#[derive(Subcommand, Debug)]
enum ShareAction {
    /// 看成员与加密区域（未授权成员看不到该区域）
    List {
        /// 项目目录
        path: PathBuf,
        /// 以某成员的身份看（不给就按当前已解锁的成员）
        #[arg(long)]
        member: Option<String>,
    },
    /// 看加密状态：每个区域多少文件、有没有还没加密的明文
    Status {
        path: PathBuf,
        #[arg(long)]
        member: Option<String>,
    },
    /// 新建成员（生成密钥对，私钥用他的口令加密）
    MemberAdd {
        path: PathBuf,
        /// 成员 id（小写 ASCII，如 alice）
        #[arg(long)]
        id: String,
        /// 显示名（可中文）
        #[arg(long, default_value = "")]
        name: String,
        /// 从标准输入读口令（脚本用；不给来源时也是从标准输入读一行）
        #[arg(long)]
        password_stdin: bool,
        /// 从文件读口令（文件里只放口令本身）
        #[arg(long)]
        password_file: Option<PathBuf>,
        /// 从环境变量读口令
        #[arg(long)]
        password_env: Option<String>,
        /// 备注（写给未来的自己）
        #[arg(long, default_value = "")]
        note: String,
    },
    /// 改口令（重新加密私钥；不动数据、不用重发区域密钥）
    Passwd {
        path: PathBuf,
        #[arg(long)]
        id: String,
        /// 旧口令所在文件
        #[arg(long)]
        old_password_file: Option<PathBuf>,
        /// 旧口令所在环境变量
        #[arg(long)]
        old_password_env: Option<String>,
        /// 新口令所在文件
        #[arg(long)]
        new_password_file: Option<PathBuf>,
        /// 新口令所在环境变量
        #[arg(long)]
        new_password_env: Option<String>,
    },
    /// 建加密区域（默认立刻把命中的文件加密）
    ZoneCreate {
        path: PathBuf,
        /// 区域名（小写 ASCII，如 patent-data）
        #[arg(long)]
        name: String,
        /// 显示名（可中文）
        #[arg(long, default_value = "")]
        label: String,
        /// 路径规则（可重复）：如 --path 'tables/patent_*.csv' --path 'methods/配方/**'
        #[arg(long = "path", value_name = "规则")]
        paths: Vec<String>,
        /// 成员与权限，形如 --member alice:admin（可重复；至少要有一个 admin）
        #[arg(long = "member", value_name = "成员[:权限]")]
        members: Vec<String>,
        /// 只写区域声明、先不加密（之后用 share apply）
        #[arg(long)]
        no_apply: bool,
        /// 加密后保留明文副本（默认把明文删掉，不留副本）
        #[arg(long)]
        keep_plaintext: bool,
    },
    /// 给区域加成员 / 改权限（新增成员只拿到当前世代的密钥）
    Grant {
        path: PathBuf,
        #[arg(long)]
        zone: String,
        #[arg(long)]
        member: String,
        /// read / write / admin
        #[arg(long, default_value = "read")]
        permission: String,
        /// 以哪位成员的身份操作（默认用当前已解锁的成员）
        #[arg(long = "as", value_name = "成员")]
        actor: Option<String>,
    },
    /// 移除成员（换密钥 + 重新加密；被移除者从此刻起读不到新内容）
    Revoke {
        path: PathBuf,
        #[arg(long)]
        zone: String,
        #[arg(long)]
        member: String,
        /// 只看会做什么，不动磁盘
        #[arg(long)]
        dry_run: bool,
        /// 以哪位成员的身份操作（默认用当前已解锁的成员）
        #[arg(long = "as", value_name = "成员")]
        actor: Option<String>,
    },
    /// 把区域里还没加密的文件加密（明文换密文）
    Apply {
        path: PathBuf,
        /// 只处理某个区域（不给就处理所有你有管理权限的区域）
        #[arg(long)]
        zone: Option<String>,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        keep_plaintext: bool,
        /// 以哪位成员的身份操作（默认用当前已解锁的成员）
        #[arg(long = "as", value_name = "成员")]
        actor: Option<String>,
    },
    /// 用当前密钥把区域内的密文重新写一遍（修复"密钥对不上"的旧密文）
    Rewrap {
        path: PathBuf,
        #[arg(long)]
        zone: String,
        /// 以哪位成员的身份操作（默认用当前已解锁的成员）
        #[arg(long = "as", value_name = "成员")]
        actor: Option<String>,
    },
    /// 解锁：解密私钥与自己的区域密钥（口令由你本人输入）
    Unlock {
        path: PathBuf,
        #[arg(long)]
        member: String,
        #[arg(long)]
        password_stdin: bool,
        #[arg(long)]
        password_file: Option<PathBuf>,
        #[arg(long)]
        password_env: Option<String>,
        /// 直接用系统钥匙串里存过的口令（配合 --remember）
        #[arg(long)]
        keychain: bool,
        /// 会话有效期（小时，默认 12）
        #[arg(long, default_value_t = 12)]
        ttl: i64,
        /// 把口令交给系统钥匙串保管（下次可 --keychain）
        #[arg(long)]
        remember: bool,
    },
    /// 上锁：清掉本机会话
    Lock {
        path: PathBuf,
        #[arg(long)]
        member: Option<String>,
        /// 连钥匙串里记住的口令一起删掉
        #[arg(long)]
        forget: bool,
    },
    /// 看区域里某个文件的明文（透明解密，不落盘）
    Cat {
        path: PathBuf,
        /// 项目内相对路径，如 tables/observations.csv
        #[arg(long)]
        file: String,
        #[arg(long)]
        member: Option<String>,
    },
    /// 解密导出到 exports/（给 R / AI / 投稿用；留痕）
    Export {
        path: PathBuf,
        #[arg(long)]
        zone: String,
        /// 导出到哪里（默认 exports/<区域名>）
        #[arg(long)]
        to: Option<String>,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        member: Option<String>,
    },
    /// 清理解密导出的明文副本（只动 exports/；用完就该删）
    Clean {
        path: PathBuf,
        /// 只清某个区域的导出（不给就清 exports/ 下面全部）
        #[arg(long)]
        zone: Option<String>,
        /// 只看会删什么
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Args, Debug)]
struct ShowArgs {
    path: PathBuf,
    /// 表名（不含 .csv）
    #[arg(long)]
    table: String,
    /// 最多显示多少行
    #[arg(long, default_value_t = 20)]
    limit: usize,
}

#[derive(Args, Debug)]
struct AppendArgs {
    path: PathBuf,
    #[arg(long)]
    table: String,
    /// 目标记录编号（写进流水，便于追溯）
    #[arg(long, default_value = "")]
    target: String,
    /// 一句话说明
    #[arg(long, default_value = "")]
    summary: String,
    /// 单元格：允许重复，形如 --set col=value
    #[arg(long = "set", value_name = "COL=VALUE")]
    sets: Vec<String>,
    /// 允许写入表里还不存在的列（按这一行自动扩展表结构）
    #[arg(long)]
    allow_new_columns: bool,
    /// 明知有类型错误也照写（默认拦下；数据可以随后补）
    #[arg(long)]
    force: bool,
}

#[derive(Args, Debug)]
struct AppendManyArgs {
    /// 项目目录
    path: PathBuf,
    #[arg(long)]
    table: String,
    /// 数据来源：CSV/TSV 文件、JSON 数组文件、或 `-`（从标准输入读 JSON）
    #[arg(long, value_name = "FILE|-")]
    from: String,
    /// 流水中记录的动作（create / import / …）
    #[arg(long, default_value = "import")]
    action: String,
    /// 一句话说明（留空自动生成）
    #[arg(long, default_value = "")]
    summary: String,
    /// 来源说明（导入文件路径等，便于追溯）
    #[arg(long, default_value = "")]
    source_ref: String,
    /// 允许写入模板未定义的列（默认拒绝）
    #[arg(long)]
    allow_new_columns: bool,
    /// 明知有类型错误也照写（默认拦下；数据可以随后补）
    #[arg(long)]
    force: bool,
    /// 只校验不写入（试运行）
    #[arg(long)]
    dry_run: bool,
}

#[derive(Args, Debug)]
struct CorrectArgs {
    path: PathBuf,
    /// 目标表
    #[arg(long)]
    table: String,
    /// 目标记录编号
    #[arg(long)]
    target: String,
    /// 字段名
    #[arg(long)]
    field: String,
    #[arg(long)]
    old: String,
    #[arg(long)]
    new: String,
    #[arg(long)]
    reason: String,
    #[arg(long, default_value = "")]
    source: String,
}

#[derive(Args, Debug)]
struct LogArgs {
    path: PathBuf,
    #[arg(long, default_value_t = 20)]
    limit: usize,
}

#[derive(Args, Debug)]
struct NoteArgs {
    /// 项目目录
    path: PathBuf,
    /// 动作类型：export / import / checkpoint / …
    #[arg(long)]
    action: String,
    /// 影响对象所在的表（可空）
    #[arg(long, default_value = "")]
    table: String,
    /// 影响对象的编号（可空）
    #[arg(long, default_value = "")]
    target: String,
    /// 一句话说明（谁做了什么、结果如何）
    #[arg(long)]
    summary: String,
    /// 来源说明（导出文件路径等）
    #[arg(long, default_value = "")]
    source_ref: String,
}

#[derive(Args, Debug)]
struct CheckpointArgs {
    path: PathBuf,
    #[arg(long, default_value = "手动检查点")]
    note: String,
}

#[derive(Args, Debug)]
struct RestoreArgs {
    path: PathBuf,
    /// 历史对象指纹（sha256）
    #[arg(long)]
    hash: String,
    /// 还原到哪个文件（项目内相对路径或绝对路径）
    #[arg(long)]
    to: PathBuf,
    #[arg(long, default_value = "回滚到历史版本")]
    summary: String,
}

fn main() {
    let cli = Cli::parse();
    if let Err(e) = run(&cli) {
        // 错误也要能机器读
        if cli.json {
            let payload = serde_json::json!({
                "schema": "cercis.cli/1",
                "ok": false,
                "error": e.to_string(),
            });
            println!("{}", serde_json::to_string_pretty(&payload).unwrap());
        } else {
            eprintln!("错误: {e}");
        }
        std::process::exit(1);
    }
}

fn run(cli: &Cli) -> Result<(), CercisError> {
    match &cli.command {
        Command::Template(a) => {
            let p = Project::open(&a.path, None)?;
            // 默认显示"术语包生效后"的标签；--raw 看原始模板
            let (t, terms) = if a.raw {
                (p.template()?, None)
            } else {
                p.template_with_terms()?
            };
            let profile = terms
                .as_ref()
                .map(|x| x.profile.clone())
                .unwrap_or_else(|| t.terms_profile.clone());
            if cli.json {
                print_json(&serde_json::json!({
                    "schema": "cercis.cli/1",
                    "ok": true,
                    "terms_profile": profile,
                    "terms_applied": terms.is_some(),
                    "terms_overrides": terms.as_ref().map(|x| x.override_count()).unwrap_or(0),
                    "template": t,
                }));
            } else {
                println!("模板：{}（{}）", t.name, t.label_cn);
                if let Some(pack) = &terms {
                    println!(
                        "术语包：{}（{}，覆盖 {} 处标签；列名不变）",
                        pack.profile,
                        pack.label_cn,
                        pack.override_count()
                    );
                }
                for td in &t.tables {
                    if let Some(only) = &a.table {
                        if only != &td.name {
                            continue;
                        }
                    }
                    println!(
                        "\n表 {}（{}） 主键: {}",
                        td.name, td.label_cn, td.primary_key
                    );
                    for f in &td.fields {
                        let unit = if f.unit.is_empty() {
                            String::new()
                        } else {
                            format!(" 单位={}", f.unit)
                        };
                        let req = if f.required { " 必填" } else { "" };
                        let rng = match &f.range {
                            Some(r) if r.len() == 2 => format!(
                                " 范围=[{}, {}]",
                                r[0].map(|v| v.to_string()).unwrap_or_default(),
                                r[1].map(|v| v.to_string()).unwrap_or_default()
                            ),
                            _ => String::new(),
                        };
                        let reff = f
                            .reference
                            .as_ref()
                            .map(|r| format!(" 引用={r}"))
                            .unwrap_or_default();
                        println!(
                            "  {:<20} {:<12} {}{}{}{}",
                            f.name, f.label_cn, f.field_type, unit, req, rng
                        );
                        if !reff.is_empty() {
                            println!("  {:<20} ↳{}", "", reff);
                        }
                    }
                }
            }
        }

        Command::InstallTemplate(a) => {
            let p = Project::open(&a.path, None)?;
            // D37：装模板是"覆盖式"改动——**改动前**先把上一份备份下来，
            // 这样停用插件时能原样还回去（备份名里带登记编号）
            let registration_id = a.plugin.as_ref().map(|_| cercis_core::ids::new_machine_id());
            let previous_template = p.template().map(|t| t.name).unwrap_or_default();
            let mut backup_detail = std::collections::BTreeMap::new();
            if let (Some(plugin), Some(id)) = (&a.plugin, &registration_id) {
                let _ = plugin;
                let (backup, _before_hash) =
                    cercis_core::registry::backup_for(&a.path, id, "template.yaml")?;
                backup_detail.insert("backup".to_string(), backup);
                backup_detail.insert("previous".to_string(), previous_template);
            }
            let t = match (&a.file, &a.name) {
                // 从文件装入：科研工具包等外部提供模板走这条通道
                // （内核保持行业普适——它不认识"科研"，只认识"模板文件"）
                (Some(file), _) => {
                    let text = std::fs::read_to_string(file).map_err(|e| {
                        CercisError::NotFound(format!("读不到模板文件 {}：{e}", file.display()))
                    })?;
                    cercis_core::Template::from_yaml(&text)?
                }
                (None, Some(name)) => match name.as_str() {
                    "generic" => cercis_core::Template::generic(),
                    "example-study" => cercis_core::Template::example_study(),
                    other => {
                        return Err(CercisError::Invalid(format!(
                            "未知模板 {other}（可用：generic / example-study，或用 --file 指定模板文件）"
                        )))
                    }
                },
                (None, None) => {
                    return Err(CercisError::Invalid(
                        "请给 --name <模板名> 或 --file <模板文件>".into(),
                    ))
                }
            };
            let created = p.install_template(&t)?;
            // 登记：装了模板之后，这个插件就对项目结构有了一个"可撤销的改动"
            if let (Some(plugin), Some(id)) = (&a.plugin, &registration_id) {
                let mut reg = cercis_core::Registry::load(&a.path)?;
                backup_detail.insert(
                    "sha256".to_string(),
                    cercis_core::registry::file_hash(&a.path.join("template.yaml")),
                );
                let entry = reg.register_with_id(
                    plugin,
                    cercis_core::RegKind::Template,
                    &t.name,
                    backup_detail.clone(),
                    id,
                )?;
                // 模板带出来的新表也要登记：停用插件时，空表跟着撤掉（有数据的会拦下）
                for table in &created {
                    let mut detail = std::collections::BTreeMap::new();
                    detail.insert("from".to_string(), format!("模板 {}", t.name));
                    reg.register(plugin, cercis_core::RegKind::Table, table, detail)?;
                }
                reg.save(&a.path)?;
                if !cli.json {
                    println!(
                        "（已为插件 {} 登记：模板 {}；停用它时会恢复上一份 {}）",
                        entry.plugin,
                        entry.target,
                        backup_detail
                            .get("previous")
                            .cloned()
                            .unwrap_or_else(|| "（无）".into())
                    );
                }
            }
            if cli.json {
                print_json(&serde_json::json!({
                    "schema": "cercis.cli/1",
                    "ok": true,
                    "template": t.name,
                    "created_tables": created,
                    "registered_for": a.plugin,
                }));
            } else {
                println!("已装入模板：{}", t.name);
                if let Some(f) = &a.file {
                    println!("来源：{}", f.display());
                }
                if created.is_empty() {
                    println!("（表已存在，未新建）");
                } else {
                    println!("新建表：{}", created.join(", "));
                }
            }
        }

        Command::Terms(a) => {
            let p = Project::open(&a.path, None)?;
            if let Some(file) = &a.file {
                let text = std::fs::read_to_string(file).map_err(|e| {
                    CercisError::NotFound(format!("读不到术语包 {}：{e}", file.display()))
                })?;
                let terms = cercis_core::Terms::from_yaml(&text)?;
                // D37：切术语包是"覆盖式"改动——先备份上一份，再切，再登记
                let registration_id =
                    a.plugin.as_ref().map(|_| cercis_core::ids::new_machine_id());
                let mut backup_detail = std::collections::BTreeMap::new();
                if let (Some(_), Some(id)) = (&a.plugin, &registration_id) {
                    let previous = p
                        .terms()?
                        .map(|t| t.profile)
                        .unwrap_or_else(|| {
                            p.template()
                                .map(|t| t.terms_profile)
                                .unwrap_or_default()
                        });
                    let (backup, _) =
                        cercis_core::registry::backup_for(&a.path, id, "terms.yaml")?;
                    backup_detail.insert("backup".to_string(), backup);
                    backup_detail.insert("previous".to_string(), previous);
                }
                p.install_terms(&terms)?;
                if let (Some(plugin), Some(id)) = (&a.plugin, &registration_id) {
                    let mut reg = cercis_core::Registry::load(&a.path)?;
                    backup_detail.insert(
                        "sha256".to_string(),
                        cercis_core::registry::file_hash(&a.path.join("terms.yaml")),
                    );
                    reg.register_with_id(
                        plugin,
                        cercis_core::RegKind::Terms,
                        &terms.profile,
                        backup_detail.clone(),
                        id,
                    )?;
                    reg.save(&a.path)?;
                }
                if cli.json {
                    print_json(&serde_json::json!({
                        "schema": "cercis.cli/1",
                        "ok": true,
                        "profile": terms.profile,
                        "label_cn": terms.label_cn,
                        "overrides": terms.override_count(),
                        "source": file.display().to_string(),
                        "registered_for": a.plugin,
                    }));
                } else {
                    println!(
                        "已切换术语包：{}（{}）——覆盖 {} 处标签，**数据文件未改动**",
                        terms.profile,
                        terms.label_cn,
                        terms.override_count()
                    );
                }
                return Ok(());
            }

            match p.terms()? {
                Some(t) => {
                    if cli.json {
                        print_json(&serde_json::json!({
                            "schema": "cercis.cli/1",
                            "ok": true,
                            "profile": t.profile,
                            "label_cn": t.label_cn,
                            "overrides": t.override_count(),
                        }));
                    } else {
                        println!(
                            "当前术语包：{}（{}）｜覆盖 {} 处标签",
                            t.profile,
                            t.label_cn,
                            t.override_count()
                        );
                    }
                }
                None => {
                    if cli.json {
                        print_json(&serde_json::json!({
                            "schema": "cercis.cli/1",
                            "ok": true,
                            "profile": "（未装术语包）",
                        }));
                    } else {
                        println!("未装术语包：界面按模板原本的中英文标签显示。");
                        println!("装入方式：cercis terms <项目> --file <术语包.yaml>");
                    }
                }
            }
        }

        Command::Query(a) => {
            let p = Project::open(&a.path, None)?;
            let mut q = Query::new();

            // 解析 --filter col op value（三连）
            if a.filters.len() % 3 != 0 {
                return Err(CercisError::Invalid(
                    "--filter 需要三个值：列名 运算符 值".into(),
                ));
            }
            for trio in a.filters.chunks(3) {
                let (col, op, val) = (&trio[0], &trio[1], &trio[2]);
                let op = match op.as_str() {
                    "eq" => Op::Eq,
                    "ne" => Op::Ne,
                    "lt" => Op::Lt,
                    "le" => Op::Le,
                    "gt" => Op::Gt,
                    "ge" => Op::Ge,
                    "contains" => Op::Contains,
                    "startswith" => Op::StartsWith,
                    "in" => Op::In,
                    "isempty" => Op::IsEmpty,
                    "notempty" => Op::NotEmpty,
                    other => {
                        return Err(CercisError::Invalid(format!(
                            "未知运算符 {other}（可用: eq ne lt le gt ge contains startswith in isempty notempty）"
                        )))
                    }
                };
                q = q.filter(col, op, val);
            }

            if let Some(sort) = &a.sort {
                let (col, dir) = match sort.split_once(':') {
                    Some((c, "desc")) => (c, SortDir::Desc),
                    Some((c, "asc")) => (c, SortDir::Asc),
                    Some((_, other)) => {
                        return Err(CercisError::Invalid(format!(
                            "排序方向只能是 asc 或 desc，实际: {other}"
                        )))
                    }
                    None => (sort.as_str(), SortDir::Asc),
                };
                q = q.sort(col, dir);
            }
            q = q.page(a.offset, a.limit);

            let rows = p.query(&a.table, q)?;
            if cli.json {
                print_json(&serde_json::json!({
                    "schema": "cercis.cli/1",
                    "ok": true,
                    "table": a.table,
                    "count": rows.len(),
                    "rows": rows,
                }));
            } else {
                println!("表 {}：命中 {} 行", a.table, rows.len());
                for r in &rows {
                    let line = r
                        .iter()
                        .map(|(k, v)| format!("{k}={v}"))
                        .collect::<Vec<_>>()
                        .join("  ");
                    println!("  {line}");
                }
            }
        }

        Command::Agents(a) => {
            let m = cercis_core::ai::refresh_all(&a.path)?;
            if cli.json {
                print_json(&serde_json::json!({
                    "schema": "cercis.cli/1",
                    "ok": true,
                    "project": m.project_name,
                    "tables": m.tables.len(),
                }));
            } else {
                println!("已刷新 AI 材料：");
                println!("  AGENTS.md（AI 说明书）");
                println!("  CLAUDE.md（其他 AI 工具入口）");
                println!("  _labnote/manifest.json（机器清单）");
                println!("  _labnote/ai_contract.json（机器契约）");
                println!("  _labnote/skill/cercis/（Agent 技能包，装到 Codex / Claude Code 用）");
            }
        }

        Command::Skill(a) => {
            if a.out.is_some() && a.install.is_some() {
                return Err(CercisError::Invalid(
                    "--out 与 --install 只能用一个（前者写到你指定的目录，后者装进 AI 宿主）".into(),
                ));
            }
            let p = Project::open(&a.path, None)?;
            let manifest = cercis_core::Manifest::generate(&a.path)?;
            let contract = cercis_core::AiContract::build(&p.template()?, &manifest);
            let skill = cercis_core::AgentSkill::build(&contract);

            let (target, host) = if let Some(host) = a.install.as_deref() {
                (skill.write(skill_host_dir(host)?)?, Some(host.to_string()))
            } else if let Some(out) = a.out.as_ref() {
                (skill.write(out)?, None)
            } else {
                (skill.write_into_project(&a.path)?, None)
            };

            let files = vec![
                target.join("SKILL.md").display().to_string(),
                target.join("reference.md").display().to_string(),
            ];
            if cli.json {
                print_json(&serde_json::json!({
                    "schema": "cercis.cli/1",
                    "ok": true,
                    "skill": skill.name,
                    "target": target.display().to_string(),
                    "host": host,
                    "files": files,
                    "commands": contract.commands.len(),
                }));
            } else {
                println!("已生成 Agent 技能包：{}", target.display());
                for f in &files {
                    println!("  {f}");
                }
                println!(
                    "说明：技能包与 _labnote/ai_contract.json 同源（{} 条命令）；改模板后重新生成即可。",
                    contract.commands.len()
                );
                if host.is_none() {
                    println!("安装：cercis skill <项目> --install codex（或 claude）");
                }
            }
        }

        Command::ManifestCmd(a) => {
            let m = cercis_core::Manifest::generate(&a.path)?;
            m.write(&a.path)?;
            if cli.json {
                print_json(&serde_json::json!({
                    "schema": "cercis.cli/1",
                    "ok": true,
                    "manifest": m,
                }));
            } else {
                println!(
                    "已生成清单：{} 张表，变更 {} 条",
                    m.tables.len(),
                    m.change_count
                );
                for t in &m.tables {
                    println!("  {:<16} {:>6} 行  {} 列", t.name, t.rows, t.columns.len());
                }
            }
        }

        Command::Plugins(a) => {
            // 改插件（不是看插件）要过 FR-N11 的闸门：
            // 项目开了 plugins.admin_only_edit 时只有管理员能动
            let mutating = matches!(
                a.action.as_ref(),
                Some(PluginsAction::Register(_))
                    | Some(PluginsAction::Unregister(_))
                    | Some(PluginsAction::Disable(_))
                    | Some(PluginsAction::Enable(_))
                    | Some(PluginsAction::Block(_))
                    | Some(PluginsAction::Unblock(_))
                    | Some(PluginsAction::Reconcile(_))
            );
            if mutating {
                cercis_core::accounts::require_plugin_edit(&a.path)?;
            }
            let found = cercis_core::plugin::discover(&a.path, a.global_dir.as_deref())?;
            let trust = cercis_core::TrustRecord::load(&a.path)?;
            let state = cercis_core::plugin::PluginState::load(&a.path)?;

            // 子命令：注册台账 / 停用 / 启用
            if let Some(action) = &a.action {
                return match action {
                    PluginsAction::Registered(x) => {
                        let reg = cercis_core::Registry::load(&a.path)?;
                        let list: Vec<&cercis_core::Registration> = reg
                            .entries
                            .iter()
                            .filter(|e| {
                                x.plugin.as_deref().map(|p| e.plugin == p).unwrap_or(true)
                                    && (x.all
                                        || e.state == cercis_core::RegState::Active
                                        || e.state == cercis_core::RegState::Blocked)
                            })
                            .collect();
                        if cli.json {
                            print_json(&serde_json::json!({
                                "schema": "cercis.cli/1",
                                "ok": true,
                                "count": list.len(),
                                "registrations": list,
                            }));
                        } else if list.is_empty() {
                            println!("没有登记（插件还没往这个项目里放过东西）。");
                        } else {
                            for e in &list {
                                let state_cn = match e.state {
                                    cercis_core::RegState::Active => "生效",
                                    cercis_core::RegState::Reverted => "已撤回",
                                    cercis_core::RegState::Blocked => "被拦下",
                                };
                                println!(
                                    "{}｜{}｜{} {}｜{}",
                                    e.plugin,
                                    state_cn,
                                    e.kind.as_str(),
                                    e.target,
                                    if e.blocked_reason.is_empty() {
                                        e.detail
                                            .get("previous")
                                            .map(|p| format!("上一份：{p}"))
                                            .unwrap_or_default()
                                    } else {
                                        e.blocked_reason.clone()
                                    }
                                );
                            }
                        }
                        Ok(())
                    }
                    PluginsAction::Register(x) => {
                        use std::collections::BTreeMap;
                        let kind = cercis_core::RegKind::parse(&x.kind).ok_or_else(|| {
                            CercisError::Invalid(format!(
                                "未知登记种类：{}（可用：table / column / view / terms / template / cache / note）",
                                x.kind
                            ))
                        })?;
                        // 覆盖式改动（术语包 / 模板）必须由内核在改动**之前**备份，
                        // 所以走 `install-template --plugin …` / `terms --plugin …`，不在这里登记
                        if matches!(
                            kind,
                            cercis_core::RegKind::Terms | cercis_core::RegKind::Template
                        ) {
                            return Err(CercisError::Invalid(
                                "术语包与模板要在改动前先备份，请改用：\
                                 `cercis terms <项目> --file <包> --plugin <插件>` 或 \
                                 `cercis install-template <项目> --file <模板> --plugin <插件>`"
                                    .into(),
                            ));
                        }
                        let mut detail: BTreeMap<String, String> = BTreeMap::new();
                        for kv in &x.details {
                            let (k, v) = kv.split_once('=').ok_or_else(|| {
                                CercisError::Invalid(format!("--detail 要写成 k=v，实际：{kv}"))
                            })?;
                            detail.insert(k.trim().to_string(), v.trim().to_string());
                        }
                        let p = Project::open(&a.path, None)?;
                        let mut reg = cercis_core::Registry::load(&a.path)?;
                        let id = cercis_core::ids::new_machine_id();
                        let entry = reg.register_with_id(
                            &x.plugin,
                            kind,
                            &x.target,
                            detail,
                            &id,
                        )?;
                        reg.save(&a.path)?;
                        p.store.append_change(&ChangeEntry::new(
                            &p.store.identity.user_id,
                            &p.store.identity.device_for_log(),
                            "register",
                            kind.as_str(),
                            &x.target,
                            &format!(
                                "插件 {} 登记：{} {}（撤销时会：{}）",
                                x.plugin,
                                kind.as_str(),
                                x.target,
                                kind.revert_hint()
                            ),
                            "",
                            "",
                        ))?;
                        if cli.json {
                            print_json(&serde_json::json!({
                                "schema": "cercis.cli/1",
                                "ok": true,
                                "registration": entry,
                            }));
                        } else {
                            println!(
                                "已登记：{} {}（{}）；撤销时会：{}",
                                kind.as_str(),
                                x.target,
                                x.plugin,
                                kind.revert_hint()
                            );
                        }
                        Ok(())
                    }
                    PluginsAction::Unregister(x) => {
                        let p = Project::open(&a.path, None)?;
                        let mut reg = cercis_core::Registry::load(&a.path)?;
                        let items = reg.revert(&a.path, &x.plugin, x.dry_run)?;
                        if !x.dry_run && !items.is_empty() {
                            let reverted = items.iter().filter(|i| i.outcome == "reverted").count();
                            let blocked = items.iter().filter(|i| i.outcome == "blocked").count();
                            p.store.append_change(&ChangeEntry::new(
                                &p.store.identity.user_id,
                                &p.store.identity.device_for_log(),
                                "unregister",
                                "plugin",
                                &x.plugin,
                                &format!(
                                    "撤销插件 {} 的登记：撤回 {} 项，被拦下 {} 项",
                                    x.plugin, reverted, blocked
                                ),
                                "",
                                "",
                            ))?;
                        }
                        if cli.json {
                            print_json(&serde_json::json!({
                                "schema": "cercis.cli/1",
                                "ok": true,
                                "dry_run": x.dry_run,
                                "items": items,
                            }));
                        } else if items.is_empty() {
                            println!("插件 {} 没有需要撤销的登记。", x.plugin);
                        } else {
                            for i in &items {
                                let mark = match i.outcome.as_str() {
                                    "planned" => "会",
                                    "reverted" => "✓",
                                    _ => "✗",
                                };
                                println!("{} {} {}：{}", mark, i.kind, i.target, i.message);
                            }
                            if x.dry_run {
                                println!("（试运行：没有动任何文件。去掉 --dry-run 才真正撤销）");
                            }
                        }
                        Ok(())
                    }
                    PluginsAction::Disable(x) => {
                        let p = Project::open(&a.path, None)?;
                        let mut st = cercis_core::plugin::PluginState::load(&a.path)?;
                        let mut reg = cercis_core::Registry::load(&a.path)?;
                        let items = if x.keep_registrations {
                            Vec::new()
                        } else {
                            reg.revert(&a.path, &x.id, x.dry_run)?
                        };
                        if !x.dry_run {
                            st.disable(&x.id);
                            st.save(&a.path)?;
                            p.store.append_change(&ChangeEntry::new(
                                &p.store.identity.user_id,
                                &p.store.identity.device_for_log(),
                                "disable",
                                "plugin",
                                &x.id,
                                &format!(
                                    "停用插件 {}（回滚登记 {} 项{}）",
                                    x.id,
                                    items.iter().filter(|i| i.outcome == "reverted").count(),
                                    if x.keep_registrations {
                                        "，保留了登记"
                                    } else {
                                        ""
                                    }
                                ),
                                "",
                                "",
                            ))?;
                        }
                        if cli.json {
                            print_json(&serde_json::json!({
                                "schema": "cercis.cli/1",
                                "ok": true,
                                "dry_run": x.dry_run,
                                "disabled": x.id,
                                "rollback": items,
                            }));
                        } else if x.dry_run {
                            println!("试运行：会停用 {}，并撤销 {} 项登记：", x.id, items.len());
                            for i in &items {
                                println!("  会 {} {}：{}", i.kind, i.target, i.message);
                            }
                        } else {
                            println!("已停用 {}（撤销登记 {} 项）", x.id, items.len());
                            for i in items.iter().filter(|i| i.outcome == "blocked") {
                                println!(
                                    "  ⚠ 留在项目里了：{} {}——{}",
                                    i.kind, i.target, i.message
                                );
                            }
                        }
                        Ok(())
                    }
                    PluginsAction::Enable(x) => {
                        let p = Project::open(&a.path, None)?;
                        let mut st = cercis_core::plugin::PluginState::load(&a.path)?;
                        st.enable(&x.id);
                        st.save(&a.path)?;
                        p.store.append_change(&ChangeEntry::new(
                            &p.store.identity.user_id,
                            &p.store.identity.device_for_log(),
                            "enable",
                            "plugin",
                            &x.id,
                            &format!("重新启用插件 {}", x.id),
                            "",
                            "",
                        ))?;
                        if cli.json {
                            print_json(&serde_json::json!({
                                "schema": "cercis.cli/1",
                                "ok": true,
                                "enabled": x.id,
                            }));
                        } else {
                            println!("已重新启用 {}", x.id);
                        }
                        Ok(())
                    }
                    PluginsAction::Desired(x) => {
                        // --write：把这份期望状态写进项目（手写或 app 生成都行）
                        if let Some(file) = &x.write {
                            let text = std::fs::read_to_string(file).map_err(|e| {
                                CercisError::NotFound(format!("读不到 {}：{e}", file.display()))
                            })?;
                            let desired = cercis_core::Desired::from_yaml(&text)?;
                            desired.save(&a.path)?;
                            if cli.json {
                                print_json(&serde_json::json!({
                                    "schema": "cercis.cli/1",
                                    "ok": true,
                                    "action": "plugins.desired.write",
                                    "path": cercis_core::desired::DESIRED_FILE,
                                    "install": desired.install.len(),
                                    "disable": desired.disable.len(),
                                    "block": desired.block.len(),
                                }));
                            } else {
                                println!(
                                    "已写入期望状态：{}（想装 {} 个、停用 {} 个、屏蔽 {} 个）",
                                    cercis_core::desired::DESIRED_FILE,
                                    desired.install.len(),
                                    desired.disable.len(),
                                    desired.block.len()
                                );
                            }
                            return Ok(());
                        }
                        // 只对账
                        let Some(desired) = cercis_core::Desired::load_optional(&a.path)? else {
                            return Err(CercisError::NotFound(format!(
                                "{} 不存在——先写一份（--write <文件>），或手写这个文件",
                                cercis_core::desired::DESIRED_FILE
                            )));
                        };
                        let installs = cercis_core::Installs::load(&global_dir_plugins()?)?;
                        let state = cercis_core::plugin::PluginState::load(&a.path)?;
                        let known: Vec<String> = found.iter().map(|p| p.manifest.id.clone()).collect();
                        let d = cercis_core::desired::diff(&desired, &installs, &state, &known);
                        if cli.json {
                            print_json(&serde_json::json!({
                                "schema": "cercis.cli/1",
                                "ok": true,
                                "aligned": d.is_empty(),
                                "diff": d,
                            }));
                        } else if d.is_empty() {
                            println!("期望状态与实际状态一致（{} 个想装的都在位）。", desired.install.len());
                        } else {
                            println!("期望状态对账（只算账，不动文件）：");
                            for w in &d.missing {
                                println!("  ＋ 要装：{}（市场 {}）", w.id, w.market);
                            }
                            for id in &d.extra {
                                println!("  － 多余（可删）：{id}");
                            }
                            for id in &d.to_disable {
                                println!("  ⏸ 要停用：{id}");
                            }
                            for id in &d.to_block {
                                println!("  ⛔ 要屏蔽：{id}");
                            }
                            for id in &d.to_enable {
                                println!("  ▶ 要启用：{id}");
                            }
                            for id in &d.to_unblock {
                                println!("  ◻ 要取消屏蔽：{id}");
                            }
                            for n in &d.notes {
                                println!("  · {n}");
                            }
                            println!("\n对齐：`cercis plugins <项目> reconcile`（装/删要 app 下载或 --from <目录>）");
                        }
                        Ok(())
                    }
                    PluginsAction::Reconcile(x) => {
                        let Some(desired) = cercis_core::Desired::load_optional(&a.path)? else {
                            return Err(CercisError::NotFound(format!(
                                "{} 不存在——先写一份（--write <文件>）",
                                cercis_core::desired::DESIRED_FILE
                            )));
                        };
                        let installs = cercis_core::Installs::load(&global_dir_plugins()?)?;
                        let mut state = cercis_core::plugin::PluginState::load(&a.path)?;
                        let known: Vec<String> = found.iter().map(|p| p.manifest.id.clone()).collect();
                        let d = cercis_core::desired::diff(&desired, &installs, &state, &known);

                        // 1) 内核自己能做完的：改停用/屏蔽（都要留痕）
                        let mut applied: Vec<String> = Vec::new();
                        for id in &d.to_disable {
                            applied.push(format!("停用 {id}"));
                        }
                        for id in &d.to_block {
                            applied.push(format!("屏蔽 {id}"));
                        }
                        for id in &d.to_unblock {
                            applied.push(format!("取消屏蔽 {id}"));
                        }
                        for id in &d.to_enable {
                            applied.push(format!("启用 {id}"));
                        }
                        if !x.dry_run && !applied.is_empty() {
                            for id in &d.to_disable {
                                state.disable(id);
                            }
                            for id in &d.to_block {
                                state.block(id);
                            }
                            for id in &d.to_unblock {
                                state.unblock(id);
                            }
                            for id in &d.to_enable {
                                state.enable(id);
                            }
                            state.save(&a.path)?;
                            let p = Project::open(&a.path, None)?;
                            p.store.append_change(&ChangeEntry::new(
                                &p.store.identity.user_id,
                                &p.store.identity.device_for_log(),
                                "reconcile",
                                "plugin",
                                "",
                                &format!("按期望状态对齐：{}", applied.join("、")),
                                "",
                                "",
                            ))?;
                        }

                        // 2) 装：只有给了 --from 才动手（内核不联网）
                        let mut installed: Vec<String> = Vec::new();
                        let mut skipped: Vec<String> = Vec::new();
                        if let Some(from) = &x.from {
                            let listing = if from.join("marketplace.json").is_file() {
                                from.join("marketplace.json")
                            } else {
                                from.join("marketplace.yaml")
                            };
                            let market_data = cercis_core::Marketplace::load(&listing)?;
                            for w in &d.missing {
                                // 只按"清单里有没有这个插件"判断，不拿市场名去猜——
                                // 用户给的 --from 是哪份清单，就按哪份清单装
                                let Some(entry) = market_data.plugin(&w.id) else {
                                    skipped.push(format!(
                                        "{} 不在这份市场清单里（{}），跳过",
                                        w.id,
                                        market_data.summary()
                                    ));
                                    continue;
                                };
                                let src = from.join(&entry.path);
                                if !src.is_dir() {
                                    continue;
                                }
                                if x.dry_run {
                                    installed.push(format!("会装 {} v{}", entry.id, entry.version));
                                    continue;
                                }
                                let repo = if market_data.repository.is_empty() {
                                    from.display().to_string()
                                } else {
                                    market_data.repository.clone()
                                };
                                let git_ref = if w.git_ref.is_empty() { "main" } else { &w.git_ref };
                                cercis_core::market::install_from_dir(
                                    &src,
                                    &global_dir_plugins()?,
                                    &w.market,
                                    &repo,
                                    git_ref,
                                    market_data.bundled,
                                )?;
                                installed.push(format!("装好 {} v{}", entry.id, entry.version));
                            }
                        }

                        if cli.json {
                            print_json(&serde_json::json!({
                                "schema": "cercis.cli/1",
                                "ok": true,
                                "dry_run": x.dry_run,
                                "diff": d,
                                "applied": applied,
                                "installed": installed,
                                "skipped": skipped,
                            }));
                        } else {
                            let mark = if x.dry_run { "会" } else { "✓" };
                            if applied.is_empty() && installed.is_empty() && d.missing.is_empty() {
                                println!("已经对齐，没有要做的。");
                            } else {
                                for a in &applied {
                                    println!("{mark} {a}");
                                }
                                for i in &installed {
                                    println!("{mark} {i}");
                                }
                                if x.dry_run && !applied.is_empty() {
                                    println!("（试运行：没有动任何文件）");
                                }
                            }
                            for s in &skipped {
                                println!("  ! {s}");
                            }
                            for w in &d.missing {
                                if x.from.is_none() {
                                    println!("  ＋ 还要装：{}（市场 {}）——app 下载后装，或加 --from <已下载目录>", w.id, w.market);
                                }
                            }
                            for id in &d.extra {
                                println!("  － 多余（可删）：{id}（`cercis market uninstall --id {id}`）");
                            }
                            for n in &d.notes {
                                println!("  · {n}");
                            }
                        }
                        Ok(())
                    }
                    PluginsAction::Block(x) => {
                        let p = Project::open(&a.path, None)?;
                        let mut st = cercis_core::plugin::PluginState::load(&a.path)?;
                        st.block(&x.id);
                        st.save(&a.path)?;
                        p.store.append_change(&ChangeEntry::new(
                            &p.store.identity.user_id,
                            &p.store.identity.device_for_log(),
                            "block",
                            "plugin",
                            &x.id,
                            &format!("屏蔽插件 {}（不加载、界面不出现、命令不执行）", x.id),
                            "",
                            "",
                        ))?;
                        if cli.json {
                            print_json(&serde_json::json!({
                                "schema": "cercis.cli/1",
                                "ok": true,
                                "blocked": x.id,
                            }));
                        } else {
                            println!("已屏蔽 {}（要恢复：plugins unblock --id {}）", x.id, x.id);
                        }
                        Ok(())
                    }
                    PluginsAction::Unblock(x) => {
                        let p = Project::open(&a.path, None)?;
                        let mut st = cercis_core::plugin::PluginState::load(&a.path)?;
                        st.unblock(&x.id);
                        st.save(&a.path)?;
                        p.store.append_change(&ChangeEntry::new(
                            &p.store.identity.user_id,
                            &p.store.identity.device_for_log(),
                            "unblock",
                            "plugin",
                            &x.id,
                            &format!("取消屏蔽插件 {}", x.id),
                            "",
                            "",
                        ))?;
                        if cli.json {
                            print_json(&serde_json::json!({
                                "schema": "cercis.cli/1",
                                "ok": true,
                                "unblocked": x.id,
                            }));
                        } else {
                            println!("已取消屏蔽 {}（它现在是停用状态，要启用再跑 enable）", x.id);
                        }
                        Ok(())
                    }

                };
            }

            if cli.json {
                print_json(&serde_json::json!({
                    "schema": "cercis.cli/1",
                    "ok": true,
                    "plugins": found.iter().map(|p| {
                        let activation = p.manifest.activation(&a.path);
                        serde_json::json!({
                            "id": p.manifest.id,
                            "name": p.manifest.name,
                            "namespace": p.manifest.namespace_id(),
                            "tier": p.manifest.tier,
                            "origin": p.origin,
                            "fingerprint": p.fingerprint,
                            "trusted": trust.is_trusted(&p.manifest.id, &p.fingerprint),
                            "disabled": state.is_disabled(&p.manifest.id),
                            "blocked": state.is_blocked(&p.manifest.id),
                            "active": activation.active && !state.is_disabled(&p.manifest.id),
                            "inactive_reasons": activation.reasons,
                            "permissions": p.manifest.permission_summary(),
                            // 结构化权限（界面/脚本要判断"能不能联网、能碰哪几张表"时用这个）
                            "permission_detail": {
                                "read": p.manifest.permissions.read,
                                "write": p.manifest.permissions.write,
                                "network": p.manifest.permissions.network,
                                "network_allowed": trust.is_network_allowed(&p.manifest.id),
                                "tables": p.manifest.permissions.tables,
                                "required_tables": p.manifest.requires.tables,
                            },
                            "ui": p.manifest.ui.len(),
                        })
                    }).collect::<Vec<_>>(),
                    "conflicts": cercis_core::plugin::conflicts(&found),
                    "blocked_registrations": cercis_core::Registry::load(&a.path)?.blocked(),
                }));
            } else if found.is_empty() {
                println!("未发现插件。");
            } else {
                let conflicts = cercis_core::plugin::conflicts(&found);
                for p in &found {
                    let trusted = trust.is_trusted(&p.manifest.id, &p.fingerprint);
                    let disabled = state.is_disabled(&p.manifest.id);
                    let blocked = state.is_blocked(&p.manifest.id);
                    let activation = p.manifest.activation(&a.path);
                    let status = if blocked {
                        "已屏蔽（不加载、界面不出现）"
                    } else if disabled {
                        "已停用"
                    } else if p.origin == "project" {
                        if trusted {
                            "已信任"
                        } else {
                            "未信任（默认不加载）"
                        }
                    } else {
                        "全局"
                    };
                    println!(
                        "{}  {}  [{}]  {}  {}  权限：{}  界面扩展：{}",
                        p.manifest.id,
                        p.manifest.name,
                        p.origin,
                        status,
                        if disabled {
                            "".to_string()
                        } else if activation.active {
                            "生效".to_string()
                        } else {
                            format!("未生效（{}）", activation.reasons.join("；"))
                        },
                        p.manifest.permission_summary(),
                        p.manifest.ui.len()
                    );
                }
                for c in conflicts {
                    println!("⚠ {c}");
                }
                let registry = cercis_core::Registry::load(&a.path)?;
                let blocked = registry.blocked();
                if !blocked.is_empty() {
                    println!(
                        "有 {} 项登记撤销时被拦下（背后有数据或文件被改过）：",
                        blocked.len()
                    );
                    for b in blocked {
                        println!("  {} {}——{}", b.plugin, b.target, b.blocked_reason);
                    }
                }
            }
        }

        Command::Trust(a) => {
            cercis_core::accounts::require_plugin_edit(&a.path)?;
            let found = cercis_core::plugin::discover(&a.path, None)?;
            let target = found
                .iter()
                .find(|p| p.manifest.id == a.id)
                .ok_or_else(|| CercisError::NotFound(format!("未找到插件：{}", a.id)))?;
            let mut tr = cercis_core::TrustRecord::load(&a.path)?;
            // 信任也是一次"项目状态变更"，按 D39 记进流水
            let p = Project::open(&a.path, None)?;
            if a.revoke {
                tr.untrust(&a.id);
                tr.set_network_allowed(&a.id, false);
                tr.save(&a.path)?;
                p.store.append_change(&ChangeEntry::new(
                    &p.store.identity.user_id,
                    &p.store.identity.device_for_log(),
                    "untrust",
                    "plugin",
                    &a.id,
                    &format!("撤销对插件 {} 的信任", a.id),
                    "",
                    "",
                ))?;
                println!("已撤销信任：{}", a.id);
            } else {
                tr.trust(&a.id, &target.fingerprint);
                // 联网要单独点头：声明里要联网却不批准，插件一运行就会被拒（权限降级）
                if a.allow_network {
                    tr.set_network_allowed(&a.id, true);
                }
                if a.deny_network {
                    tr.set_network_allowed(&a.id, false);
                }
                tr.save(&a.path)?;
                p.store.append_change(&ChangeEntry::new(
                    &p.store.identity.user_id,
                    &p.store.identity.device_for_log(),
                    "trust",
                    "plugin",
                    &a.id,
                    &format!("信任插件 {}（指纹 {}）", a.id, target.fingerprint),
                    "",
                    "",
                ))?;
                println!("已信任插件：{}", a.id);
                println!("  权限：{}", target.manifest.permission_summary());
                if !target.manifest.permissions.network.is_off() {
                    println!(
                        "  联网：{}{}",
                        target.manifest.permissions.network.summary(),
                        if tr.is_network_allowed(&a.id) {
                            "（已批准）"
                        } else {
                            "（**尚未批准**：要联网请加 --allow-network）"
                        }
                    );
                }
                if !target.manifest.permissions.tables.is_empty() {
                    println!(
                        "  只能碰这些表：{}",
                        target.manifest.permissions.tables.join("、")
                    );
                }
                println!("  指纹：{}", target.fingerprint);
                println!("（内容一旦变化，信任将自动失效）");
            }
        }

        Command::DropTable(a) => {
            // 删表是结构性改动 → 管理员
            cercis_core::accounts::require(&a.path, cercis_core::Role::Admin)?;
            let p = Project::open(&a.path, None)?;
            if a.dry_run {
                let path = p.root().join("tables").join(format!("{}.csv", a.table));
                if !path.exists() {
                    return Err(CercisError::NotFound(format!("表不存在：{}", a.table)));
                }
                let t = cercis_core::Table::load(&path)?;
                if !t.rows.is_empty() {
                    return Err(CercisError::Invalid(format!(
                        "表 {} 里已有 {} 行数据——只删空表",
                        a.table,
                        t.rows.len()
                    )));
                }
                if cli.json {
                    print_json(&serde_json::json!({
                        "schema": "cercis.cli/1",
                        "ok": true,
                        "dry_run": true,
                        "table": a.table,
                        "message": format!("会删掉空表 {}（并把它从模板里去掉）", a.table),
                    }));
                } else {
                    println!("试运行：会删掉空表 {}（并把它从模板里去掉）", a.table);
                    println!("（去掉 --dry-run 才真正删除；删除动作会写进流水）");
                }
                return Ok(());
            }
            let (table, from_template) = p.drop_empty_table(&a.table)?;
            if cli.json {
                print_json(&serde_json::json!({
                    "schema": "cercis.cli/1",
                    "ok": true,
                    "table": table,
                    "removed_from_template": from_template,
                }));
            } else {
                println!(
                    "已删掉空表 {table}{}",
                    if from_template {
                        "（也从模板里移除了）"
                    } else {
                        ""
                    }
                );
            }
        }

        Command::Market(a) => market_cmd(a, cli.json)?,
        Command::Share(a) => share_cmd(a, cli.json)?,
        Command::Pack(a) => pack_cmd(a, cli.json)?,
        Command::Merge(a) => merge_cmd(a, cli.json)?,
        Command::Guard(a) => guard_cmd(a, cli.json)?,
        Command::Sync(a) => sync_cmd(a, cli.json)?,
        Command::Diff(a) => diff_cmd(a, cli.json)?,
        Command::Remote(a) => remote_cmd(a, cli.json)?,
        Command::Workcopy(a) => workcopy_cmd(a, cli.json)?,
        Command::Account(a) => account_cmd(a, cli.json)?,

        Command::Validate(a) => {
            let p = Project::open(&a.path, None)?;
            let report = p.validate()?;
            if cli.json {
                print_json(&serde_json::json!({
                    "schema": "cercis.cli/1",
                    "ok": report.is_ok(),
                    "errors": report.errors,
                    "warnings": report.warnings,
                }));
            } else {
                print!("{}", cercis_core::validate::report_to_text(&report));
            }
            if !report.is_ok() {
                std::process::exit(2);
            }
        }

        Command::ValueTypes => {
            let c = cercis_core::value::contract();
            if cli.json {
                print_json(&serde_json::json!({
                    "schema": "cercis.cli/1",
                    "ok": true,
                    "value_types": c,
                }));
            } else {
                println!("值类型（内核唯一定义；插件与 AI 照用，不得自造）\n");
                for d in &c.domains {
                    println!(
                        "  - {:<9} {:<6}｜违反时：{}",
                        d.name,
                        d.label_cn,
                        severity_cn(d.severity_on_violation)
                    );
                    println!("      怎么写：{}", d.csv_form);
                    if !d.examples_ok.is_empty() {
                        println!("      合法例子：{}", d.examples_ok.join("、"));
                    }
                    if !d.examples_bad.is_empty() {
                        println!("      必须报告：{}", d.examples_bad.join("、"));
                    }
                    if !d.inferable {
                        println!("      注：只校验、不参与类型推断");
                    }
                }
                println!("\n约束（与类型正交，可组合）");
                for k in &c.constraints {
                    println!(
                        "  - {:<15} {:<6}｜适用于 {}｜{}",
                        k.name,
                        k.label_cn,
                        k.applies_to,
                        severity_cn(k.severity)
                    );
                    println!("      {}", k.notes);
                }
                println!("\n{}", c.missing_value_rule);
                println!("\n校验时点：");
                for p in &c.validation_points {
                    println!("  - {p}");
                }
                println!("\n{}", c.severity_policy);
            }
        }

        Command::InferTypes(a) => {
            // 两种用法：扫项目里的表 / 直接扫一个数据文件（导入向导问"这列像什么"）
            if let Some(file) = &a.file {
                let (headers, rows) = read_table_for_inference(file)?;
                let mut columns: Vec<cercis_core::infer::ColumnInference> =
                    infer_from_rows(&headers, &rows);
                if let Some(only) = &a.column {
                    columns.retain(|c| &c.column == only);
                }
                if a.only_change {
                    columns.retain(|c| c.suggest_change);
                }
                if cli.json {
                    print_json(&serde_json::json!({
                        "schema": "cercis.cli/1",
                        "ok": true,
                        "source": file,
                        "threshold": a.min_confidence,
                        "columns": columns,
                    }));
                } else {
                    println!("来源：{file}（未对照模板，只看数据本身）\n");
                    for c in &columns {
                        match &c.profile.recommended {
                            Some(r) => println!(
                                "{} → 建议 {r}（{}）",
                                c.column,
                                confidence_text(&c.profile)
                            ),
                            None => println!(
                                "{} → 无强建议（{}）",
                                c.column,
                                confidence_text(&c.profile)
                            ),
                        }
                        let s = &c.profile.stats;
                        println!(
                            "    非空 {}／共 {} 格（空 {}，占位符写法 {}）",
                            s.non_empty, s.total, s.empty, s.missing_markers
                        );
                        if let Some(text) = counterexamples_text(&c.profile) {
                            println!("    反例：{text}");
                        }
                        for n in &c.profile.notes {
                            println!("    注：{n}");
                        }
                    }
                }
                return Ok(());
            }
            let path = a
                .path
                .clone()
                .ok_or_else(|| CercisError::Invalid("请给项目目录，或用 --file 指定数据文件".into()))?;
            let p = Project::open(&path, None)?;
            let template = p.template()?;
            let tables: Vec<String> = match &a.table {
                Some(t) => vec![t.clone()],
                None => template.tables.iter().map(|t| t.name.clone()).collect(),
            };
            let mut columns: Vec<cercis_core::infer::ColumnInference> = Vec::new();
            let mut scanned_tables: Vec<String> = Vec::new();
            for name in &tables {
                let path = p.root().join("tables").join(format!("{name}.csv"));
                if !path.exists() {
                    continue;
                }
                let table = p.store.load_table_with_template(name, &template)?;
                let declared: Vec<(String, String)> = template
                    .table(name)
                    .map(|td| {
                        td.fields
                            .iter()
                            .map(|f| (f.name.clone(), f.field_type.clone()))
                            .collect()
                    })
                    .unwrap_or_default();
                let mut found = cercis_core::infer::infer_table(&table, &declared);
                for c in found.iter_mut() {
                    c.table = name.clone();
                }
                columns.extend(found);
                scanned_tables.push(name.clone());
            }
            if let Some(only) = &a.column {
                columns.retain(|c| &c.column == only);
            }
            if a.only_change {
                columns.retain(|c| c.suggest_change);
            }

            let suggest_count = columns.iter().filter(|c| c.suggest_change).count();
            let declared_bad = columns.iter().filter(|c| c.declared_violations > 0).count();

            if cli.json {
                print_json(&serde_json::json!({
                    "schema": "cercis.cli/1",
                    "ok": true,
                    "threshold": a.min_confidence,
                    "tables_scanned": scanned_tables,
                    "columns": columns,
                    "summary": {
                        "columns": columns.len(),
                        "suggest_change": suggest_count,
                        "columns_with_violations": declared_bad,
                    },
                }));
            } else if columns.is_empty() {
                println!("没有可推断的列（表不存在，或没有数据）。");
            } else {
                for c in &columns {
                    let s = &c.profile.stats;
                    let head = format!("{}.{}（声明 {}）", c.table, c.column, c.declared_type);
                    match &c.profile.recommended {
                        Some(r) if c.suggest_change => {
                            println!("{head} → 建议改成 {r}（{}）", confidence_text(&c.profile));
                        }
                        Some(r) => {
                            println!("{head} → 与建议一致（{r}，{}）", confidence_text(&c.profile));
                        }
                        None => {
                            println!("{head} → 无强建议（{}）", confidence_text(&c.profile));
                        }
                    }
                    println!(
                        "    非空 {}／共 {} 格（空 {}，占位符写法 {}）",
                        s.non_empty, s.total, s.empty, s.missing_markers
                    );
                    if c.declared_violations > 0 {
                        println!(
                            "    ⚠ 有 {} 格不符合声明的 {} 类型：{}",
                            c.declared_violations,
                            c.declared_type,
                            c.declared_counterexamples.join("、")
                        );
                    }
                    if c.suggest_change {
                        if let Some(text) = counterexamples_text(&c.profile) {
                            println!("    反例：{text}");
                        }
                    }
                    for n in &c.profile.notes {
                        println!("    注：{n}");
                    }
                }
                println!(
                    "\n共 {} 列（表格 {}）：{} 列建议改类型，{} 列有不符合当前类型的格子。",
                    columns.len(),
                    scanned_tables.join("、"),
                    suggest_count,
                    declared_bad
                );
                println!("提示：这只是建议，不会自动改模板；确认后用 set-field-type 改（先加 --dry-run 看影响）。");
            }
        }

        Command::SetFieldType(a) => {
            // 改字段类型是结构性改动 → 管理员
            cercis_core::accounts::require(&a.path, cercis_core::Role::Admin)?;
            let p = Project::open(&a.path, None)?;
            let patch = build_field_patch(a)?;
            if patch.is_empty() {
                return Err(CercisError::Invalid(
                    "没有要改的内容（至少给一项：--type / --required / --optional / --range / --pattern / --precision / --allowed-values / --reference / --unit / --notes）"
                        .into(),
                ));
            }
            let current = p.template()?;
            let next = current.with_field_patch(&a.table, &a.field, &patch)?;

            // 影响预览：拿新模板去校验现有数据，只算"这一次改动**新增**的问题"
            let before = p.validate()?;
            let after = p.validate_with(&next)?;
            let mut new_errors: Vec<&cercis_core::Issue> = Vec::new();
            let mut new_warnings: Vec<&cercis_core::Issue> = Vec::new();
            for issue in after.errors.iter() {
                if !before.errors.iter().any(|b| same_issue(b, issue)) {
                    new_errors.push(issue);
                }
            }
            for issue in after.warnings.iter() {
                if !before.warnings.iter().any(|b| same_issue(b, issue)) {
                    new_warnings.push(issue);
                }
            }

            let describe = patch.describe();
            if !new_errors.is_empty() && !a.force {
                let mut msg = format!(
                    "这次改动会让 {} 处格子不符合校验（示例：{}）",
                    new_errors.len(),
                    new_errors
                        .iter()
                        .take(3)
                        .map(|e| format!("{}:{} {}.{}", e.table, e.row, e.field, e.message))
                        .collect::<Vec<_>>()
                        .join("；")
                );
                msg.push_str("。已存在的原值不会被改动：直接写会留下这些错误，体检会报出来。确认要改就加 --force（数据随后补），或先清理数据。");
                return Err(CercisError::Invalid(msg));
            }

            if a.dry_run {
                if cli.json {
                    print_json(&serde_json::json!({
                        "schema": "cercis.cli/1",
                        "ok": true,
                        "dry_run": true,
                        "table": a.table,
                        "field": a.field,
                        "change": describe,
                        "new_errors": new_errors.len(),
                        "new_warnings": new_warnings.len(),
                        "errors": new_errors,
                        "warnings": new_warnings,
                    }));
                } else {
                    println!("试运行：{}.{} 将改为 {}", a.table, a.field, describe);
                    println!(
                        "影响：新增错误 {} 处，新增提醒 {} 处（未写模板）",
                        new_errors.len(),
                        new_warnings.len()
                    );
                    for e in new_errors.iter().take(5) {
                        println!("  错误 {}:{} {}：{}", e.table, e.row, e.field, e.message);
                    }
                    for w in new_warnings.iter().take(5) {
                        println!("  提醒 {}:{} {}：{}", w.table, w.row, w.field, w.message);
                    }
                    println!("（去掉 --dry-run 即正式写入模板；数据文件不会被改动）");
                }
                return Ok(());
            }

            let (_, describe) = p.apply_field_patch(&a.table, &a.field, &patch, &a.summary)?;
            if cli.json {
                print_json(&serde_json::json!({
                    "schema": "cercis.cli/1",
                    "ok": true,
                    "dry_run": false,
                    "table": a.table,
                    "field": a.field,
                    "change": describe,
                    "new_errors": new_errors.len(),
                    "new_warnings": new_warnings.len(),
                }));
            } else {
                println!(
                    "已修改模板：{}.{}（{}）",
                    a.table, a.field, describe
                );
                println!(
                    "数据文件未改动；新增错误 {} 处、新增提醒 {} 处（cercis validate 可复核）",
                    new_errors.len(),
                    new_warnings.len()
                );
                if !a.summary.is_empty() {
                    println!("说明：{}", a.summary);
                }
            }
        }

        Command::Version => {
            if cli.json {
                print_json(&serde_json::json!({
                    "schema": "cercis.cli/1",
                    "ok": true,
                    "core_version": cercis_core::CORE_VERSION,
                    "schema_version": cercis_core::SCHEMA_VERSION,
                    "dirs": cercis_core::project::DIRS.len(),
                }));
            } else {
                println!(
                    "Cercis 内核 {}（项目结构版本 {}）",
                    cercis_core::CORE_VERSION,
                    cercis_core::SCHEMA_VERSION
                );
                println!("愿瑟希斯捍卫你的思想。 / May Cerces safeguard your thoughts.");
            }
        }

        Command::Init(a) => {
            let identity = Identity {
                user_id: a.user.clone(),
                device_id: hostname_or("local-device"),
                record_device: a.record_device,
            };
            let p = Project::create(&a.path, &a.name, identity)?;
            if cli.json {
                print_json(&serde_json::json!({
                    "schema": "cercis.cli/1",
                    "ok": true,
                    "project": p.root().display().to_string(),
                    "name": p.config.name,
                }));
            } else {
                println!("已创建项目：{}", p.root().display());
                println!("项目名称：{}", p.config.name);
            }
        }

        Command::Doctor(a) => {
            let p = Project::open(&a.path, None)?;
            let issues = p.doctor()?;
            if cli.json {
                print_json(&serde_json::json!({
                    "schema": "cercis.cli/1",
                    "ok": true,
                    "issues": issues,
                }));
            } else if issues.is_empty() {
                println!("体检通过，未发现问题。");
            } else {
                println!("发现 {} 项：", issues.len());
                for i in issues {
                    println!("  - {i}");
                }
            }
        }

        Command::Show(a) => {
            let p = Project::open(&a.path, None)?;
            let t = p.store.load_table(&a.table, &[])?;
            println!(
                "表 {}（{} 列，{} 行；显示前 {} 行）",
                a.table,
                t.columns.len(),
                t.rows.len(),
                a.limit
            );
            for r in t.rows.iter().take(a.limit) {
                let cells = t
                    .columns
                    .iter()
                    .map(|c| format!("{c}={}", r.get(c).cloned().unwrap_or_default()))
                    .collect::<Vec<_>>()
                    .join("  ");
                println!("  {cells}");
            }
        }

        Command::Append(a) => {
            let p = Project::open(&a.path, None)?;
            let mut row = Row::new();
            for kv in &a.sets {
                let (k, v) = kv.split_once('=').ok_or_else(|| {
                    CercisError::Invalid(format!("--set 需要 col=value 形式，实际: {kv}"))
                })?;
                row.insert(k.trim().to_string(), v.trim().to_string());
            }
            if row.is_empty() {
                return Err(CercisError::Invalid("至少提供一个 --set col=value".into()));
            }
            // 写入前的那道校验（§5.9.3）：结构错误拦下，提醒放行
            let template = p.template()?;
            let (errors, warnings) = check_cells_against_template(&template, &a.table, &[row.clone()]);
            if !errors.is_empty() && !a.force {
                return Err(CercisError::Invalid(format!(
                    "{} 处格子不符合模板的类型规则（{}）。要么改值，要么加 --force 照写（会留下已知错误）",
                    errors.len(),
                    errors
                        .iter()
                        .take(3)
                        .map(|e| format!("{}：{}", e.field, e.message))
                        .collect::<Vec<_>>()
                        .join("；")
                )));
            }
            for w in warnings.iter().take(3) {
                eprintln!("提醒 [{}] {}：{}", w.row, w.field, w.message);
            }
            // 若表不存在，按本次提供的列创建（列名按字母序稳定）
            let owned: Vec<String> = row.keys().cloned().collect();
            let cols: Vec<&str> = owned.iter().map(|s| s.as_str()).collect();
            if a.allow_new_columns {
                p.store
                    .append_row_extending(&a.table, &cols, row, &a.target, &a.summary)?;
            } else {
                p.store
                    .append_row(&a.table, &cols, row, &a.target, &a.summary)?;
            }
            if cli.json {
                print_json(&serde_json::json!({
                    "schema": "cercis.cli/1",
                    "ok": true,
                    "table": a.table,
                    "target": a.target,
                    "extended_columns": a.allow_new_columns,
                }));
            } else if a.allow_new_columns {
                println!("已写入表 {}（必要时自动扩展了列；已留痕）", a.table);
            } else {
                println!("已写入表 {}（已留痕）", a.table);
            }
        }

        Command::AppendMany(a) => {
            let rows = read_rows_input(&a.from)?;
            if rows.is_empty() {
                return Err(CercisError::Invalid(format!("{} 中没有数据行", a.from)));
            }

            let p = Project::open(&a.path, None)?;
            let template = p.template()?;
            let template_cols: Vec<String> = template
                .table(&a.table)
                .map(|td| td.fields.iter().map(|f| f.name.clone()).collect())
                .unwrap_or_default();

            // 输入涉及的列：模板列在前，输入里新增的列在后（顺序稳定，便于复核）
            let mut columns = template_cols.clone();
            for r in &rows {
                for k in r.keys() {
                    if !columns.contains(k) {
                        columns.push(k.clone());
                    }
                }
            }

            // 模板已定义该表时，默认拒绝写入模板之外的列（防止手滑多写一列脏数据）
            if !template_cols.is_empty() && !a.allow_new_columns {
                let extra: Vec<&str> = columns[template_cols.len()..]
                    .iter()
                    .map(|s| s.as_str())
                    .collect();
                if !extra.is_empty() {
                    return Err(CercisError::Invalid(format!(
                        "输入含有模板 {} 未定义的列：{}（请先扩展模板，或显式加 --allow-new-columns）",
                        template.name,
                        extra.join(", ")
                    )));
                }
            }

            let summary = if a.summary.is_empty() {
                format!("导入 {} 行到表 {}（来源：{}）", rows.len(), a.table, a.from)
            } else {
                a.summary.clone()
            };

            // 写入前的类型校验（§5.9.3 的第一个时点）：结构错误整批拦下，提醒放行。
            let (cell_errors, cell_warnings) =
                check_cells_against_template(&template, &a.table, &rows);

            if a.dry_run {
                if cli.json {
                    print_json(&serde_json::json!({
                        "schema": "cercis.cli/1",
                        "ok": true,
                        "dry_run": true,
                        "table": a.table,
                        "rows": rows.len(),
                        "columns": columns,
                        "errors": cell_errors,
                        "warnings": cell_warnings,
                        "sample": rows.iter().take(3).collect::<Vec<_>>(),
                    }));
                } else {
                    println!("试运行：表 {} 将新增 {} 行", a.table, rows.len());
                    println!("列：{}", columns.join(", "));
                    for r in rows.iter().take(3) {
                        let line = r
                            .iter()
                            .map(|(k, v)| format!("{k}={v}"))
                            .collect::<Vec<_>>()
                            .join("  ");
                        println!("  {line}");
                    }
                    if cell_errors.is_empty() {
                        println!("校验：没有结构错误（{} 处提醒）", cell_warnings.len());
                    } else {
                        println!("校验：{} 处错误（这些会让整批写不进去）", cell_errors.len());
                        for e in cell_errors.iter().take(8) {
                            println!("  错误 第 {} 行 {}：{}", e.row, e.field, e.message);
                        }
                        if cell_errors.len() > 8 {
                            println!("  ……还有 {} 处", cell_errors.len() - 8);
                        }
                    }
                    for w in cell_warnings.iter().take(8) {
                        println!("  提醒 第 {} 行 {}：{}", w.row, w.field, w.message);
                    }
                    println!("（未写入任何数据；去掉 --dry-run 即正式导入）");
                }
                return Ok(());
            }

            if !cell_errors.is_empty() && !a.force {
                return Err(CercisError::Invalid(format!(
                    "有 {} 处格子不符合模板的类型规则，整批未写入（先加 --dry-run 看清单；要照写加 --force）。示例：{}",
                    cell_errors.len(),
                    cell_errors
                        .iter()
                        .take(3)
                        .map(|e| format!("第 {} 行 {}：{}", e.row, e.field, e.message))
                        .collect::<Vec<_>>()
                        .join("；")
                )));
            }

            let refs: Vec<&str> = columns.iter().map(|s| s.as_str()).collect();
            let outcome = p.store.append_rows(
                &a.table,
                &refs,
                rows,
                &a.action,
                &summary,
                &a.source_ref,
            )?;

            if cli.json {
                print_json(&serde_json::json!({
                    "schema": "cercis.cli/1",
                    "ok": true,
                    "dry_run": false,
                    "table": a.table,
                    "rows_written": outcome.rows_written,
                    "batch_id": outcome.batch_id,
                    "hash_after": outcome.hash_after,
                    "action": a.action,
                    "source_ref": a.source_ref,
                }));
            } else {
                println!(
                    "已导入 {} 行到表 {}（一次事务、一条流水；批次 {}）",
                    outcome.rows_written, a.table, outcome.batch_id
                );
            }
        }

        Command::Correct(a) => {
            let p = Project::open(&a.path, None)?;
            let id = p.store.correct(
                &a.table, &a.target, &a.field, &a.old, &a.new, &a.reason, &a.source,
            )?;
            if cli.json {
                print_json(&serde_json::json!({
                    "schema": "cercis.cli/1",
                    "ok": true,
                    "correction_id": id,
                }));
            } else {
                println!("已记录更正：{id}（原值永久保留）");
            }
        }

        Command::Log(a) => {
            let p = Project::open(&a.path, None)?;
            let entries = p.store.read_change_log()?;
            let list: Vec<_> = entries.iter().rev().take(a.limit).collect();
            if cli.json {
                print_json(&serde_json::json!({
                    "schema": "cercis.cli/1",
                    "ok": true,
                    "count": list.len(),
                    "entries": list,
                }));
            } else {
                for e in list {
                    println!(
                        "{}  {}  {}  {}  {}",
                        e.ts, e.action, e.target_table, e.target_id, e.summary
                    );
                }
            }
        }

        Command::Note(a) => {
            let p = Project::open(&a.path, None)?;
            if a.action.trim().is_empty() || a.summary.trim().is_empty() {
                return Err(CercisError::Invalid(
                    "note 需要 --action 与 --summary（这条流水说明了什么）".into(),
                ));
            }
            let mut entry = ChangeEntry::new(
                &p.store.identity.user_id,
                &p.store.identity.device_for_log(),
                a.action.trim(),
                &a.table,
                &a.target,
                &a.summary,
                "",
                "",
            );
            entry.source_ref = a.source_ref.clone();
            entry.tool_version = cercis_core::CORE_VERSION.to_string();
            p.store.append_change(&entry)?;

            if cli.json {
                print_json(&serde_json::json!({
                    "schema": "cercis.cli/1",
                    "ok": true,
                    "change_id": entry.change_id,
                    "action": entry.action,
                    "ts": entry.ts,
                }));
            } else {
                println!("已记入流水：{}（{}）", entry.action, entry.summary);
            }
        }

        Command::Checkpoint(a) => {
            let p = Project::open(&a.path, None)?;
            let dir = p.checkpoint(&a.note)?;
            if cli.json {
                print_json(&serde_json::json!({
                    "schema": "cercis.cli/1",
                    "ok": true,
                    "checkpoint": dir.display().to_string(),
                }));
            } else {
                println!("已创建检查点：{}", dir.display());
            }
        }

        Command::Restore(a) => {
            let p = Project::open(&a.path, None)?;
            p.store.restore_object_to_file(&a.hash, &a.to, &a.summary)?;
            if cli.json {
                print_json(&serde_json::json!({
                    "schema": "cercis.cli/1",
                    "ok": true,
                    "restored_to": a.to.display().to_string(),
                    "hash": a.hash,
                }));
            } else {
                println!("已还原到 {}", a.to.display());
            }
        }
    }
    Ok(())
}

/// 多人账户与权限分级（FR-N08/N09）：账户、登录、角色。
fn account_cmd(a: &AccountArgs, json_out: bool) -> Result<(), CercisError> {
    use cercis_core::{accounts, Role};

    match &a.action {
        AccountAction::Add {
            path,
            id,
            name,
            role,
            password_stdin,
            password_file,
            password_env,
            note,
        } => {
            let password = read_password_arg(*password_stdin, password_file, password_env, "口令")?;
            let acc = accounts::add(
                path,
                id,
                name,
                Role::parse(role)?,
                &password,
                cercis_core::crypto::KdfParams::from_env(),
                note,
            )?;
            if json_out {
                print_json(&serde_json::json!({
                    "schema": "cercis.cli/1", "ok": true,
                    "account": { "id": acc.id, "display_name": acc.display_name, "role": acc.role },
                }));
            } else {
                println!("已建账户 {}（{}）", acc.id, acc.role.label_cn());
                println!("  项目里只存加盐哈希，没有明文口令。");
                println!("  现在起这个项目要求登录：cercis account login {} --id {}", path.display(), acc.id);
            }
            Ok(())
        }
        AccountAction::List { path } => {
            let list = accounts::list(path)?;
            let session = accounts::current(path)?;
            if json_out {
                print_json(&serde_json::json!({
                    "schema": "cercis.cli/1",
                    "ok": true,
                    "enabled": !list.is_empty(),
                    "whoami": session.map(|s| serde_json::json!({
                        "id": s.id, "role": s.role, "expires": s.expires,
                    })),
                    "accounts": list.iter().map(|a| serde_json::json!({
                        "id": a.id, "display_name": a.display_name,
                        "role": a.role, "created": a.created, "note": a.note,
                    })).collect::<Vec<_>>(),
                }));
            } else if list.is_empty() {
                println!("这个项目还没有账户（开放模式：不限制）。");
                println!("  要启用多人协作：cercis account add <项目> --id alice --role admin");
            } else {
                println!("账户（{} 个）：", list.len());
                for a in &list {
                    println!(
                        "  {:<12} {:<10} {}{}",
                        a.id,
                        a.role.label_cn(),
                        if a.display_name.is_empty() {
                            String::new()
                        } else {
                            format!("  {}", a.display_name)
                        },
                        if a.note.is_empty() {
                            String::new()
                        } else {
                            format!("  （{}）", a.note)
                        }
                    );
                }
                match session {
                    Some(s) => println!("\n当前身份：{}（{}，到 {}）", s.id, s.role.label_cn(), s.expires),
                    None => println!("\n当前未登录（登录：cercis account login <项目> --id <账户>）"),
                }
            }
            Ok(())
        }
        AccountAction::Login {
            path,
            id,
            password_stdin,
            password_file,
            password_env,
            ttl,
        } => {
            let password = read_password_arg(*password_stdin, password_file, password_env, "口令")?;
            let s = accounts::login(
                path,
                id,
                &password,
                cercis_core::crypto::KdfParams::from_env(),
                *ttl,
            )?;
            if json_out {
                print_json(&serde_json::json!({
                    "schema": "cercis.cli/1", "ok": true,
                    "id": s.id, "role": s.role, "expires": s.expires,
                }));
            } else {
                println!("已登录：{}（{}），到 {}", s.id, s.role.label_cn(), s.expires);
                println!("  登录态存在本机用户目录，不进项目；退出：cercis account logout {}", path.display());
            }
            Ok(())
        }
        AccountAction::Whoami { path } => {
            let session = accounts::current(path)?;
            if json_out {
                print_json(&serde_json::json!({
                    "schema": "cercis.cli/1",
                    "ok": true,
                    "whoami": session.map(|s| serde_json::json!({
                        "id": s.id, "role": s.role, "expires": s.expires,
                    })),
                }));
            } else {
                match session {
                    Some(s) => println!("我是 {}（{}），登录到 {}", s.id, s.role.label_cn(), s.expires),
                    None => println!("当前未登录（开放模式下不影响使用）。"),
                }
            }
            Ok(())
        }
        AccountAction::Logout { path } => {
            let removed = accounts::logout(path)?;
            if json_out {
                print_json(&serde_json::json!({
                    "schema": "cercis.cli/1", "ok": true, "logged_out": removed,
                }));
            } else {
                println!("{}", if removed { "已退出登录。" } else { "本来就没登录。" });
            }
            Ok(())
        }
        AccountAction::Passwd {
            path,
            id,
            old_password_file,
            old_password_env,
            new_password_file,
            new_password_env,
        } => {
            let old = read_password_arg(false, old_password_file, old_password_env, "旧口令")?;
            let new = read_password_arg(false, new_password_file, new_password_env, "新口令")?;
            accounts::change_password(
                path,
                id,
                &old,
                &new,
                cercis_core::crypto::KdfParams::from_env(),
            )?;
            if json_out {
                print_json(&serde_json::json!({
                    "schema": "cercis.cli/1", "ok": true, "id": id,
                }));
            } else {
                println!("已改 {id} 的口令（还是只存哈希）。");
            }
            Ok(())
        }
        AccountAction::Role {
            path,
            id,
            role,
            actor,
        } => {
            let acc = accounts::set_role(path, id, Role::parse(role)?, actor.as_deref())?;
            if json_out {
                print_json(&serde_json::json!({
                    "schema": "cercis.cli/1", "ok": true,
                    "id": acc.id, "role": acc.role,
                }));
            } else {
                println!("已把 {} 设为{}", acc.id, acc.role.label_cn());
            }
            Ok(())
        }
        AccountAction::Remove { path, id, actor } => {
            accounts::remove(path, id, actor.as_deref())?;
            if json_out {
                print_json(&serde_json::json!({
                    "schema": "cercis.cli/1", "ok": true, "removed": id,
                }));
            } else {
                println!("已删除账户 {id}");
            }
            Ok(())
        }
    }
}

/// NAS / 网络位置（FR-N01–N06）：登记、体检、写租约、凭据进钥匙串。
fn remote_cmd(a: &RemoteArgs, json_out: bool) -> Result<(), CercisError> {
    use cercis_core::remote;

    match &a.action {
        RemoteAction::Add {
            path,
            name,
            path_on_nas,
            kind,
            note,
        } => {
            let rec = remote::add(path, name, kind, path_on_nas, note)?;
            if json_out {
                print_json(&serde_json::json!({
                    "schema": "cercis.cli/1", "ok": true, "remote": rec,
                }));
            } else {
                println!("已登记网络位置 {} → {}（{}）", rec.name, rec.path, rec.kind);
                println!("  体检：cercis remote test {} --name {}", path.display(), rec.name);
                println!("  口令请用 remote credential set 存进系统钥匙串，不要写进项目。");
            }
            Ok(())
        }
        RemoteAction::List { path } => {
            let list = remote::list(path)?;
            if json_out {
                print_json(&serde_json::json!({
                    "schema": "cercis.cli/1", "ok": true, "remotes": list,
                }));
            } else if list.is_empty() {
                println!("还没有登记网络位置。");
                println!("  加一个：cercis remote add <项目> --name nas --path /Volumes/lab-nas");
            } else {
                println!("网络位置（{} 个）：", list.len());
                for r in &list {
                    let cred = if r.credential_account.is_empty() {
                        String::new()
                    } else {
                        "  （凭据在钥匙串）".to_string()
                    };
                    let note = if r.note.is_empty() {
                        String::new()
                    } else {
                        format!("  {}", r.note)
                    };
                    println!("  {:<10} {:<42} {:<8}{note}{cred}", r.name, r.path, r.kind);
                }
            }
            Ok(())
        }
        RemoteAction::Remove { path, name } => {
            let rec = remote::remove(path, name)?;
            if json_out {
                print_json(&serde_json::json!({
                    "schema": "cercis.cli/1", "ok": true, "removed": rec.name,
                }));
            } else {
                println!("已移除网络位置 {}（文件一个没动）", rec.name);
            }
            Ok(())
        }
        RemoteAction::Test { path, name } => {
            let target = match name {
                Some(n) => n.clone(),
                None => match remote::list(path)?.first() {
                    Some(r) => r.name.clone(),
                    None => path.display().to_string(),
                },
            };
            let t = remote::test(path, &target)?;
            if json_out {
                print_json(&serde_json::json!({
                    "schema": "cercis.cli/1",
                    "ok": t.reachable && t.readable,
                    "test": t,
                }));
            } else {
                println!("{}", t.summary());
                if t.is_project {
                    println!("  这个位置本身就是一个 Cercis 项目");
                }
                for f in &t.findings {
                    println!("  - {f}");
                }
                for c in &t.conflicts {
                    println!("  ⚠ 分叉迹象：{c}");
                }
            }
            Ok(())
        }
        RemoteAction::Claim {
            path,
            who,
            minutes,
        } => {
            let msg = remote::claim(path, who, *minutes)?;
            if json_out {
                print_json(&serde_json::json!({
                    "schema": "cercis.cli/1", "ok": true, "who": who, "minutes": minutes,
                }));
            } else {
                println!("{msg}");
            }
            Ok(())
        }
        RemoteAction::Release { path, who } => {
            let n = remote::release(path, who)?;
            if json_out {
                print_json(&serde_json::json!({
                    "schema": "cercis.cli/1", "ok": true, "cleared": n,
                }));
            } else {
                println!("已清掉 {n} 个租约（自己的 + 已过期的）");
            }
            Ok(())
        }
        RemoteAction::Credential { action } => match action {
            CredentialAction::Set {
                path,
                name,
                password_stdin,
                password_file,
                password_env,
            } => {
                let secret =
                    read_password_arg(*password_stdin, password_file, password_env, "远程凭据")?;
                let account = remote::credential_set(path, name, &secret)?;
                if json_out {
                    print_json(&serde_json::json!({
                        "schema": "cercis.cli/1", "ok": true,
                        "remote": name, "keychain_account": account,
                    }));
                } else {
                    println!("凭据已存进系统钥匙串：{account}");
                    println!("  项目文件里没有口令（access/remotes.yaml 只记钥匙串账号）。");
                }
                Ok(())
            }
            CredentialAction::Has { path, name } => {
                let has = remote::credential_has(path, name)?;
                if json_out {
                    print_json(&serde_json::json!({
                        "schema": "cercis.cli/1", "ok": true,
                        "remote": name, "stored": has,
                    }));
                } else {
                    println!(
                        "{}",
                        if has {
                            format!("{name} 的凭据在钥匙串里")
                        } else {
                            format!("钥匙串里没有 {name} 的凭据")
                        }
                    );
                }
                Ok(())
            }
            CredentialAction::Delete { path, name } => {
                let removed = remote::credential_delete(path, name)?;
                if json_out {
                    print_json(&serde_json::json!({
                        "schema": "cercis.cli/1", "ok": true,
                        "remote": name, "removed": removed,
                    }));
                } else {
                    println!(
                        "{}",
                        if removed {
                            format!("已删掉 {name} 的凭据")
                        } else {
                            format!("钥匙串里本来就没有 {name} 的凭据")
                        }
                    );
                }
                Ok(())
            }
        },
    }
}

/// 工作副本（FR-N03 / FR-N07）：建、看状态、双向同步。
fn workcopy_cmd(a: &WorkcopyArgs, json_out: bool) -> Result<(), CercisError> {
    use cercis_core::remote;

    match &a.action {
        WorkcopyAction::Create { main, to, name } => {
            let wc = remote::workcopy_create(main, to, name)?;
            if json_out {
                print_json(&serde_json::json!({
                    "schema": "cercis.cli/1", "ok": true, "workcopy": wc,
                }));
            } else {
                println!("已建工作副本：{}", to.display());
                println!("  主副本：{}", wc.main);
                println!("  断网也能记；联网后：cercis workcopy sync {}", to.display());
                println!("  看两边差多少：cercis workcopy status {}", to.display());
            }
            Ok(())
        }
        WorkcopyAction::Status { copy } => {
            let st = remote::workcopy_status(copy)?;
            if json_out {
                print_json(&serde_json::json!({
                    "schema": "cercis.cli/1",
                    "ok": true,
                    "offline": st.verdict == "offline",
                    "status": st,
                }));
            } else {
                println!("{}", st.summary());
                match st.verdict.as_str() {
                    "ok" => println!("  两边一致，不用同步。"),
                    "needs-sync" => println!("  要同步：cercis workcopy sync {}", copy.display()),
                    "has-conflicts" => println!(
                        "  已经同步完了；剩下的是**冲突副本**要人工挑一份（挑完删掉副本即可）"
                    ),
                    _ => println!("  主副本不可达（离线）：本地照记，联网后再同步。"),
                }
                for c in &st.conflicts {
                    println!("  ⚠ 冲突副本待处理：{c}");
                }
                for n in &st.notes {
                    println!("  注：{n}");
                }
            }
            Ok(())
        }
        WorkcopyAction::Sync { copy, dry_run } => {
            let rep = remote::workcopy_sync(copy, *dry_run)?;
            if json_out {
                print_json(&serde_json::json!({
                    "schema": "cercis.cli/1",
                    "ok": true,
                    "dry_run": rep.dry_run,
                    "summary": rep.summary(),
                    "conflicts_total": rep.conflicts_total,
                    "pulled_added": rep.pulled.added,
                    "pushed_added": rep.pushed.added,
                    "pulled_merged": rep.pulled.merged,
                    "pushed_merged": rep.pushed.merged,
                    "notes": rep.notes,
                }));
            } else {
                println!("{}", rep.summary());
                if rep.dry_run {
                    println!("（试运行：两边都没改）");
                }
                for m in &rep.pulled.merged {
                    println!("  拉回来：{} 新增 {} 行", m.table, m.added_rows);
                }
                for m in &rep.pushed.merged {
                    println!("  推上去：{} 新增 {} 行", m.table, m.added_rows);
                }
                for n in &rep.notes {
                    println!("  注：{n}");
                }
            }
            Ok(())
        }
    }
}

/// 差异对比（FR-M05）：记录级（字段改了什么）或快照级（多了/少了哪些行）。
fn diff_cmd(a: &DiffArgs, json_out: bool) -> Result<(), CercisError> {
    use cercis_core::diff;

    match (&a.table, &a.id, &a.from, &a.to) {
        // 记录级
        (Some(table), Some(id), _, _) => {
            let d = diff::record_diff(&a.path, table, id)?;
            if json_out {
                print_json(&serde_json::json!({
                    "schema": "cercis.cli/1",
                    "ok": true,
                    "kind": "record",
                    "summary": d.summary(),
                    "diff": d,
                }));
            } else {
                println!("{}", diff::render_record(&d));
            }
            Ok(())
        }
        // 快照级
        (_, _, Some(from), Some(to)) => {
            let d = diff::snapshot_diff(&a.path, from, to)?;
            if json_out {
                print_json(&serde_json::json!({
                    "schema": "cercis.cli/1",
                    "ok": true,
                    "kind": "snapshot",
                    "summary": d.summary(),
                    "diff": d,
                }));
            } else if a.only_changes {
                println!("{}", diff::render_snapshot(&d));
            } else {
                println!("{}", d.summary());
                for l in &d.lines {
                    println!("{} {}", l.op, l.text);
                }
                for n in &d.notes {
                    println!("注：{n}");
                }
            }
            Ok(())
        }
        _ => Err(CercisError::Invalid(
            "要给出要比什么：\n\
             - 一条记录：cercis diff <项目> --table records --id R-1\n\
             - 两份快照：cercis diff <项目> --from <老指纹> --to <新指纹>\n\
             （指纹从 `cercis log` 或 `cercis manifest-cmd` 里拿）"
                .into(),
        )),
    }
}

/// 共存体检（S13 / FR-I06）：云盘 / Git 目录里的分叉风险与已经分叉的迹象。
fn sync_cmd(a: &SyncArgs, json_out: bool) -> Result<(), CercisError> {
    let rep = cercis_core::sync::check(&a.path)?;
    if json_out {
        print_json(&serde_json::json!({
            "schema": "cercis.cli/1",
            "ok": rep.verdict != "needs-merge",
            "verdict": rep.verdict,
            "cloud": rep.cloud,
            "git": rep.git,
            "findings": rep.findings,
            "notes": rep.notes,
            "summary": rep.summary(),
        }));
    } else {
        println!("{}", rep.summary());
        if !rep.cloud.is_empty() {
            println!("  云盘：{}", rep.cloud);
        }
        if let Some(g) = &rep.git {
            println!(
                "  Git：{}（分支 {}{}）",
                g.repo_root,
                g.branch,
                if g.dirty_files >= 0 {
                    format!("，未提交 {} 个文件", g.dirty_files)
                } else {
                    String::new()
                }
            );
        }
        for f in &rep.findings {
            println!("  - {}（{}）：{}", f.path, f.kind, f.detail);
            println!("      建议：{}", f.advice);
        }
        for n in &rep.notes {
            println!("  注：{n}");
        }
        if rep.verdict == "needs-merge" {
            println!(
                "\n收拢办法：把要保留的一边作为主项目，另一边用 `cercis pack` 打包，\
                 再 `cercis merge <包目录> --into <主项目>`；合并只增不改，两边的新记录都会留下。"
            );
        }
    }
    Ok(())
}

/// 分享守卫（S12 / FR-I04–I05）：白名单 + 真实数据拦截 + 隐私清单。
fn guard_cmd(a: &GuardArgs, json_out: bool) -> Result<(), CercisError> {
    use cercis_core::guard::{self, GuardOptions};

    let opts = GuardOptions {
        allow: a.allow.clone(),
        keywords: a.keywords.clone(),
        scan_privacy: !a.no_privacy,
    };
    let rep = match &a.pack {
        Some(out) => guard::pack_share(&a.path, out, &opts)?,
        None => guard::scan(&a.path, &opts)?,
    };

    if json_out {
        print_json(&serde_json::json!({
            "schema": "cercis.cli/1",
            "ok": rep.is_shareable(),
            "verdict": rep.verdict,
            "synthetic": rep.synthetic,
            "allowed": rep.allowed,
            "to_review": rep.to_review,
            "refused": rep.refused,
            "skipped": rep.skipped,
            "files": rep.files,
            "privacy": rep.privacy,
            "notes": rep.notes,
            "packed_to": a.pack.as_ref().map(|p| p.display().to_string()),
        }));
    } else {
        println!(
            "分享守卫：{}（放行 {} / 待确认 {} / 拒绝 {} / 跳过 {}）",
            match rep.verdict.as_str() {
                "ok" => "可以分享",
                "review" => "需要人工确认",
                _ => "不能分享：发现真实数据类文件",
            },
            rep.allowed,
            rep.to_review,
            rep.refused,
            rep.skipped
        );
        for f in rep.files.iter().filter(|f| f.class == "refuse") {
            println!("  拒绝：{}（{}）", f.path, f.reason);
        }
        for f in rep.files.iter().filter(|f| f.class == "review") {
            println!("  待确认：{}（{}）", f.path, f.reason);
        }
        if !rep.privacy.is_empty() {
            println!("\n隐私线索（请人工过目）：");
            for p in rep.privacy.iter().take(20) {
                println!("  {}：{} → {}", p.path, p.kind, p.sample);
            }
            if rep.privacy.len() > 20 {
                println!("  ……还有 {} 条", rep.privacy.len() - 20);
            }
        }
        for n in &rep.notes {
            println!("注：{n}");
        }
        if let Some(out) = &a.pack {
            println!("\n分享包：{}（清单见 share-manifest.json）", out.display());
        } else {
            println!("\n要直接把白名单内容打成分享包：cercis guard <目录> --pack <输出目录>");
        }
    }
    Ok(())
}

/// 打包带走（S12）：全量 / 增量 / 分卷；加密区域保持密文。
fn pack_cmd(a: &PackArgs, json_out: bool) -> Result<(), CercisError> {
    use cercis_core::pack::{self, PackOptions};

    let opts = PackOptions {
        part_size: a.part_size_mb.saturating_mul(1024 * 1024),
        since: a.since.clone(),
        include_history: !a.no_history,
        with_checkpoints: a.with_checkpoints,
        dry_run: a.dry_run,
    };
    if !a.dry_run {
        std::fs::create_dir_all(&a.out)?;
    }
    let rep = pack::pack(&a.path, &a.out, &opts)?;

    if json_out {
        print_json(&serde_json::json!({
            "schema": "cercis.cli/1",
            "ok": true,
            "dry_run": rep.dry_run,
            "out": rep.out,
            "parts": rep.parts,
            "files": rep.files,
            "bytes": rep.bytes,
            "skipped_unchanged": rep.skipped_unchanged,
            "removed": rep.removed,
            "notes": rep.notes,
            "watermark": {
                "change_count": rep.manifest.watermark.change_count,
                "last_change_at": rep.manifest.watermark.last_change_at,
            },
            "state_hash": rep.manifest.state_hash,
        }));
    } else if a.dry_run {
        println!(
            "试运行：会把 {} 个文件（{} 字节）装进 {}，分成 {} 卷{}",
            rep.files,
            rep.bytes,
            rep.out,
            rep.parts.len(),
            if rep.skipped_unchanged > 0 {
                format!("（其中 {} 个没变、跳过）", rep.skipped_unchanged)
            } else {
                String::new()
            }
        );
    } else {
        println!(
            "已打包：{} 个文件（{} 字节）→ {}，共 {} 卷",
            rep.files,
            rep.bytes,
            rep.out,
            rep.parts.len()
        );
        if rep.parts.len() > 1 {
            println!("  卷：{}", rep.parts.join(" / "));
        }
        if rep.skipped_unchanged > 0 {
            println!("  增量：{} 个文件没变，跳过", rep.skipped_unchanged);
        }
        if !rep.removed.is_empty() {
            println!(
                "  相比上一包，源项目里少了 {} 个文件（只记录，不代删）：{}",
                rep.removed.len(),
                rep.removed.join("、")
            );
        }
        println!("  清单：{}", a.out.join("pack.json").display());
        println!("  变更水位：{} 条（最后一条 {}）", rep.manifest.watermark.change_count, rep.manifest.watermark.last_change_at);
        for n in &rep.notes {
            println!("  注：{n}");
        }
        println!("\n在另一台设备上合并：cercis merge {} --into <那边的项目>", a.out.display());
    }
    Ok(())
}

/// 合并一个包（S12）：先校验，再并集；冲突留双方副本；本地文件永不被覆盖。
fn merge_cmd(a: &MergeArgs, json_out: bool) -> Result<(), CercisError> {
    use cercis_core::pack::{self, MergeOptions};

    let opts = MergeOptions {
        dry_run: a.dry_run,
        member: a.member.clone(),
    };
    let rep = pack::merge(&a.pack, &a.into, &opts)?;

    if json_out {
        print_json(&serde_json::json!({
            "schema": "cercis.cli/1",
            "ok": true,
            "dry_run": rep.dry_run,
            "project": rep.project_name,
            "verified": rep.verified,
            "added": rep.added,
            "merged": rep.merged,
            "conflicts": rep.conflicts,
            "unchanged": rep.unchanged,
            "refused": rep.refused,
            "removed_reported": rep.removed_reported,
            "report_path": rep.report_path,
            "notes": rep.notes,
            "summary": rep.summary(),
        }));
    } else {
        println!("{}", rep.summary());
        if rep.dry_run {
            println!("（试运行：磁盘上没有改动）");
        }
        for m in &rep.merged {
            println!(
                "  并集：{} 新增 {} 行{}{}",
                m.table,
                m.added_rows,
                if m.added_columns.is_empty() {
                    String::new()
                } else {
                    format!("，新增列 {}", m.added_columns.join("、"))
                },
                if m.encrypted { "（加密区域）" } else { "" }
            );
            if !m.key_conflicts.is_empty() {
                println!(
                    "    ⚠ 同一编号有两份不同内容（都留着，请人工核对）：{}",
                    m.key_conflicts.join("、")
                );
            }
        }
        for c in &rep.conflicts {
            println!("  冲突：{} → 对方那份留成 {}", c.path, c.wrote);
        }
        for r in &rep.refused {
            println!("  没动：{r}");
        }
        if !rep.removed_reported.is_empty() {
            println!(
                "  对方删掉了 {} 个文件（本工具没有替你删）：{}",
                rep.removed_reported.len(),
                rep.removed_reported.join("、")
            );
        }
        if !rep.report_path.is_empty() {
            println!("\n报告：{}", rep.report_path);
        }
        for n in &rep.notes {
            println!("注：{n}");
        }
    }
    Ok(())
}

/// 加密共享与分权（S11）：成员、区域、解锁、透明读写、解密导出。
fn share_cmd(a: &ShareArgs, json_out: bool) -> Result<(), CercisError> {
    use cercis_core::access::{self, Permission};

    // `--home` 只影响本次进程（会话与钥匙串账号都放这里）
    if let Some(dir) = &a.home {
        std::env::set_var("CERCIS_SHARE_HOME", dir);
    }

    match &a.action {
        ShareAction::List { path, member } => {
            let root = path.clone();
            require_project(&root)?;
            let members = access::list_members(&root)?;
            let session = access::resolve_session(&root, member.as_deref())?;
            let who = member
                .clone()
                .or_else(|| session.as_ref().map(|s| s.member.clone()));
            let zones = access::list_zones(&root, who.as_deref())?;
            if json_out {
                print_json(&serde_json::json!({
                    "schema": "cercis.cli/1",
                    "ok": true,
                    "project": root.display().to_string(),
                    "as_member": who,
                    "unlocked_members": access::unlocked_members(&root),
                    "share_home": access::share_home().display().to_string(),
                    "keychain_available": cercis_core::keychain::available(),
                    "members": members.iter().map(|m| serde_json::json!({
                        "id": m.id,
                        "display_name": m.display_name,
                        "public_key_fingerprint": m.public_key.chars().take(8).collect::<String>(),
                        "created": m.created,
                        "note": m.note,
                    })).collect::<Vec<_>>(),
                    "zones": zones.iter().map(|z| serde_json::json!({
                        "name": z.name,
                        "label": z.label,
                        "paths": z.paths,
                        "generation": z.generation,
                        "key_fingerprint": z.key_fingerprint,
                        "members": z.members.iter().map(|m| serde_json::json!({
                            "id": m.id,
                            "permission": m.permission.as_str(),
                        })).collect::<Vec<_>>(),
                        "my_permission": who.as_deref().and_then(|w| z.permission_of(w)).map(|p| p.as_str().to_string()),
                    })).collect::<Vec<_>>(),
                }));
            } else {
                println!("成员（{} 人）：", members.len());
                if members.is_empty() {
                    println!("  还没有成员。新建：cercis share member-add <项目> --id alice --name 张老师");
                }
                for m in &members {
                    println!(
                        "  {:<12} {:<12} 公钥 {}…  建于 {}{}",
                        m.id,
                        if m.display_name.is_empty() { "—" } else { &m.display_name },
                        &m.public_key[..8.min(m.public_key.len())],
                        m.created,
                        if m.note.is_empty() {
                            String::new()
                        } else {
                            format!("（{}）", m.note)
                        }
                    );
                }
                println!("\n加密区域（{} 个）：", zones.len());
                if zones.is_empty() {
                    println!("  还没有加密区域。建一个：cercis share zone-create <项目> --name patent-data --path 'methods/配方/**' --member alice:admin");
                }
                for z in &zones {
                    println!(
                        "  {:<16} {:<14} 第 {} 代密钥（{}）  {} 条路径规则",
                        z.name,
                        if z.label.is_empty() { "—" } else { &z.label },
                        z.generation,
                        z.key_fingerprint,
                        z.paths.len()
                    );
                    for m in &z.members {
                        println!("      - {}：{}", m.id, m.permission.label_cn());
                    }
                }
                println!("\n会话：{}", access::share_home().display());
                let unlocked = access::unlocked_members(&root);
                if unlocked.is_empty() {
                    println!("  当前未解锁（加密区域里的内容现在读不了）");
                } else {
                    println!("  已解锁成员：{}", unlocked.join(" / "));
                }
                println!(
                    "  系统钥匙串：{}",
                    if cercis_core::keychain::available() {
                        "可用（unlock --remember 可记住口令）"
                    } else {
                        "不可用（每次输入口令，或用 --password-file）"
                    }
                );
                println!("  修复命令：share apply（补加密新文件）、share rewrap（统一密钥世代）");
            }
            Ok(())
        }

        ShareAction::Status { path, member } => {
            let root = path.clone();
            require_project(&root)?;
            let st = access::status(&root, member.as_deref())?;
            let issues = access::doctor_issues(&root)?;
            if json_out {
                print_json(&serde_json::json!({
                    "schema": "cercis.cli/1",
                    "ok": true,
                    "members": st.members.len(),
                    "unlocked_as": st.session_member,
                    "share_home": st.share_home,
                    "keychain_available": st.keychain_available,
                    "zones": st.zones.iter().map(|z| serde_json::json!({
                        "zone": z.zone,
                        "label": z.label,
                        "generation": z.generation,
                        "key_fingerprint": z.key_fingerprint,
                        "files": z.files,
                        "bytes": z.bytes,
                        "permission": z.permission.map(|p| p.as_str().to_string()),
                        "members": z.members.iter().map(|(id, p)| serde_json::json!({
                            "id": id, "permission": p.as_str(),
                        })).collect::<Vec<_>>(),
                        "plaintext_left": z.plaintext_left,
                        "unreadable": z.unreadable,
                    })).collect::<Vec<_>>(),
                    "issues": issues,
                }));
            } else if st.zones.is_empty() {
                println!("本项目没有加密区域（默认不加密：CSV 保持开放，R / AI 可直读）。");
                println!("要加密某部分内容：cercis share zone-create …");
            } else {
                println!(
                    "加密状态（成员 {} 人，{}）：",
                    st.members.len(),
                    match &st.session_member {
                        Some(m) => format!("当前以 {m} 解锁"),
                        None => "当前未解锁".to_string(),
                    }
                );
                for z in &st.zones {
                    println!(
                        "  {}  第 {} 代密钥（{}）  {} 个文件 / {} 字节{}",
                        if z.label.is_empty() {
                            z.zone.clone()
                        } else {
                            format!("{}（{}）", z.label, z.zone)
                        },
                        z.generation,
                        z.key_fingerprint,
                        z.files,
                        z.bytes,
                        match z.permission {
                            Some(p) => format!("  我的权限：{}", p.label_cn()),
                            None => String::new(),
                        }
                    );
                    if !z.plaintext_left.is_empty() {
                        println!(
                            "      ⚠ 还有 {} 个文件没加密（明文在磁盘上）：{}",
                            z.plaintext_left.len(),
                            z.plaintext_left.join("、")
                        );
                    }
                    if z.unreadable > 0 {
                        println!(
                            "      ⚠ 有 {} 个密文用当前钥匙解不开（多半是换密钥后没重加密）：修复命令 cercis share rewrap <项目> --zone {}",
                            z.unreadable, z.zone
                        );
                    }
                }
                if !issues.is_empty() {
                    println!("\n需要处理：");
                    for i in &issues {
                        println!("  - {i}");
                    }
                }
            }
            Ok(())
        }

        ShareAction::MemberAdd {
            path,
            id,
            name,
            password_stdin,
            password_file,
            password_env,
            note,
        } => {
            let root = path.clone();
            require_project(&root)?;
            let password = read_password_arg(*password_stdin, password_file, password_env, "口令")?;
            let record = access::create_member(
                &root,
                id,
                name,
                &password,
                cercis_core::crypto::KdfParams::from_env(),
                note,
            )?;
            if json_out {
                print_json(&serde_json::json!({
                    "schema": "cercis.cli/1",
                    "ok": true,
                    "member": {
                        "id": record.id,
                        "display_name": record.display_name,
                        "public_key": record.public_key,
                        "key_file": record.key_file,
                        "created": record.created,
                    },
                    "note": "私钥已用该成员的口令（Argon2id）加密后存进项目；口令本身没有写进项目。",
                }));
            } else {
                println!("已新建成员 {}（{}）", record.id, record.display_name);
                println!("  公钥：{}…", &record.public_key[..8.min(record.public_key.len())]);
                println!("  私钥文件：{}（用他的口令加密过）", record.key_file);
                println!("  提示：口令忘了无法找回——只能删掉这个身份，重新授权一个新的。");
            }
            Ok(())
        }

        ShareAction::Passwd {
            path,
            id,
            old_password_file,
            old_password_env,
            new_password_file,
            new_password_env,
        } => {
            let root = path.clone();
            require_project(&root)?;
            let old = read_password_arg(false, old_password_file, old_password_env, "旧口令")?;
            let new = read_password_arg(false, new_password_file, new_password_env, "新口令")?;
            access::change_password(
                &root,
                id,
                &old,
                &new,
                cercis_core::crypto::KdfParams::from_env(),
            )?;
            if json_out {
                print_json(&serde_json::json!({
                    "schema": "cercis.cli/1",
                    "ok": true,
                    "member": id,
                    "note": "私钥换了口令保护；区域密钥与数据都没动。",
                }));
            } else {
                println!("已更新成员 {id} 的口令（区域密钥与数据都没动）。");
            }
            Ok(())
        }

        ShareAction::ZoneCreate {
            path,
            name,
            label,
            paths,
            members,
            no_apply,
            keep_plaintext,
        } => {
            let root = path.clone();
            require_project(&root)?;
            let grants = parse_grants(members)?;
            let (zone, report) = access::create_zone(
                &root,
                name,
                label,
                paths,
                &grants,
                !*no_apply,
                *keep_plaintext,
            )?;
            if json_out {
                print_json(&serde_json::json!({
                    "schema": "cercis.cli/1",
                    "ok": true,
                    "zone": {
                        "name": zone.name,
                        "label": zone.label,
                        "paths": zone.paths,
                        "generation": zone.generation,
                        "key_fingerprint": zone.key_fingerprint,
                        "members": zone.members.iter().map(|m| serde_json::json!({
                            "id": m.id, "permission": m.permission.as_str(),
                        })).collect::<Vec<_>>(),
                    },
                    "encrypted": report.as_ref().map(|r| r.encrypted.clone()).unwrap_or_default(),
                    "kept_plaintext": report.as_ref().map(|r| r.kept_plaintext.clone()).unwrap_or_default(),
                }));
            } else {
                println!("已建立加密区域 {}（第 1 代密钥，指纹 {}）", zone.display(), zone.key_fingerprint);
                println!("  路径规则：{}", zone.paths.join("、"));
                println!("  成员：{}", zone.member_ids().join(" / "));
                match &report {
                    Some(r) if r.encrypted.is_empty() => {
                        println!("  当前没有文件命中这些规则（以后新写入的文件会自动加密）。");
                    }
                    Some(r) => {
                        println!(
                            "  已加密 {} 个文件（{} 字节）{}",
                            r.encrypted.len(),
                            r.bytes,
                            if r.kept_plaintext.is_empty() {
                                "，明文副本已移除"
                            } else {
                                "，明文副本按要求保留"
                            }
                        );
                        for f in &r.encrypted {
                            println!("      - {f} → {f}.enc");
                        }
                    }
                    None => println!("  按要求只写了声明，文件还没加密（之后：cercis share apply）。"),
                }
                println!("\n下一步：cercis share unlock <项目> --member <你>（解锁后才能像平常一样读它）");
            }
            Ok(())
        }

        ShareAction::Grant {
            path,
            zone,
            member,
            permission,
            actor,
        } => {
            let root = path.clone();
            require_project(&root)?;
            let perm = Permission::parse(permission)?;
            let z = access::grant(&root, zone, member, perm, actor.as_deref())?;
            if json_out {
                print_json(&serde_json::json!({
                    "schema": "cercis.cli/1",
                    "ok": true,
                    "zone": z.name,
                    "members": z.members.iter().map(|m| serde_json::json!({
                        "id": m.id, "permission": m.permission.as_str(),
                    })).collect::<Vec<_>>(),
                }));
            } else {
                println!("已在区域 {} 里把 {member} 设为{}", z.display(), perm.label_cn());
                if perm != Permission::Read {
                    println!("  注：新加的成员只拿到当前世代的密钥，看不到更早的内容。");
                }
            }
            Ok(())
        }

        ShareAction::Revoke {
            path,
            zone,
            member,
            dry_run,
            actor,
        } => {
            let root = path.clone();
            require_project(&root)?;
            if *dry_run {
                let zones = access::list_zones(&root, None)?;
                let z = zones
                    .iter()
                    .find(|z| &z.name == zone)
                    .ok_or_else(|| CercisError::NotFound(format!("区域不存在: {zone}")))?;
                let files = access::zone_files(&root, z)
                    .into_iter()
                    .filter(|rel| access::enc_path(&root.join(rel)).exists())
                    .count();
                let msg = format!(
                    "试运行：会从区域 {} 移除 {member}，换新一代密钥并重新加密 {files} 个文件；\
                     被移除者从此刻起读不到新内容（他之前看过的无法追回）",
                    z.display()
                );
                if json_out {
                    print_json(&serde_json::json!({
                        "schema": "cercis.cli/1", "ok": true,
                        "dry_run": true, "zone": z.name, "member": member, "files": files,
                    }));
                } else {
                    println!("{msg}");
                }
                return Ok(());
            }
            let rep = access::revoke(&root, zone, member, actor.as_deref())?;
            if json_out {
                print_json(&serde_json::json!({
                    "schema": "cercis.cli/1",
                    "ok": true,
                    "zone": rep.zone,
                    "removed": rep.member,
                    "generation": rep.generation,
                    "key_fingerprint": rep.key_fingerprint,
                    "reencrypted": rep.reencrypted,
                    "remaining": rep.remaining,
                }));
            } else {
                println!(
                    "已从区域 {zone} 移除 {member}：换到第 {} 代密钥（{}），重新加密 {} 个文件。",
                    rep.generation, rep.key_fingerprint, rep.reencrypted
                );
                println!("  剩下成员：{}", rep.remaining.join(" / "));
                println!("  提醒：他之前已经看到过的内容无法追回；如需事后补救，请更换真正的敏感信息（如口令、配方编号）。");
            }
            Ok(())
        }

        ShareAction::Apply {
            path,
            zone,
            dry_run,
            keep_plaintext,
            actor,
        } => {
            let root = path.clone();
            require_project(&root)?;
            let reports = access::apply(
                &root,
                zone.as_deref(),
                actor.as_deref(),
                *dry_run,
                *keep_plaintext,
            )?;
            if json_out {
                print_json(&serde_json::json!({
                    "schema": "cercis.cli/1",
                    "ok": true,
                    "dry_run": dry_run,
                    "zones": reports.iter().map(|r| serde_json::json!({
                        "zone": r.zone,
                        "encrypted": r.encrypted,
                        "kept_plaintext": r.kept_plaintext,
                        "already": r.already,
                        "bytes": r.bytes,
                    })).collect::<Vec<_>>(),
                }));
            } else if reports.is_empty() {
                println!("没有需要处理的区域。");
            } else {
                for r in &reports {
                    if r.encrypted.is_empty() {
                        println!(
                            "区域 {}：没有新的明文文件（已有 {} 个密文文件）。",
                            r.zone, r.already
                        );
                    } else if *dry_run {
                        println!(
                            "试运行：区域 {} 会加密 {} 个文件（{} 字节）：{}",
                            r.zone,
                            r.encrypted.len(),
                            r.bytes,
                            r.encrypted.join("、")
                        );
                    } else {
                        println!(
                            "区域 {}：已加密 {} 个文件（{} 字节）{}",
                            r.zone,
                            r.encrypted.len(),
                            r.bytes,
                            if r.kept_plaintext.is_empty() {
                                "，明文已移除"
                            } else {
                                "，明文副本保留"
                            }
                        );
                    }
                }
            }
            Ok(())
        }

        ShareAction::Rewrap { path, zone, actor } => {
            let root = path.clone();
            require_project(&root)?;
            let (rewritten, _failed, fingerprint) = access::rewrap(&root, zone, actor.as_deref())?;
            if json_out {
                print_json(&serde_json::json!({
                    "schema": "cercis.cli/1",
                    "ok": true,
                    "zone": zone,
                    "rewritten": rewritten,
                    "key_fingerprint": fingerprint,
                }));
            } else {
                println!(
                    "区域 {zone}：已用当前密钥（{fingerprint}）重写 {rewritten} 个密文文件。"
                );
            }
            Ok(())
        }

        ShareAction::Unlock {
            path,
            member,
            password_stdin,
            password_file,
            password_env,
            keychain,
            ttl,
            remember,
        } => {
            let root = path.clone();
            require_project(&root)?;
            let report = if *keychain {
                match access::unlock_with_keychain(&root, member, *ttl)? {
                    Some(r) => r,
                    None => {
                        return Err(CercisError::Invalid(format!(
                            "钥匙串里没有 {member} 的口令。\n\
                             - 第一次请先：cercis share unlock <项目> --member {member} --remember\n\
                             - 或改用 --password-file / --password-env"
                        )))
                    }
                }
            } else {
                let password =
                    read_password_arg(*password_stdin, password_file, password_env, "口令")?;
                access::unlock(&root, member, &password, *ttl, *remember)?
            };
            if json_out {
                print_json(&serde_json::json!({
                    "schema": "cercis.cli/1",
                    "ok": true,
                    "member": report.member,
                    "expires": report.expires,
                    "session_file": report.session_file,
                    "zones": report.zones.iter().map(|z| serde_json::json!({
                        "name": z.name,
                        "permission": z.permission.as_str(),
                        "key_fingerprint": z.key_fingerprint,
                    })).collect::<Vec<_>>(),
                    "notes": report.notes,
                }));
            } else {
                println!("已解锁：{}", report.member);
                if report.zones.is_empty() {
                    println!("  （他目前不在任何加密区域里，所以没有拿到密钥）");
                }
                for z in &report.zones {
                    println!(
                        "  - {:<16} {}  密钥指纹 {}",
                        z.name,
                        z.permission.label_cn(),
                        z.key_fingerprint
                    );
                }
                println!("  会话到期：{}", report.expires);
                println!("  会话文件：{}（0600，只在本机，不进项目）", report.session_file);
                for n in &report.notes {
                    println!("  注：{n}");
                }
                if report.zones.iter().any(|z| z.permission.can_write()) {
                    println!("\n现在起，区域内的表可以像普通表一样读写了（透明解密/加密）。");
                } else {
                    println!("\n你在这几个区域里是只读：能看、能导出，改数据需要管理员给你 write 权限。");
                }
            }
            Ok(())
        }

        ShareAction::Lock {
            path,
            member,
            forget,
        } => {
            let root = path.clone();
            require_project(&root)?;
            let locked = access::lock(&root, member.as_deref(), *forget)?;
            if json_out {
                print_json(&serde_json::json!({
                    "schema": "cercis.cli/1",
                    "ok": true,
                    "locked": locked,
                    "forgot_keychain": forget,
                }));
            } else if locked.is_empty() {
                println!("本来就没有解锁的会话。");
            } else {
                println!("已上锁：{}", locked.join(" / "));
                if *forget {
                    println!("  钥匙串里记住的口令也已删除。");
                }
            }
            Ok(())
        }

        ShareAction::Cat {
            path,
            file,
            member,
        } => {
            let root = path.clone();
            require_project(&root)?;
            match access::read_through_zone(&root, member.as_deref(), file)? {
                Some(bytes) => {
                    if json_out {
                        print_json(&serde_json::json!({
                            "schema": "cercis.cli/1",
                            "ok": true,
                            "file": file,
                            "bytes": bytes.len(),
                            "text": String::from_utf8_lossy(&bytes),
                        }));
                    } else {
                        use std::io::Write;
                        std::io::stdout().write_all(&bytes)?;
                    }
                }
                None => {
                    return Err(CercisError::Invalid(format!(
                        "「{file}」不在任何加密区域里（普通文件直接打开就行，不需要 share cat）"
                    )))
                }
            }
            Ok(())
        }

        ShareAction::Clean {
            path,
            zone,
            dry_run,
        } => {
            let root = path.clone();
            require_project(&root)?;
            let (n, bytes, files) = access::clean_exports(&root, zone.as_deref(), *dry_run)?;
            if json_out {
                print_json(&serde_json::json!({
                    "schema": "cercis.cli/1",
                    "ok": true,
                    "dry_run": dry_run,
                    "files": files,
                    "bytes": bytes,
                }));
            } else if n == 0 {
                println!("exports/ 里没有明文副本（本来就很干净）。");
            } else if *dry_run {
                println!("试运行：会删掉 {n} 个解密导出的明文副本（{bytes} 字节）：");
                for f in &files {
                    println!("  - {f}");
                }
            } else {
                println!("已清掉 {n} 个明文副本（{bytes} 字节）——加密区域又只剩密文了。");
            }
            Ok(())
        }

        ShareAction::Export {
            path,
            zone,
            to,
            dry_run,
            member,
        } => {
            let root = path.clone();
            require_project(&root)?;
            let rep = access::export_zone(&root, zone, member.as_deref(), to.as_deref(), *dry_run)?;
            if json_out {
                print_json(&serde_json::json!({
                    "schema": "cercis.cli/1",
                    "ok": true,
                    "dry_run": rep.dry_run,
                    "zone": rep.zone,
                    "member": rep.member,
                    "dest": rep.dest,
                    "files": rep.files,
                    "bytes": rep.bytes,
                }));
            } else if *dry_run {
                println!(
                    "试运行：区域 {} 会解密导出 {} 个文件（{} 字节）到 {}",
                    rep.zone,
                    rep.files.len(),
                    rep.bytes,
                    rep.dest
                );
            } else {
                println!(
                    "已解密导出 {} 个文件（{} 字节）到 {}（以 {} 的身份）",
                    rep.files.len(),
                    rep.bytes,
                    rep.dest,
                    rep.member
                );
                println!("  注意：导出的明文副本在项目 exports/ 下，用完记得清理；这次导出已记入流水。");
                println!("  清理：cercis share clean {} --zone {}", path.display(), zone);
            }
            Ok(())
        }
    }
}

fn require_project(root: &Path) -> Result<(), CercisError> {
    if root.join("project.yaml").exists() {
        Ok(())
    } else {
        Err(CercisError::NotFound(format!(
            "不是 Cercis 项目（缺少 project.yaml）: {}",
            root.display()
        )))
    }
}

/// 口令来源：`--password-env` > `--password-file` > 标准输入读一行。
fn read_password_arg(
    stdin_flag: bool,
    file: &Option<PathBuf>,
    env_name: &Option<String>,
    what: &str,
) -> Result<String, CercisError> {
    if let Some(name) = env_name {
        let name = name.trim();
        match std::env::var(name) {
            Ok(v) if !v.is_empty() => return Ok(v),
            _ => {
                return Err(CercisError::Invalid(format!(
                    "环境变量 {name} 没设置或为空（{what}要从这里读）"
                )))
            }
        }
    }
    if let Some(f) = file {
        let text = std::fs::read_to_string(f)?;
        let s = text.trim_end_matches(['\n', '\r']).to_string();
        if s.is_empty() {
            return Err(CercisError::Invalid(format!("口令文件是空的: {}", f.display())));
        }
        return Ok(s);
    }
    let _ = stdin_flag;
    eprintln!("请输入{what}（直接粘贴或输入，然后回车；屏幕上会显示）:");
    let mut buf = String::new();
    std::io::stdin().read_line(&mut buf)?;
    let s = buf.trim_end_matches(['\n', '\r']).to_string();
    if s.is_empty() {
        return Err(CercisError::Invalid(format!("{what}不能为空")));
    }
    Ok(s)
}

/// 解析 `--member alice:admin` / `--member bob`（默认只读）。
fn parse_grants(items: &[String]) -> Result<Vec<(String, cercis_core::access::Permission)>, CercisError> {
    use cercis_core::access::Permission;
    let mut out = Vec::new();
    for raw in items {
        for part in raw.split(',') {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            let (id, perm) = match part.split_once(':') {
                Some((id, p)) => (id.trim().to_string(), Permission::parse(p)?),
                None => (part.to_string(), Permission::Read),
            };
            out.push((id, perm));
        }
    }
    Ok(out)
}

fn print_json(v: &serde_json::Value) {
    println!(
        "{}",
        serde_json::to_string_pretty(v).unwrap_or_else(|_| "{}".into())
    );
}

/// 插件市场：预置市场列表、清单校验、从已下载目录安装、卸载（内核不联网）。
fn market_cmd(a: &MarketArgs, json_out: bool) -> Result<(), CercisError> {

            let global_dir = match &a.global_dir {
                Some(d) => d.clone(),
                None => default_global_plugin_dir()?,
            };
            match &a.action {
                // 市场清单：预置条目 + 已装插件（下载在 app，内核只认"已经下载好的东西"）
                MarketAction::List => {
                    let installs = cercis_core::Installs::load(&global_dir)?;
                    let user = cercis_core::UserMarkets::load(&global_dir)?;
                    let effective = user.effective();
                    let markets: Vec<serde_json::Value> = effective
                        .iter()
                        .map(|m| {
                            serde_json::json!({
                                "id": m.id,
                                "label_cn": m.label_cn,
                                "repository": m.repository,
                                "marketplace_path": m.marketplace_path,
                                "default_ref": m.default_ref,
                                "bundled": m.bundled,
                                "removable": !m.bundled,
                            })
                        })
                        .collect();
                    if json_out {
                        print_json(&serde_json::json!({
                            "schema": "cercis.cli/1",
                            "ok": true,
                            "global_dir": global_dir.display().to_string(),
                            "markets": markets,
                            "removed_presets": user.removed_presets,
                            "installed": installs.records,
                        }));
                    } else {
                        println!("市场（市场就是 Cercis 管理插件的方式：来源、版本、装卸都在这里）：");
                        for m in &effective {
                            println!(
                                "  {:<9} {:<18} {}{}  {}",
                                m.id,
                                if m.label_cn.is_empty() { "—" } else { &m.label_cn },
                                m.repository,
                                if m.default_ref.is_empty() {
                                    String::new()
                                } else {
                                    format!("（{}）", m.default_ref)
                                },
                                if m.bundled {
                                    "随包交付：不可移除，插件只能停用"
                                } else {
                                    "可移除"
                                }
                            );
                        }
                        if !user.removed_presets.is_empty() {
                            println!(
                                "  （已移除的预置市场：{}；找回来：market add --preset <id>）",
                                user.removed_presets.join(" / ")
                            );
                        }
                        println!("\n用户插件目录：{}", global_dir.display());
                        if installs.records.is_empty() {
                            println!("已装（来自市场）：无。科研插件默认不预装——想用就装。");
                        } else {
                            println!("已装（来自市场）：");
                            for r in &installs.records {
                                println!(
                                    "  {}  {}  {}  {}{}",
                                    r.id,
                                    r.version,
                                    r.repository,
                                    &r.fingerprint[..8.min(r.fingerprint.len())],
                                    if r.bundled { "（随包：不可删除）" } else { "" }
                                );
                            }
                        }
                    }
                    Ok(())
                }
                MarketAction::Add {
                    repository,
                    id,
                    label,
                    path,
                    git_ref,
                    preset,
                } => {
                    let mut user = cercis_core::UserMarkets::load(&global_dir)?;
                    if !preset.trim().is_empty() {
                        user.restore_preset(preset.trim())?;
                        user.save(&global_dir)?;
                        let msg = format!("已找回预置市场 {}", preset.trim());
                        if json_out {
                            print_json(&serde_json::json!({
                                "schema": "cercis.cli/1",
                                "ok": true,
                                "action": "market.add",
                                "preset": preset.trim(),
                                "message": msg,
                            }));
                        } else {
                            println!("{msg}");
                        }
                        return Ok(());
                    }
                    if repository.trim().is_empty() {
                        return Err(CercisError::Invalid(
                            "要么给 --repository <git 地址>，要么用 --preset <id> 找回预置市场".into(),
                        ));
                    }
                    let id = if id.trim().is_empty() {
                        repository
                            .trim_end_matches('/')
                            .rsplit('/')
                            .next()
                            .unwrap_or("market")
                            .trim_end_matches(".git")
                            .to_string()
                    } else {
                        id.trim().to_string()
                    };
                    let entry = cercis_core::market::MarketEntry {
                        id: id.clone(),
                        label_cn: if label.trim().is_empty() {
                            id.clone()
                        } else {
                            label.trim().to_string()
                        },
                        repository: repository.trim().to_string(),
                        marketplace_path: path.trim().to_string(),
                        default_ref: git_ref.trim().to_string(),
                        bundled: false,
                        added_at: cercis_core::ids::now_iso8601(),
                    };
                    user.add(entry.clone());
                    user.save(&global_dir)?;
                    if json_out {
                        print_json(&serde_json::json!({
                            "schema": "cercis.cli/1",
                            "ok": true,
                            "action": "market.add",
                            "market": entry,
                        }));
                    } else {
                        println!("已添加市场 {}（{}）", entry.id, entry.repository);
                        println!("私有仓库的鉴权在 app 侧处理；内核只做校验与安装。");
                    }
                    Ok(())
                }
                MarketAction::Remove { id } => {
                    let mut user = cercis_core::UserMarkets::load(&global_dir)?;
                    let message = user.remove(id)?;
                    user.save(&global_dir)?;
                    if json_out {
                        print_json(&serde_json::json!({
                            "schema": "cercis.cli/1",
                            "ok": true,
                            "action": "market.remove",
                            "id": id,
                            "message": message,
                        }));
                    } else {
                        println!("{message}");
                    }
                    Ok(())
                }
                MarketAction::Validate { file } => {
                    let market = cercis_core::Marketplace::load(file)?;
                    if json_out {
                        print_json(&serde_json::json!({
                            "schema": "cercis.cli/1",
                            "ok": true,
                            "marketplace": market,
                        }));
                    } else {
                        println!("清单合法：{}", market.summary());
                        for p in &market.plugins {
                            println!(
                                "  {}  v{}  {}  接口 {}",
                                p.id, p.version, p.path, p.requires_plugin_api
                            );
                        }
                    }
                    Ok(())
                }
                MarketAction::Install {
                    from,
                    plugin,
                    market,
                    git_ref,
                    dry_run,
                } => {
                    // 仓库里必须有清单；按清单找到插件的目录
                    let listing = from.join("marketplace.json");
                    let alt = from.join("marketplace.yaml");
                    let listing = if listing.is_file() {
                        listing
                    } else if alt.is_file() {
                        alt
                    } else {
                        return Err(CercisError::NotFound(format!(
                            "{} 里没有 marketplace.json——请先让 app 下载仓库（或手动 clone）",
                            from.display()
                        )));
                    };
                    let market_data = cercis_core::Marketplace::load(&listing)?;
                    let entry = market_data.plugin(plugin).ok_or_else(|| {
                        CercisError::NotFound(format!(
                            "市场 {} 里没有插件 {plugin}（可用：{}）",
                            market_data.summary(),
                            market_data
                                .plugins
                                .iter()
                                .map(|p| p.id.as_str())
                                .collect::<Vec<_>>()
                                .join(" / ")
                        ))
                    })?;
                    let src = from.join(&entry.path);
                    if !src.is_dir() {
                        // 手写 marketplace.json 最容易错的就是插件路径：把仓库实际结构列出来
                        let listing: Vec<String> = std::fs::read_dir(from)
                            .map(|it| {
                                it.flatten()
                                    .filter(|e| e.path().is_dir() && e.file_name() != ".git")
                                    .map(|e| format!("{}/", e.file_name().to_string_lossy()))
                                    .collect()
                            })
                            .unwrap_or_default();
                        return Err(CercisError::NotFound(format!(
                            "清单里写的插件路径是 {}，但仓库里没有这个目录（仓库根下有：{}）",
                            entry.path,
                            if listing.is_empty() {
                                "（空）".to_string()
                            } else {
                                listing.join(" ")
                            }
                        )));
                    }
                    if *dry_run {
                        if json_out {
                            print_json(&serde_json::json!({
                                "schema": "cercis.cli/1",
                                "ok": true,
                                "dry_run": true,
                                "plugin": entry.id,
                                "version": entry.version,
                                "from": src.display().to_string(),
                                "target": global_dir.join(&entry.id).display().to_string(),
                            }));
                        } else {
                            println!(
                                "试运行：会装 {} v{}（{} → {}）",
                                entry.id,
                                entry.version,
                                src.display(),
                                global_dir.join(&entry.id).display()
                            );
                        }
                        return Ok(());
                    }
                    let repo = if market_data.repository.is_empty() {
                        from.display().to_string()
                    } else {
                        market_data.repository.clone()
                    };
                    // D42：随包市场（bundled）里的插件装进来也是"随包插件"——不能删除，只能停用
                    let (record, dest) = cercis_core::market::install_from_dir(
                        &src,
                        &global_dir,
                        market,
                        &repo,
                        git_ref,
                        market_data.bundled,
                    )?;
                    if json_out {
                        print_json(&serde_json::json!({
                            "schema": "cercis.cli/1",
                            "ok": true,
                            "action": "market.install",
                            "record": record,
                            "path": dest.display().to_string(),
                        }));
                    } else {
                        println!(
                            "已安装 {} v{}（{}）",
                            record.id, record.version, dest.display()
                        );
                        println!("指纹：{}（内容一变，信任自动失效）", record.fingerprint);
                        println!(
                            "下一步：在 app 里启用（或用 `cercis plugins <项目> --global-dir {}` 查看）",
                            global_dir.display()
                        );
                    }
                    Ok(())
                }
                MarketAction::Uninstall { id, dry_run, note } => {
                    let (_, message) =
                        cercis_core::market::uninstall(&global_dir, id, *dry_run, note)?;
                    if json_out {
                        print_json(&serde_json::json!({
                            "schema": "cercis.cli/1",
                            "ok": true,
                            "dry_run": dry_run,
                            "id": id,
                            "message": message,
                        }));
                    } else {
                        println!("{message}");
                    }
                    Ok(())
                }
                MarketAction::Check { from, only_updates } => {
                    let listing = if from.join("marketplace.json").is_file() {
                        from.join("marketplace.json")
                    } else {
                        from.join("marketplace.yaml")
                    };
                    let market_data = cercis_core::Marketplace::load(&listing)?;
                    let global_dir = global_dir.clone();
                    let installs = cercis_core::Installs::load(&global_dir)?;
                    let mut items = cercis_core::market::check_updates(&installs, &market_data);
                    if *only_updates {
                        items.retain(|i| i.actionable());
                    }
                    if json_out {
                        print_json(&serde_json::json!({
                            "schema": "cercis.cli/1",
                            "ok": true,
                            "market": market_data.summary(),
                            "updates": items,
                        }));
                    } else if items.is_empty() {
                        println!("没有可检查的插件（这个市场里没有你装过的插件）。");
                    } else {
                        println!("更新检查：{}（只提示，不自动升级）", market_data.summary());
                        for i in &items {
                            let line = match i.status.as_str() {
                                "update_available" => format!(
                                    "↑ {}：{} → {}{}",
                                    i.id,
                                    i.installed_version,
                                    i.available_version,
                                    if i.needs_reconfirmation {
                                        "（**权限有变化：升级前要重新确认**）"
                                    } else {
                                        ""
                                    }
                                ),
                                "up_to_date" => format!("= {}：已是最新（{}）", i.id, i.installed_version),
                                "newer_installed" => format!(
                                    "· {}：本地版本更新（{} > {}），不提示降级",
                                    i.id, i.installed_version, i.available_version
                                ),
                                "incompatible" => format!(
                                    "! {}：市场里的新版本需要内核接口 {}（当前支持到 {}），先升级内核",
                                    i.id,
                                    i.requires_plugin_api,
                                    cercis_core::plugin::PLUGIN_API_VERSION
                                ),
                                _ => format!("? {}：市场里没有这个插件了（已装 {}）", i.id, i.installed_version),
                            };
                            println!("  {line}");
                        }
                        let n = items.iter().filter(|i| i.actionable()).count();
                        if n > 0 {
                            println!(
                                "\n共 {n} 个可更新。升级命令：`cercis market install --from {} --plugin <id> --ref <ref>`",
                                from.display()
                            );
                        }
                    }
                    Ok(())
                }
                MarketAction::Restore {
                    id,
                    from,
                    market,
                    git_ref,
                } => {
                    // 给了目录就按市场清单找到插件真正的位置（仓库根 → plugins/<id>），
                    // 否则会拿仓库根当插件目录，报"清单不存在"
                    let (resolved, bundled, repo_url) = match from {
                        Some(root) => {
                            let listing = if root.join("marketplace.json").is_file() {
                                root.join("marketplace.json")
                            } else {
                                root.join("marketplace.yaml")
                            };
                            let market_data = cercis_core::Marketplace::load(&listing)?;
                            let entry = market_data.plugin(id).ok_or_else(|| {
                                CercisError::NotFound(format!(
                                    "市场 {} 里没有插件 {id}",
                                    market_data.summary()
                                ))
                            })?;
                            let mut src = root.join(&entry.path);
                            if !src.is_dir() {
                                src = root.clone();
                            }
                            (
                                Some(src),
                                market_data.bundled,
                                market_data.repository.clone(),
                            )
                        }
                        None => (None, false, String::new()),
                    };
                    let (_, message, dest) = cercis_core::market::restore(
                        &global_dir,
                        id,
                        resolved.as_deref(),
                        &repo_url,
                        market,
                        git_ref,
                        bundled,
                    )?;
                    if json_out {
                        print_json(&serde_json::json!({
                            "schema": "cercis.cli/1",
                            "ok": true,
                            "id": id,
                            "message": message,
                            "path": dest.map(|d| d.display().to_string()),
                        }));
                    } else {
                        println!("{message}");
                    }
                    Ok(())
                }
            }
        }


/// 用户级插件目录（市场安装的插件放这里）：`CERCIS_GLOBAL_PLUGINS` > `~/.cercis/plugins`。
fn global_dir_plugins() -> Result<PathBuf, CercisError> {
    default_global_plugin_dir()
}

/// 用户级插件目录（市场安装的插件放这里）：`--global-dir` > `CERCIS_GLOBAL_PLUGINS` > `~/.cercis/plugins`。
fn default_global_plugin_dir() -> Result<PathBuf, CercisError> {
    if let Some(d) = std::env::var_os("CERCIS_GLOBAL_PLUGINS") {
        return Ok(PathBuf::from(d));
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| CercisError::Invalid("取不到 HOME，无法定位用户插件目录".into()))?;
    Ok(home.join(".cercis").join("plugins"))
}

/// 强度写成中文（值类型清单用）。
fn severity_cn(severity: &str) -> &'static str {
    match severity {
        "error" => "错误（阻止写入）",
        "warning" => "提醒（放行）",
        _ => "不校验",
    }
}

/// 置信度写成"最高匹配 integer 99%（99/100 格）"这样。
fn confidence_text(profile: &cercis_core::infer::Profile) -> String {
    let best = profile
        .candidates
        .iter()
        .filter(|c| c.domain != "text")
        .max_by(|a, b| {
            a.confidence
                .partial_cmp(&b.confidence)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
    match best {
        Some(c) => format!(
            "最高匹配 {} {:.0}%（{}/{} 格）",
            c.domain,
            c.confidence * 100.0,
            c.matched,
            profile.stats.non_empty
        ),
        None => "没有候选类型".into(),
    }
}

/// 反例那一行：优先给**建议类型**下的反例（那才是"改类型前该看一眼"的格子）。
fn counterexamples_text(profile: &cercis_core::infer::Profile) -> Option<String> {
    let target = profile
        .recommended
        .as_deref()
        .and_then(|r| profile.candidates.iter().find(|c| c.domain == r))
        .or_else(|| {
            profile
                .candidates
                .iter()
                .find(|c| !c.counterexamples.is_empty())
        })?;
    if target.counterexamples.is_empty() {
        return None;
    }
    Some(
        target
            .counterexamples
            .iter()
            .map(|v| format!("{v}（不符 {}）", target.domain))
            .collect::<Vec<_>>()
            .join("、"),
    )
}

/// 把 `--type/--range/...` 组装成字段补丁。
fn build_field_patch(
    a: &SetFieldTypeArgs,
) -> Result<cercis_core::template::FieldPatch, CercisError> {
    let range = match &a.range {
        None => None,
        Some(spec) => {
            let (lo, hi) = spec.split_once(',').ok_or_else(|| {
                CercisError::Invalid(format!(
                    "范围要写成 `最小值,最大值`（可留空一端，如 `0,` 或 `,250`）：{spec}"
                ))
            })?;
            let parse = |s: &str| -> Result<Option<f64>, CercisError> {
                let s = s.trim();
                if s.is_empty() {
                    return Ok(None);
                }
                s.parse::<f64>()
                    .map(Some)
                    .map_err(|_| CercisError::Invalid(format!("范围里的数读不出来：{s}")))
            };
            Some(vec![parse(lo)?, parse(hi)?])
        }
    };
    Ok(cercis_core::template::FieldPatch {
        field_type: a.field_type.clone(),
        required: if a.required {
            Some(true)
        } else if a.optional {
            Some(false)
        } else {
            None
        },
        unit: a.unit.clone(),
        range,
        clear_range: a.clear_range,
        allowed_values: a.allowed_values.clone(),
        pattern: a.pattern.clone(),
        precision: a.precision,
        clear_precision: a.clear_precision,
        reference: a.reference.clone(),
        notes: a.notes.clone(),
    })
}

/// 两条校验结论是不是同一条（用于算"这次改动新增了哪些问题"）。
fn same_issue(a: &cercis_core::Issue, b: &cercis_core::Issue) -> bool {
    a.kind == b.kind
        && a.table == b.table
        && a.row == b.row
        && a.field == b.field
        && a.message == b.message
}

/// 用内核的类型规则检查一批**即将写入**的行（§5.9.3 的第一个校验时点：写入前）。
///
/// 只检查模板里声明过的列：模板之外的列（`--allow-new-columns`）没有类型可依据，
/// 直接跳过——**放行不猜**。
/// 读一个数据文件（CSV/TSV/JSON，或 `-` 读标准输入）用于类型推断，保留列顺序。
fn read_table_for_inference(from: &str) -> Result<(Vec<String>, Vec<Row>), CercisError> {
    let rows = read_rows_input(from)?;
    let headers: Vec<String> = if from == "-" {
        // 标准输入只有 JSON：列顺序取各行键名字典序（BTreeMap 已经排好）
        let mut keys: Vec<String> = Vec::new();
        for r in &rows {
            for k in r.keys() {
                if !keys.contains(k) {
                    keys.push(k.clone());
                }
            }
        }
        keys
    } else {
        let path = Path::new(from);
        let ext = path
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        if matches!(ext.as_str(), "csv" | "tsv" | "txt") {
            let delimiter = if ext == "tsv" { b'\t' } else { b',' };
            let mut rdr = csv::ReaderBuilder::new()
                .has_headers(true)
                .flexible(true)
                .delimiter(delimiter)
                .from_path(path)?;
            rdr.headers()?
                .iter()
                .map(|h| h.trim_start_matches('\u{feff}').trim().to_string())
                .collect()
        } else {
            let mut keys: Vec<String> = Vec::new();
            for r in &rows {
                for k in r.keys() {
                    if !keys.contains(k) {
                        keys.push(k.clone());
                    }
                }
            }
            keys
        }
    };
    Ok((headers, rows))
}

/// 从"已经读进来的行"推类型（不涉及项目模板）。
fn infer_from_rows(headers: &[String], rows: &[Row]) -> Vec<cercis_core::infer::ColumnInference> {
    let cols: Vec<&str> = headers.iter().map(|s| s.as_str()).collect();
    let mut table = cercis_core::Table::in_memory("/tmp/cercis-infer-input.csv", &cols);
    for r in rows {
        table.push(r.clone());
    }
    let mut out = cercis_core::infer::infer_table(&table, &[]);
    for c in out.iter_mut() {
        // 没有模板可对照：声明类型留空，建议就是"该用什么类型"
        c.declared_type = String::new();
        c.suggest_change = c.profile.recommended.is_some();
    }
    out
}

fn check_cells_against_template(
    template: &cercis_core::Template,
    table: &str,
    rows: &[Row],
) -> (Vec<cercis_core::Issue>, Vec<cercis_core::Issue>) {
    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    let Some(td) = template.table(table) else {
        return (errors, warnings);
    };
    for (i, row) in rows.iter().enumerate() {
        for f in &td.fields {
            let Some(cell) = row.get(&f.name) else {
                continue;
            };
            for finding in cercis_core::value::check_value(f.domain(), &f.constraints(), cell) {
                let issue = cercis_core::Issue {
                    level: finding.severity.as_str().to_string(),
                    kind: finding.kind.to_string(),
                    table: table.to_string(),
                    row: i + 1,
                    field: f.name.clone(),
                    message: finding.message,
                };
                match finding.severity {
                    cercis_core::Severity::Error => errors.push(issue),
                    cercis_core::Severity::Warning => warnings.push(issue),
                }
            }
        }
    }
    (errors, warnings)
}

/// AI 宿主的技能目录（技能包要装到这里才会被自动加载）。
fn skill_host_dir(host: &str) -> Result<PathBuf, CercisError> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| CercisError::Invalid("取不到 HOME，无法定位技能目录".into()))?;
    match host {
        "codex" => Ok(home.join(".codex").join("skills")),
        "claude" => Ok(home.join(".claude").join("skills")),
        other => Err(CercisError::Invalid(format!(
            "未知的宿主 {other}（可用：codex / claude；其他宿主请用 --out <目录>）"
        ))),
    }
}

/// 读取批量输入：CSV/TSV 文件、JSON 数组文件、`-`（标准输入读 JSON）或内联 JSON。
fn read_rows_input(from: &str) -> Result<Vec<Row>, CercisError> {
    if from == "-" {
        let mut buf = String::new();
        std::io::stdin().read_to_string(&mut buf)?;
        return parse_json_rows(&buf, "<stdin>");
    }

    let path = Path::new(from);
    if path.is_file() {
        let ext = path
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        return match ext.as_str() {
            "csv" => read_delimited_rows(path, b','),
            "tsv" => read_delimited_rows(path, b'\t'),
            _ => {
                let text = std::fs::read_to_string(path)?;
                parse_json_rows(&text, from)
            }
        };
    }

    let trimmed = from.trim_start();
    if trimmed.starts_with('[') || trimmed.starts_with('{') {
        return parse_json_rows(from, "<inline>");
    }
    Err(CercisError::NotFound(format!("输入文件不存在: {from}")))
}

/// 解析分隔符文本（首行为表头）。
fn read_delimited_rows(path: &Path, delimiter: u8) -> Result<Vec<Row>, CercisError> {
    let mut rdr = csv::ReaderBuilder::new()
        .has_headers(true)
        .flexible(true)
        .delimiter(delimiter)
        .from_path(path)?;

    let headers: Vec<String> = rdr
        .headers()?
        .iter()
        .map(|s| s.trim().to_string())
        .collect();
    if headers.is_empty() || headers.iter().any(|h| h.is_empty()) {
        return Err(CercisError::Invalid(format!(
            "{} 的表头为空或存在空列名",
            path.display()
        )));
    }

    let mut rows = Vec::new();
    for (i, rec) in rdr.records().enumerate() {
        let rec = rec.map_err(|e| {
            CercisError::Invalid(format!("{} 第 {} 行解析失败: {e}", path.display(), i + 2))
        })?;
        let mut row = Row::new();
        for (j, col) in headers.iter().enumerate() {
            row.insert(col.clone(), rec.get(j).unwrap_or("").to_string());
        }
        rows.push(row);
    }
    Ok(rows)
}

/// 解析 JSON：数组（每个元素是对象）或单个对象；标量一律转成字符串。
fn parse_json_rows(text: &str, source: &str) -> Result<Vec<Row>, CercisError> {
    let value: serde_json::Value = serde_json::from_str(text)
        .map_err(|e| CercisError::Invalid(format!("{source} 不是合法 JSON: {e}")))?;
    let items = match value {
        serde_json::Value::Array(items) => items,
        obj @ serde_json::Value::Object(_) => vec![obj],
        other => {
            return Err(CercisError::Invalid(format!(
                "{source} 需要 JSON 数组或对象，实际是 {other}"
            )))
        }
    };

    let mut rows = Vec::new();
    for (i, item) in items.iter().enumerate() {
        let obj = item.as_object().ok_or_else(|| {
            CercisError::Invalid(format!("{source} 第 {} 项不是 JSON 对象", i + 1))
        })?;
        let mut row = Row::new();
        for (k, v) in obj {
            let cell = match v {
                serde_json::Value::String(s) => s.clone(),
                serde_json::Value::Null => String::new(),
                other => other.to_string(),
            };
            row.insert(k.clone(), cell);
        }
        rows.push(row);
    }
    Ok(rows)
}

fn hostname_or(fallback: &str) -> String {
    std::env::var("HOSTNAME")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| fallback.to_string())
}
