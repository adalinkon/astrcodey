//! 文本格式化工具：单行摘要生成。

/// 把任意文本压成单行摘要，超长时尾部追加 `…`。
///
/// 行为：
/// - 折叠所有空白序列为单个空格（与 `text.split_whitespace().join(" ")` 等价）。
/// - 按字符数（非字节数）截断到 `max_chars`；超出时附加 `…`（U+2026）。
/// - 长度计算基于 Unicode 标量值（`char`），对 ASCII 与 CJK 行为一致；
///   不做字形宽度感知，需要对齐显示宽度时调用方应另行处理。
///
/// 用于把工具调用参数、命令行、用户输入等折叠成可放进单行 UI 的预览。
pub fn compact_inline(text: &str, max_chars: usize) -> String {
    let compact = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.chars().count() <= max_chars {
        return compact;
    }

    let mut preview = compact.chars().take(max_chars).collect::<String>();
    preview.push('…');
    preview
}
