// 文本截断工具函数

const SEARCH_CONTEXT_BEFORE_CHARS: usize = 16;
const ELLIPSIS: &str = "...";

pub fn is_textual_content_type(content_type: &str) -> bool {
    content_type
        .split(',')
        .map(str::trim)
        .any(|kind| matches!(kind, "text" | "rich_text" | "link"))
}

pub fn truncate_string(s: String, max_len: usize) -> String {
    if s.is_empty() || s.len() <= max_len {
        return s;
    }
    
    let mut truncate_point = max_len.saturating_sub(50);
    while truncate_point > 0 && !s.is_char_boundary(truncate_point) {
        truncate_point -= 1;
    }
    
    if truncate_point == 0 {
        return "...(内容过长已截断)".to_string();
    }
    
    match s.get(..truncate_point) {
        Some(slice) => format!("{}...(内容过长已截断)", slice),
        None => "...(内容过长已截断)".to_string(),
    }
}

fn original_byte_index_at_lowercase_offset(text: &str, target_offset: usize) -> Option<usize> {
    if target_offset == 0 {
        return Some(0);
    }

    let mut lowercase_offset = 0;
    for (byte_index, ch) in text.char_indices() {
        if lowercase_offset == target_offset {
            return Some(byte_index);
        }

        lowercase_offset += ch
            .to_lowercase()
            .map(char::len_utf8)
            .sum::<usize>();
        if lowercase_offset > target_offset {
            return None;
        }
    }

    (lowercase_offset == target_offset).then_some(text.len())
}

fn find_keyword_range(text: &str, keyword: &str) -> Option<(usize, usize)> {
    if let Some(start) = text.find(keyword) {
        return Some((start, start + keyword.len()));
    }

    let lowercase_text = text.to_lowercase();
    let lowercase_keyword = keyword.to_lowercase();
    let lowercase_start = lowercase_text.find(&lowercase_keyword)?;
    let lowercase_end = lowercase_start + lowercase_keyword.len();
    let start = original_byte_index_at_lowercase_offset(text, lowercase_start)?;
    let end = original_byte_index_at_lowercase_offset(text, lowercase_end)?;
    Some((start, end))
}

fn byte_index_before_chars(text: &str, end: usize, count: usize) -> usize {
    if count == 0 {
        return end;
    }

    text[..end]
        .char_indices()
        .rev()
        .nth(count - 1)
        .map(|(index, _)| index)
        .unwrap_or(0)
}

