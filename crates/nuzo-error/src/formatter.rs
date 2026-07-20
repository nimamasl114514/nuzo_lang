//! 诊断输出格式化器 -- 统一颜色、宽度、样式管理
//!
//! 提供终端自适应宽度和 ANSI 颜色支持，不支持颜色时自动降级为纯文本。
//!
//! # 核心能力
//!
//! - **终端宽度自适应**：通过环境变量 `COLUMNS` 或默认值，clamp 到 [MIN_WIDTH, MAX_WIDTH]
//! - **颜色自动降级**：非交互式终端（管道/重定向）自动禁用颜色
//! - **统一样式字典**：所有诊断输出共享同一套颜色/样式规范，确保视觉一致性
//! - **零外部依赖**：纯标准库实现，不引入 console/termcolor 等第三方 crate
//!
//! # 使用示例
//!
//! ```rust
//! use nuzo_error::DiagnosticFormatter;
//!
//! let fmt = DiagnosticFormatter::new();
//! println!("{}", fmt.error_style().apply_to("类型不匹配"));
//! println!("{}", fmt.separator());
//! ```

use crate::types::ErrorSeverity;
use std::fmt;

// ============================================================================
// 常量：宽度边界
// ============================================================================

/// 报告最小宽度（低于此值可读性极差）
const MIN_WIDTH: usize = 60;

/// 报告最大宽度（超过此值在宽屏终端上阅读体验下降）
const MAX_WIDTH: usize = 120;

/// 默认宽度（无法检测终端宽度时的回退值）
const DEFAULT_WIDTH: usize = 72;

// ============================================================================
// ANSI 转义码常量
// ============================================================================

/// 重置所有样式
const RESET: &str = "\x1b[0m";

/// 粗体
const BOLD: &str = "\x1b[1m";

/// 暗淡/暗灰
const DIM: &str = "\x1b[2m";

/// 红色前景
const FG_RED: &str = "\x1b[31m";

/// 绿色前景
const FG_GREEN: &str = "\x1b[32m";

/// 黄色前景
const FG_YELLOW: &str = "\x1b[33m";

/// 蓝色前景
const FG_BLUE: &str = "\x1b[34m";

/// 青色前景
const FG_CYAN: &str = "\x1b[36m";

// ============================================================================
// AnsiStyle -- 轻量级 ANSI 样式封装
// ============================================================================

/// ANSI 样式封装
///
/// 通过组合前景色和文本属性（粗体/暗淡），生成 ANSI 转义序列。
/// 当 `active == false` 时，`apply_to` 直接透传文本，不添加任何转义码。
///
/// # 设计决策
///
/// 不使用 `console::Style` 或 `termcolor`，因为：
/// 1. nuzo_error 是底层 crate，应尽量减少外部依赖
/// 2. vendor 目录离线管理，新增依赖同步成本高
/// 3. 所需功能（前景色 + 粗体/暗淡）用 6 个 ANSI 码即可覆盖
#[derive(Debug, Clone)]
pub struct AnsiStyle {
    /// 是否激活样式（颜色关闭时为 false）
    active: bool,
    /// 前景色 ANSI 码
    fg: Option<&'static str>,
    /// 文本属性 ANSI 码（粗体/暗淡）
    attr: Option<&'static str>,
}

impl AnsiStyle {
    /// 创建无样式（透传模式）
    fn none() -> Self {
        Self { active: false, fg: None, attr: None }
    }

    /// 创建带前景色的样式
    fn fg(color: &'static str) -> Self {
        Self { active: true, fg: Some(color), attr: None }
    }

    /// 创建带前景色 + 粗体的样式
    fn fg_bold(color: &'static str) -> Self {
        Self { active: true, fg: Some(color), attr: Some(BOLD) }
    }

    /// 创建带暗淡属性的样式
    fn dim() -> Self {
        Self { active: true, fg: None, attr: Some(DIM) }
    }

    /// 将样式应用到文本，返回带 ANSI 码的字符串
    ///
    /// 当 `active == false` 时直接返回原始文本。
    pub fn apply_to(&self, text: impl AsRef<str>) -> StyledText {
        let text = text.as_ref();
        if !self.active {
            StyledText { raw: text.to_owned(), styled: text.to_owned() }
        } else {
            let mut styled = String::with_capacity(text.len() + 16);
            // 先应用属性（粗体/暗淡），再应用前景色
            if let Some(attr) = self.attr {
                styled.push_str(attr);
            }
            if let Some(fg) = self.fg {
                styled.push_str(fg);
            }
            styled.push_str(text);
            styled.push_str(RESET);

            StyledText { raw: text.to_owned(), styled }
        }
    }
}

