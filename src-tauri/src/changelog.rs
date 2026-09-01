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
            version: "0.1.4".to_string(),
            title: "0.1.4".to_string(),
            changes: vec![
                "新增「设置」菜单:可选择使用 npx 还是 pnpm dlx 启动,并选择 DSH 版本通道(latest / next / alpha)。".to_string(),
                "插件安装失败时自动卸载失败的插件,避免残留损坏的半安装。".to_string(),
                "「关于」新增「更新日志」,可查看当前及历史版本的更新记录。".to_string(),
                "进入主窗口后异步检查新版本,发现新版本时在菜单栏提示并支持一键进入关于页更新。".to_string(),
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
