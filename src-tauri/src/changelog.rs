//! Built-in changelog shown in the 关于 > 更新日志 page.
//!
//! The changelog is authored here so it ships with the binary (no network
//! needed). Each entry lists the version and the user-facing changes for that
//! release; newer versions appear first.

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct ChangelogEntry {
    /// Semver version without the leading `v`.
    pub version: String,
    /// Short human-readable label (e.g. "0.1.3").
    pub title: String,
    /// Bullet list of user-facing changes (newest first within the version).
    pub changes: Vec<String>,
}

/// All known release entries, newest first.
pub fn entries() -> Vec<ChangelogEntry> {
    vec![
        ChangelogEntry {
            version: "0.2.2".to_string(),
            title: "0.2.2".to_string(),
            changes: vec![
                "「运行」菜单新增「清除 DSH 缓存」:可选择清除依赖还是完整清除(保留用户数据),自动结束并重启 DSH。".to_string(),
                "清除缓存时显示带进度条的加载窗口,后台线程执行删除不再卡住界面;完整清除也清理 profiles/node_modules 等真实目录。".to_string(),
                "修复清除缓存进度条卡在 30% 的问题(改为重命名+异步删除,1.2GB 数据不再阻塞进度)。".to_string(),
                "插件列表新增「更新」按钮,可一键把插件升级到最新版本。".to_string(),
                "修复插件冲突误报:缺少包不再被误判为插件冲突,clear-cache 改为同步且可靠。".to_string(),
                "通过命名互斥量强制单实例运行,避免出现重复托盘图标。".to_string(),
                "工具窗口打开时通过 URL ?page= 参数显示正确页面(启动失败后进入插件列表而非安装页)。".to_string(),
            ],
        },
        ChangelogEntry {
            version: "0.2.1".to_string(),
            title: "0.2.1".to_string(),
            changes: vec![
                "修复 macOS/无系统 Node 环境启动失败:内置 Node 目录加入 PATH,DSH 依赖的 postinstall 脚本不再报「node: command not found」。".to_string(),
                "修复 DSH 异步启动失败后「重试启动」按钮仍禁用的问题。".to_string(),
                "更新安装器:覆盖 exe 前等待旧进程完全退出释放文件句柄,避免更新时「Error opening file for writing」。".to_string(),
                "正在下载/安装新版本时禁用「检查更新」按钮,防止重复触发更新。".to_string(),
                "关于页 GitHub 链接改为品牌橙配色,hover 显示下划线。".to_string(),
            ],
        },
        ChangelogEntry {
            version: "0.2.0".to_string(),
            title: "0.2.0".to_string(),
            changes: vec![
                "新增系统托盘:左键单击显示主窗口,右键菜单含设置/查看版本/安装插件/插件列表/导出数据/导入数据/退出。".to_string(),
                "设置新增「关闭主窗口时」:可选择退出程序或隐藏到托盘继续运行(默认隐藏到托盘)。".to_string(),
                "loading 页新增「复制日志」按钮,可一键复制界面显示的日志。".to_string(),
                "修复等待日志刷屏把真实错误信息顶没的问题,错误信息自动置顶可见。".to_string(),
                "DSH 启动因插件冲突失败时自动弹出插件列表页,便于手动卸载问题插件。".to_string(),
                "修复 macOS 全新安装时数据目录不存在导致 DSH 启动失败的问题。".to_string(),
            ],
        },
        ChangelogEntry {
            version: "0.1.5".to_string(),
            title: "0.1.5".to_string(),
            changes: vec![
                "修复 macOS 打包后找不到内置 pnpm/npx 导致 DSH 启动失败的问题。".to_string(),
                "修复 pnpm 启动时因 build scripts 审批交互提示而卡住的问题(改为非交互自动批准)。".to_string(),
                "设置改为 pnpm dlx 启动 + latest 版本通道。".to_string(),
            ],
        },
        ChangelogEntry {
            version: "0.1.4".to_string(),
            title: "0.1.4".to_string(),
            changes: vec![
                "新增「设置」:可选择使用 npx 还是 pnpm dlx 启动,并选择 DSH 版本通道(latest / next / alpha)。".to_string(),
                "插件安装失败时自动卸载失败的插件,避免残留损坏的半安装。".to_string(),
                "「关于」新增「更新日志」,可查看当前及历史版本的更新记录。".to_string(),
                "进入主窗口后异步检查新版本,发现新版本时在菜单栏提示并支持一键进入关于页更新。".to_string(),
                "loading 页退出会彻底结束 DSH 进程并退出应用,不再后台残留。".to_string(),
            ],
        },
        ChangelogEntry {
            version: "0.1.3".to_string(),
            title: "0.1.3".to_string(),
            changes: vec![
                "新增「调试工具」菜单(打开 DevTools)。".to_string(),
                "非主窗口禁止最大化。".to_string(),
                "菜单重命名整理。".to_string(),
            ],
        },
        ChangelogEntry {
            version: "0.1.2".to_string(),
            title: "0.1.2".to_string(),
            changes: vec![
                "修复插件列表作用域包名(@scope/name)解析。".to_string(),
                "进程关闭时通过 Job Object 结束整个子进程树,并在 NSIS 预安装钩子中避免更新时 node.exe 被占用。".to_string(),
            ],
        },
        ChangelogEntry {
            version: "0.1.1".to_string(),
            title: "0.1.1".to_string(),
            changes: vec![
                "桌面启动器:内置 Node + pnpm dlx 启动 DeepSeek Harness。".to_string(),
                "插件管理:安装 / 卸载 / 列表(工具窗口)。".to_string(),
                "配置导出 / 导入(zip 打包 DSH 数据目录)。".to_string(),
                "原生菜单(运行 / 查看 / 关于)与自动更新。".to_string(),
                "使用 npmmirror 源以加速下载,并隔离 DSH_HOME 数据目录。".to_string(),
            ],
        },
    ]
}

/// Tauri command: return the full changelog (newest first).
#[tauri::command]
pub fn get_changelog() -> Vec<ChangelogEntry> {
    entries()
}