// ============================================================================
// StyledText -- 带样式的文本
// ============================================================================

/// 带样式的文本
///
/// 同时保存原始文本和带 ANSI 码的文本，方便不同场景使用。
#[derive(Debug, Clone)]
pub struct StyledText {
    /// 原始文本（不含 ANSI 码）
    raw: String,
    /// 带样式的文本（含 ANSI 码）
    styled: String,
}

impl StyledText {
    /// 获取原始文本（不含 ANSI 码）
    pub fn raw(&self) -> &str {
        &self.raw
    }

    /// 获取带样式的文本（含 ANSI 码）
    pub fn styled(&self) -> &str {
        &self.styled
    }
}

impl fmt::Display for StyledText {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.styled)
    }
}

impl AsRef<str> for StyledText {
    fn as_ref(&self) -> &str {
        &self.styled
    }
}

// ============================================================================
// DiagnosticFormatter
// ============================================================================

/// 诊断输出格式化器
///
/// 封装终端宽度检测、颜色开关和样式方法，为所有诊断输出提供统一的视觉风格。
///
/// # 颜色降级
///
/// 当输出目标不是交互式终端（如管道/重定向）时，自动禁用颜色。
/// 通过 `std::io::stderr().is_terminal()` 检测（Rust 1.70+ 稳定）。
///
/// # 终端宽度检测
///
/// 优先读取环境变量 `COLUMNS`，失败则使用默认值 72。
/// 最终 clamp 到 [60, 120] 范围内。
#[derive(Debug, Clone)]
pub struct DiagnosticFormatter {
    /// 报告宽度（clamp 到 [MIN_WIDTH, MAX_WIDTH]）
    width: usize,
    /// 是否启用颜色
    colorize: bool,
}

impl DiagnosticFormatter {
    // ========================================================================
    // 构造器
    // ========================================================================

    /// 创建新的格式化器（自动检测终端宽度和颜色支持）
    ///
    /// 终端宽度通过环境变量 `COLUMNS` 获取，clamp 到 [60, 120]；
    /// 颜色支持通过 `std::io::stderr().is_terminal()` 检测。
    pub fn new() -> Self {
        let width = Self::detect_terminal_width();
        let colorize = Self::detect_color_support();
        Self { width, colorize }
    }

    /// 创建禁用颜色的格式化器（用于测试或管道输出）
    ///
    /// 宽度仍自动检测，但颜色强制关闭。
    pub fn no_color() -> Self {
        Self { width: Self::detect_terminal_width(), colorize: false }
    }

    /// 创建指定宽度且禁用颜色的格式化器（用于确定性测试）
    pub fn no_color_with_width(width: usize) -> Self {
        Self { width: width.clamp(MIN_WIDTH, MAX_WIDTH), colorize: false }
    }

    /// 返回一个宽度为指定值（已 clamp 到 [MIN_WIDTH, MAX_WIDTH]）的新格式化器，颜色设置保持不变。
    pub fn with_width(self, width: usize) -> Self {
        Self { width: width.clamp(MIN_WIDTH, MAX_WIDTH), colorize: self.colorize }
    }

    /// 返回一个颜色开关被强制设为指定值的新格式化器，宽度保持不变。
    pub fn with_color(self, colorize: bool) -> Self {
        Self { width: self.width, colorize }
    }

    // ========================================================================
    // 访问器
    // ========================================================================

    /// 获取报告宽度
    pub fn width(&self) -> usize {
        self.width
    }

    /// 是否启用颜色
    pub fn should_colorize(&self) -> bool {
        self.colorize
    }

    // ========================================================================
    // 样式方法（按严重级别）
    // ========================================================================

    /// Fatal 级别样式：粗体红色
    ///
    /// 用于编译器内部一致性被破坏等不可恢复的致命错误。
    pub fn fatal_style(&self) -> AnsiStyle {
        if self.colorize { AnsiStyle::fg_bold(FG_RED) } else { AnsiStyle::none() }
    }

    /// Error 级别样式：红色
    ///
    /// 用于用户代码逻辑错误等需要立即关注的问题。
    pub fn error_style(&self) -> AnsiStyle {
        if self.colorize { AnsiStyle::fg(FG_RED) } else { AnsiStyle::none() }
    }