// 截取关键词附近的搜索摘要，并让关键词靠近摘要开头。
pub fn truncate_around_keyword(s: String, keyword: &str, max_len: usize) -> String {
    if s.is_empty() || keyword.is_empty() || s.len() <= max_len {
        return if s.len() <= max_len { s } else { truncate_string(s, max_len) };
    }

    let (keyword_start, keyword_end) = match find_keyword_range(&s, keyword) {
        Some(range) => range,
        None => return truncate_string(s, max_len),
    };

    let mut start = byte_index_before_chars(&s, keyword_start, SEARCH_CONTEXT_BEFORE_CHARS);
    let prefix_len = if start > 0 { ELLIPSIS.len() } else { 0 };
    let mut slice_len = max_len.saturating_sub(prefix_len + ELLIPSIS.len());

    if keyword_end.saturating_sub(start) > slice_len {
        start = keyword_start;
        slice_len = max_len.saturating_sub(ELLIPSIS.len() * 2);
    }

    let mut end = start.saturating_add(slice_len).min(s.len());
    while end > start && !s.is_char_boundary(end) {
        end -= 1;
    }

    if end <= start || end < keyword_end {
        return truncate_string(s, max_len);
    }

    let slice = match s.get(start..end) {
        Some(slice) => slice,
        None => return truncate_string(s, max_len),
    };
    
    let mut result = String::with_capacity(max_len);

    if start > 0 {
        result.push_str(ELLIPSIS);
    }

    result.push_str(slice);

    if end < s.len() {
        result.push_str(ELLIPSIS);
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_compound_text_content_types() {
        assert!(is_textual_content_type("text,link"));
        assert!(is_textual_content_type("rich_text,link,image"));
        assert!(is_textual_content_type(" link , image "));
        assert!(!is_textual_content_type("image,file"));
    }

    #[test]
    fn search_excerpt_keeps_keyword_near_the_start() {
        let content = format!("{}目标关键词{}", "前文".repeat(1000), "后文".repeat(1000));
        let excerpt = truncate_around_keyword(content, "目标关键词", 1600);
        let keyword_index = excerpt.find("目标关键词").expect("摘要应包含关键词");

        assert!(excerpt.len() <= 1600);
        assert!(excerpt.starts_with(ELLIPSIS));
        assert!(excerpt[..keyword_index].chars().count() <= SEARCH_CONTEXT_BEFORE_CHARS + 3);
    }

    #[test]
    fn search_excerpt_matches_ascii_case_insensitively() {
        let content = format!("{}Needle{}", "before ".repeat(500), " after".repeat(500));
        let excerpt = truncate_around_keyword(content, "needle", 800);

        assert!(excerpt.len() <= 800);
        assert!(excerpt.contains("Needle"));
    }

    #[test]
    fn search_excerpt_preserves_utf8_boundaries() {
        let content = format!("{}关键字{}", "甲".repeat(1000), "乙".repeat(1000));
        let excerpt = truncate_around_keyword(content, "关键字", 257);

        assert!(excerpt.len() <= 257);
        assert!(excerpt.contains("关键字"));
    }

    #[test]
    fn content_type_detection_is_exact_and_case_sensitive() {
        assert!(is_textual_content_type("text"));
        assert!(is_textual_content_type("rich_text"));
        assert!(is_textual_content_type("link"));
        assert!(is_textual_content_type("link, rich_text"));
        assert!(is_textual_content_type(" text , image "));
        assert!(!is_textual_content_type("image"));
        assert!(!is_textual_content_type("image,file"));
        assert!(!is_textual_content_type("Text")); // 大小写敏感
        assert!(!is_textual_content_type(""));
        assert!(!is_textual_content_type("text;")); // 必须精确匹配 kind
        assert!(!is_textual_content_type("texts"));
    }

    #[test]
    fn truncate_string_passes_through_short_and_empty_inputs() {
        assert_eq!(truncate_string(String::new(), 100), "");
        assert_eq!(truncate_string("abc".to_string(), 3), "abc");
        assert_eq!(truncate_string("abc".to_string(), 5), "abc");
        assert_eq!(truncate_string("A".repeat(100), 100), "A".repeat(100));
    }

    #[test]
    fn truncate_string_appends_ellipsis_suffix_when_over_max_len() {
        // max_len=100 → 截断点 = 100 - 50 = 50
        assert_eq!(
            truncate_string("A".repeat(101), 100),
            format!("{}...(内容过长已截断)", "A".repeat(50))
        );
        // max_len=51 → 截断点 = 1
        assert_eq!(truncate_string("A".repeat(60), 51), "A...(内容过长已截断)");
    }

    #[test]
    fn truncate_string_returns_message_only_when_max_len_is_50_or_below() {
        assert_eq!(truncate_string("A".repeat(60), 50), "...(内容过长已截断)");
        assert_eq!(truncate_string("A".repeat(60), 49), "...(内容过长已截断)");
        assert_eq!(truncate_string("A".repeat(60), 0), "...(内容过长已截断)");
    }

    #[test]
    fn truncate_string_never_splits_multibyte_chars() {
        // "甲" 占 3 字节；截断点 50 → 回退到 48（16 个字符）
        assert_eq!(
            truncate_string("甲".repeat(40), 100),
            format!("{}...(内容过长已截断)", "甲".repeat(16))
        );
        // max_len=51 → 截断点 1 不是字符边界 → 仅消息
        assert_eq!(truncate_string("甲".repeat(20), 51), "...(内容过长已截断)");
    }

    #[test]
    fn keyword_excerpt_exact_case_and_format() {
        // s = A*10 + KEY + B*30 (43 字节), max_len=25
        let s = format!("{}KEY{}", "A".repeat(10), "B".repeat(30));
        assert_eq!(
            truncate_around_keyword(s, "KEY", 25),
            format!("{}KEY{}...", "A".repeat(10), "B".repeat(9))
        );
    }

    #[test]
    fn keyword_excerpt_matches_case_insensitively_with_context() {
        // s = A*20 + needle + B*30 (56 字节), max_len=40 → 前文保留 16 字符
        let s = format!("{}needle{}", "A".repeat(20), "B".repeat(30));
        assert_eq!(
            truncate_around_keyword(s, "NEEDLE", 40),
            format!("...{}needle{}...", "A".repeat(16), "B".repeat(12))
        );
    }

    #[test]
    fn keyword_excerpt_preserves_utf8_and_keeps_keyword_in_slice() {
        // 甲*30 + 关键词 + 乙*30 (189 字节), max_len=100
        let s = format!("{}关键词{}", "甲".repeat(30), "乙".repeat(30));
        assert_eq!(
            truncate_around_keyword(s, "关键词", 100),
            format!("...{}关键词{}...", "甲".repeat(16), "乙".repeat(12))
        );
    }

    #[test]
    fn keyword_excerpt_falls_back_to_truncate_string_when_keyword_missing() {
        assert_eq!(
            truncate_around_keyword("A".repeat(100), "xyz", 40),
            "...(内容过长已截断)"
        );
    }

    #[test]
    fn keyword_excerpt_edge_inputs() {
        assert_eq!(truncate_around_keyword(String::new(), "k", 10), "");
        // 空 keyword → truncate_string 兜底
        assert_eq!(
            truncate_around_keyword("A".repeat(120), "", 100),
            format!("{}...(内容过长已截断)", "A".repeat(50))
        );
        // 短文本原样返回（即使没有 keyword）
        assert_eq!(
            truncate_around_keyword("abcdef".to_string(), "zzz", 100),
            "abcdef"
        );
    }

    #[test]
    fn keyword_excerpt_boundary_when_keyword_barely_fits() {
        // "KEYWORD" = 7 字节；slice_len = max_len - 6
        let s = format!("{}KEYWORD{}", "A".repeat(40), "B".repeat(40));
        // max_len=14：end=48 > keyword_end=47 → 吸收 1 个后续字符（结果 15 字节 > max_len，如实记录）
        assert_eq!(truncate_around_keyword(s.clone(), "KEYWORD", 14), "...KEYWORDB...");
        // max_len=13：end 恰好 == keyword_end → 恰好放下
        assert_eq!(truncate_around_keyword(s.clone(), "KEYWORD", 13), "...KEYWORD...");
        // max_len=12：end=46 < keyword_end=47 → truncate_string 兜底
        assert_eq!(
            truncate_around_keyword(s, "KEYWORD", 12),
            "...(内容过长已截断)"
        );
    }
}
