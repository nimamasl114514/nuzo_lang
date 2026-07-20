//! 字符串相似度算法（用于拼写纠错建议）
//!
//! 提供 [`levenshtein`] 距离计算与 [`suggest_similar`] 候选筛选，
//! 供 [`crate::classifier::ErrorClassifier`] 在 `UndefinedVariable` 等错误场景下
//! 生成 "Did you mean X?" 风格的结构化建议。
//!
//! # 算法
//!
//! Levenshtein 距离 = 将 `a` 转换为 `b` 所需的最少单字符操作（插入/删除/替换）数。
//! 使用 O(min(m,n)) 空间的滚动数组 DP 实现。
//!
//! # 示例
//!
//! ```
//! use nuzo_error::similar::{levenshtein, suggest_similar};
//!
//! assert_eq!(levenshtein("kitten", "sitting"), 3);
//! assert_eq!(levenshtein("abc", "abc"), 0);
//!
//! let candidates = vec!["foo".to_string(), "bar".to_string(), "for".to_string()];
//! let result = suggest_similar("for", &candidates, 1, 2);
//! assert_eq!(result, vec!["for".to_string()]);
//! ```

/// 计算两个字符串之间的 Levenshtein 距离（编辑距离）。
///
/// 距离 = 将 `a` 转换为 `b` 所需的最少单字符操作（插入/删除/替换）数。
/// 空字符串与长度为 n 的字符串距离为 n。
///
/// # 复杂度
///
/// - 时间：O(m * n)，其中 m、n 为两字符串字符数
/// - 空间：O(min(m, n))，使用滚动数组
///
/// # 示例
///
/// ```
/// use nuzo_error::similar::levenshtein;
/// assert_eq!(levenshtein("kitten", "sitting"), 3);
/// assert_eq!(levenshtein("abc", "abc"), 0);
/// assert_eq!(levenshtein("", "abc"), 3);
/// ```
pub fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();

    // 让 b 为较短的一方，节省空间
    let (longer, shorter) = if a.len() >= b.len() { (&a, &b) } else { (&b, &a) };
    let m = longer.len();
    let n = shorter.len();

    if n == 0 {
        return m;
    }

    // prev 代表上一行的距离，curr 代表当前行
    let mut prev: Vec<usize> = (0..=n).collect();
    let mut curr: Vec<usize> = vec![0; n + 1];

    for i in 1..=m {
        curr[0] = i;
        for j in 1..=n {
            let cost = if longer[i - 1] == shorter[j - 1] { 0 } else { 1 };
            // min(prev[j] + 1, curr[j-1] + 1, prev[j-1] + cost)
            curr[j] = (prev[j] + 1).min(curr[j - 1] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }

    prev[n]
}

/// 从候选列表中找出与 `target` 最相似的字符串（按 Levenshtein 距离升序）。
///
/// 返回最多 `max_suggestions` 个候选，距离超过 `max_distance` 的候选会被过滤掉。
///
/// # 参数
///
/// - `target` — 待匹配的目标字符串（通常是用户拼错的标识符）
/// - `candidates` — 候选字符串列表（通常是当前作用域的变量名）
/// - `max_suggestions` — 最多返回几个建议
/// - `max_distance` — 允许的最大编辑距离（距离过大的候选视为不相关）
///
/// # 示例
///
/// ```
/// use nuzo_error::similar::suggest_similar;
///
/// let candidates = vec!["foo".to_string(), "bar".to_string(), "for".to_string()];
/// let result = suggest_similar("for", &candidates, 1, 2);
/// assert_eq!(result, vec!["for".to_string()]);
///
/// // 拼错的变量名
/// let vars = vec!["count".to_string(), "counter".to_string(), "total".to_string()];
/// let result = suggest_similar("conut", &vars, 1, 3);
/// assert_eq!(result, vec!["count".to_string()]);
/// ```
pub fn suggest_similar(
    target: &str,
    candidates: &[String],
    max_suggestions: usize,
    max_distance: usize,
) -> Vec<String> {
    // 不区分大小写比较：距离计算时把双方转为小写
    let target_lower = target.to_lowercase();

    let mut scored: Vec<(usize, &String)> = candidates
        .iter()
        .map(|c| {
            let c_lower = c.to_lowercase();
            (levenshtein(&target_lower, &c_lower), c)
        })
        .filter(|(d, _)| *d <= max_distance && *d > 0) // 距离 0 排除（完全相同不需要建议）
        .collect();

    scored.sort_by_key(|(d, _)| *d);

    scored.into_iter().take(max_suggestions).map(|(_, c)| c.clone()).collect()
}

/// 自动计算合理的最大距离阈值。
///
/// 经验法则：距离上限 = 目标长度的 1/3（整数除法，向下取整），最少为 1。
/// 这样短标识符（1-3 字符）允许 1 个差异，长标识符允许更多。
///
/// # 示例
///
/// ```
/// use nuzo_error::similar::default_max_distance;
/// assert_eq!(default_max_distance("a"), 1);
/// assert_eq!(default_max_distance("abc"), 1);
/// assert_eq!(default_max_distance("abcdef"), 2);
/// // 10/3 = 3（整数除法向下取整，非 4）
/// assert_eq!(default_max_distance("abcdefghij"), 3);
/// ```
pub fn default_max_distance(target: &str) -> usize {
    let char_count = target.chars().count();
    (char_count / 3).max(1)
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ------------------------------------------------------------------
    // levenshtein 基础用例
    // ------------------------------------------------------------------

    #[test]
    fn levenshtein_identical_strings() {
        assert_eq!(levenshtein("abc", "abc"), 0);
        assert_eq!(levenshtein("", ""), 0);
        assert_eq!(levenshtein("hello", "hello"), 0);
    }

    #[test]
    fn levenshtein_empty_string() {
        assert_eq!(levenshtein("", "abc"), 3);
        assert_eq!(levenshtein("abc", ""), 3);
        assert_eq!(levenshtein("", ""), 0);
    }

    #[test]
    fn levenshtein_single_substitution() {
        assert_eq!(levenshtein("cat", "bat"), 1);
        assert_eq!(levenshtein("kitten", "sitten"), 1);
    }

    #[test]
    fn levenshtein_single_insertion() {
        assert_eq!(levenshtein("cat", "cats"), 1);
        assert_eq!(levenshtein("act", "acts"), 1);
    }

    #[test]
    fn levenshtein_single_deletion() {
        assert_eq!(levenshtein("cats", "cat"), 1);
        assert_eq!(levenshtein("acts", "act"), 1);
    }

    #[test]
    fn levenshtein_classic_kitten_sitting() {
        // kitten → sitten → sittin → sitting（3 步）
        assert_eq!(levenshtein("kitten", "sitting"), 3);
    }

    #[test]
    fn levenshtein_supports_unicode() {
        // 中文字符按 Unicode scalar 计数，每个汉字算 1
        assert_eq!(levenshtein("你好", "你好"), 0);
        assert_eq!(levenshtein("你好", "你好吗"), 1);
        assert_eq!(levenshtein("你好", "再见"), 2);
    }

    #[test]
    fn levenshtein_case_sensitive() {
        // 大小写敏感：'A' 与 'a' 不同
        assert_eq!(levenshtein("ABC", "abc"), 3);
    }

    // ------------------------------------------------------------------
    // suggest_similar
    // ------------------------------------------------------------------

    #[test]
    fn suggest_similar_exact_match_excluded() {
        // 完全相同的候选应该被排除（距离 0）
        let candidates = vec!["foo".to_string(), "bar".to_string()];
        let result = suggest_similar("foo", &candidates, 1, 2);
        assert!(result.is_empty(), "完全相同的候选应被排除");
    }

    #[test]
    fn suggest_similar_returns_closest_first() {
        // 目标 "foz"：与 "foo"/"for"/"fox" 距离均为 1（替换 z），
        // 与 "foobar" 距离 4（插入 o,b,a,r 并替换 z）。
        let candidates = vec![
            "foobar".to_string(), // 距离 4 > max_distance=3，被过滤
            "foo".to_string(),    // 距离 1
            "for".to_string(),    // 距离 1
            "fox".to_string(),    // 距离 1
        ];
        let result = suggest_similar("foz", &candidates, 2, 3);
        assert_eq!(result.len(), 2, "应返回 2 个候选（foobar 距离 4 被过滤）");
        // 距离 1 的候选应排在前面；返回的 2 个都应是距离 1 的
        assert!(
            result.iter().all(|c| c == "foo" || c == "for" || c == "fox"),
            "返回的候选应都是距离 1 的: {:?}",
            result
        );
        assert!(
            !result.contains(&"foobar".to_string()),
            "距离 4 的候选超过 max_distance=3 应被过滤"
        );
    }

    #[test]
    fn suggest_similar_filters_by_max_distance() {
        let candidates = vec![
            "x".to_string(),        // 距离 4
            "abc".to_string(),      // 距离 1
            "abcdefgh".to_string(), // 距离 6
        ];
        let result = suggest_similar("abcd", &candidates, 5, 2);
        assert_eq!(result, vec!["abc".to_string()]);
    }

    #[test]
    fn suggest_similar_case_insensitive() {
        // 验证大小写不敏感比较：
        // 目标 "fox" lower 后 = "fox"
        // 候选 "FOX" lower 后 = "fox" → 距离 0，应被排除（视为完全相同）
        // 候选 "For" lower 后 = "for" → 距离 1，应保留
        let candidates = vec!["FOX".to_string(), "For".to_string()];
        let result = suggest_similar("fox", &candidates, 1, 2);
        assert_eq!(result, vec!["For".to_string()]);
    }

    #[test]
    fn suggest_similar_typo_count_conut() {
        // 经典拼写错误："conut" → "count"（字符交换）
        let vars = vec!["count".to_string(), "counter".to_string(), "total".to_string()];
        let result = suggest_similar("conut", &vars, 1, 3);
        assert_eq!(result, vec!["count".to_string()]);
    }

    #[test]
    fn suggest_similar_no_candidates_returns_empty() {
        let candidates: Vec<String> = vec![];
        let result = suggest_similar("foo", &candidates, 1, 2);
        assert!(result.is_empty());
    }

    #[test]
    fn suggest_similar_respects_max_suggestions() {
        let candidates = vec!["fox".to_string(), "for".to_string(), "for_".to_string()];
        // 所有候选距离都 ≤ 2，但只返回 1 个
        let result = suggest_similar("for", &candidates, 1, 2);
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn suggest_similar_unicode_chinese() {
        // 中文标识符（如 nuzo 中的变量名）也能匹配
        let candidates = vec!["计数器".to_string(), "总数".to_string()];
        let result = suggest_similar("计数", &candidates, 1, 2);
        assert_eq!(result, vec!["计数器".to_string()]);
    }

    // ------------------------------------------------------------------
    // default_max_distance
    // ------------------------------------------------------------------

    #[test]
    fn default_max_distance_short_string() {
        assert_eq!(default_max_distance("a"), 1);
        assert_eq!(default_max_distance("ab"), 1);
        assert_eq!(default_max_distance("abc"), 1);
    }

    #[test]
    fn default_max_distance_medium_string() {
        assert_eq!(default_max_distance("abcd"), 1);
        assert_eq!(default_max_distance("abcde"), 1);
        assert_eq!(default_max_distance("abcdef"), 2);
        assert_eq!(default_max_distance("abcdefgh"), 2);
        assert_eq!(default_max_distance("abcdefghi"), 3);
    }

    #[test]
    fn default_max_distance_long_string() {
        assert_eq!(default_max_distance("abcdefghij"), 3);
        assert_eq!(default_max_distance("abcdefghijklmn"), 4);
    }

    #[test]
    fn default_max_distance_empty_string() {
        assert_eq!(default_max_distance(""), 1);
    }

    #[test]
    fn default_max_distance_unicode() {
        // 中文按字符计数
        assert_eq!(default_max_distance("你好"), 1);
        assert_eq!(default_max_distance("你好世界"), 1);
        assert_eq!(default_max_distance("你好世界再见"), 2);
    }
}