    /// Warning 级别样式：黄色
    ///
    /// 用于可能有问题但不影响运行的情况。
    pub fn warning_style(&self) -> AnsiStyle {
        if self.colorize { AnsiStyle::fg(FG_YELLOW) } else { AnsiStyle::none() }
    }

    /// Info 级别样式：蓝色
    ///
    /// 用于信息提示。
    pub fn info_style(&self) -> AnsiStyle {
        if self.colorize { AnsiStyle::fg(FG_BLUE) } else { AnsiStyle::none() }
    }

    // ========================================================================
    // 样式方法（错误码分类专用 - 均为粗体以便在 header 中突出显示）
    // ========================================================================

    /// 编译期错误码样式：粗体蓝色
    ///
    /// 用于 `C0xxx` 系列错误码（CompileError/ModuleNotFound/CircularImport/DuplicateSymbol）。
    /// 蓝色呼应"编译器"语义，与运行时错误（红色）形成视觉区分。
    pub fn compile_style(&self) -> AnsiStyle {
        if self.colorize { AnsiStyle::fg_bold(FG_BLUE) } else { AnsiStyle::none() }
    }

    /// 内部错误码样式：粗体黄色
    ///
    /// 用于 `I0xxx` 系列错误码（Internal/InvalidBytecodeVersion）。
    /// 黄色提示"这是 VM/编译器 bug，需要用户上报"，与运行时错误（红）和编译期错误（蓝）区分。
    pub fn internal_style(&self) -> AnsiStyle {
        if self.colorize { AnsiStyle::fg_bold(FG_YELLOW) } else { AnsiStyle::none() }
    }

    // ========================================================================
    // 样式方法（通用）
    // ========================================================================

    /// 暗灰色样式（用于框线、分隔线等辅助视觉元素）
    pub fn dim_style(&self) -> AnsiStyle {
        if self.colorize { AnsiStyle::dim() } else { AnsiStyle::none() }
    }

    /// 成功/绿色样式（用于修复建议）
    pub fn success_style(&self) -> AnsiStyle {
        if self.colorize { AnsiStyle::fg(FG_GREEN) } else { AnsiStyle::none() }
    }

    /// 青色样式（用于源码位置）
    pub fn cyan_style(&self) -> AnsiStyle {
        if self.colorize { AnsiStyle::fg(FG_CYAN) } else { AnsiStyle::none() }
    }

    // ========================================================================
    // 辅助方法（框线与分隔线）
    // ========================================================================

    /// 生成水平分隔线
    ///
    /// 使用 Unicode 绘图字符 `─`，宽度等于报告宽度。
    pub fn separator(&self) -> String {
        self.dim_style().apply_to("\u{2500}".repeat(self.width)).to_string()
    }

    /// 生成顶部框线
    ///
    /// 使用 Unicode 绘图字符 `═`，宽度等于报告宽度。
    pub fn top_border(&self) -> String {
        self.dim_style().apply_to("\u{2550}".repeat(self.width)).to_string()
    }

    /// 生成底部框线
    ///
    /// 使用 Unicode 绘图字符 `═`，宽度等于报告宽度。
    pub fn bottom_border(&self) -> String {
        self.dim_style().apply_to("\u{2550}".repeat(self.width)).to_string()
    }

    // ========================================================================
    // 辅助方法（段落标题）
    // ========================================================================

    /// 生成段落标题（带 emoji 和分隔线）
    ///
    /// 格式：`{emoji} {title} {─...}`，总宽度等于报告宽度。
    pub fn section_header(&self, emoji: &str, title: &str) -> String {
        // emoji + 空格 + title + 空格 + 分隔线 = 总宽度
        let prefix = format!("{} {} ", emoji, title);
        let remaining = self.width.saturating_sub(prefix.chars().count());
        format!("{}{}", prefix, self.dim_style().apply_to("\u{2500}".repeat(remaining)))
    }

    // ========================================================================
    // 严重级别分发
    // ========================================================================

    /// 按严重级别获取样式
    pub fn severity_style(&self, severity: ErrorSeverity) -> AnsiStyle {
        match severity {
            ErrorSeverity::Fatal => self.fatal_style(),
            ErrorSeverity::Error => self.error_style(),
            ErrorSeverity::Warning => self.warning_style(),
            ErrorSeverity::Info => self.info_style(),
        }
    }

    /// 按严重级别获取 emoji 前缀
    pub fn severity_emoji(&self, severity: ErrorSeverity) -> &'static str {
        match severity {
            ErrorSeverity::Fatal => "\u{1F4A5}",          // 💥
            ErrorSeverity::Error => "\u{274C}",           // ❌
            ErrorSeverity::Warning => "\u{26A0}\u{FE0F}", // ⚠️
            ErrorSeverity::Info => "\u{2139}\u{FE0F}",    // ℹ️
        }
    }

    /// 按严重级别获取中文标签
    pub fn severity_label(&self, severity: ErrorSeverity) -> &'static str {
        match severity {
            ErrorSeverity::Fatal => "致命错误",
            ErrorSeverity::Error => "错误",
            ErrorSeverity::Warning => "警告",
            ErrorSeverity::Info => "信息",
        }
    }

    // ========================================================================
    // 内部检测
    // ========================================================================

    /// 检测终端宽度，clamp 到 [MIN_WIDTH, MAX_WIDTH]
    ///
    /// 优先读取环境变量 `COLUMNS`（大多数终端模拟器会设置），
    /// 解析失败则回退到 `DEFAULT_WIDTH`。
    fn detect_terminal_width() -> usize {
        let detected = std::env::var("COLUMNS")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(DEFAULT_WIDTH);
        detected.clamp(MIN_WIDTH, MAX_WIDTH)
    }

    /// 检测 stderr 是否为交互式终端（支持颜色）
    ///
    /// 使用 `std::io::IsTerminal` trait（Rust 1.70+ 稳定）。
    /// 在非交互式环境（管道/重定向）中自动返回 false。
    fn detect_color_support() -> bool {
        use std::io::IsTerminal;
        std::io::stderr().is_terminal()
    }
}

impl Default for DiagnosticFormatter {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ------------------------------------------------------------------
    // Happy Path
    // ------------------------------------------------------------------

    #[test]
    fn new_formatter_has_valid_width() {
        let fmt = DiagnosticFormatter::new();
        assert!(fmt.width() >= MIN_WIDTH, "宽度不应小于 MIN_WIDTH");
        assert!(fmt.width() <= MAX_WIDTH, "宽度不应大于 MAX_WIDTH");
    }

    #[test]
    fn no_color_formatter_disables_color() {
        let fmt = DiagnosticFormatter::no_color();
        assert!(!fmt.should_colorize(), "no_color 模式应禁用颜色");
    }

    #[test]
    fn no_color_with_width_clamps() {
        let fmt = DiagnosticFormatter::no_color_with_width(30);
        assert_eq!(fmt.width(), MIN_WIDTH, "低于最小值应 clamp 到 MIN_WIDTH");

        let fmt = DiagnosticFormatter::no_color_with_width(200);
        assert_eq!(fmt.width(), MAX_WIDTH, "超过最大值应 clamp 到 MAX_WIDTH");

        let fmt = DiagnosticFormatter::no_color_with_width(80);
        assert_eq!(fmt.width(), 80, "合法宽度应原样保留");
    }

    #[test]
    fn default_equals_new() {
        let a = DiagnosticFormatter::new();
        let b = DiagnosticFormatter::default();
        assert_eq!(a.width(), b.width());
        assert_eq!(a.should_colorize(), b.should_colorize());
    }

    #[test]
    fn severity_style_returns_correct_variant() {
        let fmt = DiagnosticFormatter::no_color();
        // 无颜色模式下所有样式都是 none，验证不会 panic
        let _ = fmt.severity_style(ErrorSeverity::Fatal);
        let _ = fmt.severity_style(ErrorSeverity::Error);
        let _ = fmt.severity_style(ErrorSeverity::Warning);
        let _ = fmt.severity_style(ErrorSeverity::Info);
    }

    #[test]
    fn severity_emoji_not_empty() {
        let fmt = DiagnosticFormatter::no_color();
        assert!(!fmt.severity_emoji(ErrorSeverity::Fatal).is_empty());
        assert!(!fmt.severity_emoji(ErrorSeverity::Error).is_empty());
        assert!(!fmt.severity_emoji(ErrorSeverity::Warning).is_empty());
        assert!(!fmt.severity_emoji(ErrorSeverity::Info).is_empty());
    }

    #[test]
    fn severity_label_chinese() {
        let fmt = DiagnosticFormatter::no_color();
        assert_eq!(fmt.severity_label(ErrorSeverity::Fatal), "致命错误");
        assert_eq!(fmt.severity_label(ErrorSeverity::Error), "错误");
        assert_eq!(fmt.severity_label(ErrorSeverity::Warning), "警告");
        assert_eq!(fmt.severity_label(ErrorSeverity::Info), "信息");
    }

    // ------------------------------------------------------------------
    // 辅助方法
    // ------------------------------------------------------------------

    #[test]
    fn separator_length_matches_width() {
        let fmt = DiagnosticFormatter::no_color_with_width(72);
        // 无颜色模式下，separator 是纯文本，字符数应等于 width
        assert_eq!(fmt.separator().chars().count(), fmt.width());
    }

    #[test]
    fn section_header_contains_emoji_and_title() {
        let fmt = DiagnosticFormatter::no_color_with_width(72);
        let header = fmt.section_header("\u{1F4A5}", "致命错误");
        assert!(header.starts_with("\u{1F4A5} 致命错误"));
    }

    // ------------------------------------------------------------------
    // Edge Case: 宽度 clamp
    // ------------------------------------------------------------------

    #[test]
    #[allow(clippy::assertions_on_constants)] // 编译期常量一致性校验，确保 MIN_WIDTH <= MAX_WIDTH
    fn width_clamp_constants_consistent() {
        assert!(MIN_WIDTH <= MAX_WIDTH);
        assert!((MIN_WIDTH..=MAX_WIDTH).contains(&DEFAULT_WIDTH));
    }

    // ------------------------------------------------------------------
    // Poison Pill: 无颜色模式下样式为空
    // ------------------------------------------------------------------

    #[test]
    fn no_color_styles_are_passthrough() {
        let fmt = DiagnosticFormatter::no_color();
        // 无颜色模式下 apply_to 不改变文本
        let text = "hello";
        assert_eq!(fmt.fatal_style().apply_to(text).to_string(), text);
        assert_eq!(fmt.error_style().apply_to(text).to_string(), text);
        assert_eq!(fmt.warning_style().apply_to(text).to_string(), text);
        assert_eq!(fmt.info_style().apply_to(text).to_string(), text);
        assert_eq!(fmt.dim_style().apply_to(text).to_string(), text);
        assert_eq!(fmt.success_style().apply_to(text).to_string(), text);
        assert_eq!(fmt.cyan_style().apply_to(text).to_string(), text);
    }

    // ------------------------------------------------------------------
    // AnsiStyle 单元测试
    // ------------------------------------------------------------------

    #[test]
    fn ansi_style_none_passthrough() {
        let style = AnsiStyle::none();
        let result = style.apply_to("test");
        assert_eq!(result.raw(), "test");
        assert_eq!(result.styled(), "test");
    }

    #[test]
    fn ansi_style_fg_contains_color_code() {
        let style = AnsiStyle::fg(FG_RED);
        let result = style.apply_to("error");
        assert!(result.styled().starts_with(FG_RED), "应包含红色前景码");
        assert!(result.styled().ends_with(RESET), "应以 RESET 结尾");
        assert_eq!(result.raw(), "error");
    }

    #[test]
    fn ansi_style_fg_bold_contains_both_codes() {
        let style = AnsiStyle::fg_bold(FG_RED);
        let result = style.apply_to("fatal");
        let styled = result.styled();
        assert!(styled.starts_with(BOLD), "应先应用粗体码");
        assert!(styled.contains(FG_RED), "应包含红色前景码");
        assert!(styled.ends_with(RESET), "应以 RESET 结尾");
    }

    #[test]
    fn ansi_style_dim_contains_dim_code() {
        let style = AnsiStyle::dim();
        let result = style.apply_to("dim text");
        assert!(result.styled().starts_with(DIM), "应以暗淡码开头");
        assert!(result.styled().ends_with(RESET), "应以 RESET 结尾");
    }

    // ------------------------------------------------------------------
    // StyledText 测试
    // ------------------------------------------------------------------

    #[test]
    fn styled_text_display_shows_styled() {
        let style = AnsiStyle::fg(FG_GREEN);
        let text = style.apply_to("ok");
        // Display trait 应输出带样式的文本
        let display = format!("{}", text);
        assert_eq!(display, text.styled());
    }

    #[test]
    fn styled_text_as_ref_returns_styled() {
        let style = AnsiStyle::fg(FG_CYAN);
        let text = style.apply_to("pos");
        assert_eq!(text.as_ref(), text.styled());
    }
}
