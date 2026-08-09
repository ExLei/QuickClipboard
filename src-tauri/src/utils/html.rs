// HTML 处理工具函数

pub fn truncate_html(html: String, max_visible_len: usize) -> String {
    if html.is_empty() {
        return html;
    }
    
    if max_visible_len == 0 {
        return "...(内容过长已截断)".to_string();
    }
    
    let mut visible_count: usize = 0;
    let mut in_tag = false;
    
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => {
                visible_count = visible_count.saturating_add(1);
                if visible_count > max_visible_len {
                    break;
                }
            }
            _ => {}
        }
    }
    
    if visible_count <= max_visible_len {
        return html;
    }
    
    let mut result = String::with_capacity(html.len().min(max_visible_len * 10));
    visible_count = 0;
    in_tag = false;
    let mut open_tags: Vec<String> = Vec::with_capacity(16);
    let mut current_tag = String::with_capacity(32);
    let mut is_closing_tag = false;
    let mut tag_started = false;
    
    for c in html.chars() {
        if c == '<' {
            in_tag = true;
            tag_started = false;
            is_closing_tag = false;
            current_tag.clear();
            result.push(c);
        } else if c == '>' {
            in_tag = false;
            result.push(c);
            
            if !current_tag.is_empty() {
                let tag_name = current_tag.to_lowercase();
                let is_self_closing = matches!(tag_name.as_str(), 
                    "br" | "hr" | "img" | "input" | "meta" | "link" | "area" | "base" | "col" | "embed" | "source" | "track" | "wbr");
                
                if !is_self_closing {
                    if is_closing_tag {
                        if let Some(pos) = open_tags.iter().rposition(|t| t == &tag_name) {
                            open_tags.remove(pos);
                        }
                    } else {
                        if open_tags.len() < 100 {
                            open_tags.push(tag_name);
                        }
                    }
                }
            }
        } else if in_tag {
            result.push(c);
            
            if c == '/' && !tag_started {
                is_closing_tag = true;
            } else if c.is_alphanumeric() && !tag_started {
                tag_started = true;
                if current_tag.len() < 50 {
                    current_tag.push(c);
                }
            } else if tag_started && (c.is_alphanumeric() || c == '-') {
                if current_tag.len() < 50 {
                    current_tag.push(c);
                }
            } else if tag_started {
                tag_started = false;
            }
        } else {
            visible_count = visible_count.saturating_add(1);
            if visible_count > max_visible_len {
                break;
            }
            result.push(c);
        }
    }
    
    for tag in open_tags.iter().rev().take(50) {
        result.push_str("</");
        result.push_str(tag);
        result.push('>');
    }
    result.push_str("...(内容过长已截断)");
    
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    const SUFFIX: &str = "...(内容过长已截断)";

    #[test]
    fn empty_html_returns_unchanged() {
        assert_eq!(truncate_html(String::new(), 100), "");
    }

    #[test]
    fn zero_max_visible_len_returns_truncation_message() {
        assert_eq!(truncate_html("<b>abc</b>".to_string(), 0), SUFFIX);
    }

    #[test]
    fn html_within_limit_passes_through_unchanged() {
        let html = "<b>abc</b>".to_string();
        // 边界：可见字符数 == max → 原样返回
        assert_eq!(truncate_html(html.clone(), 3), html);
        assert_eq!(truncate_html(html.clone(), 10), html);
    }

    #[test]
    fn truncates_visible_text_and_closes_open_tags() {
        // 超限的 'c' 不会进入结果（break 发生在 push 之前）
        assert_eq!(
            truncate_html("<b>abc</b>".to_string(), 2),
            format!("<b>ab</b>{}", SUFFIX)
        );
    }

    #[test]
    fn self_closing_tags_are_not_balanced() {
        assert_eq!(
            truncate_html("<p>a<br>bc</p>".to_string(), 1),
            format!("<p>a<br></p>{}", SUFFIX)
        );
    }

    #[test]
    fn img_and_wbr_are_also_treated_as_self_closing() {
        // 裸 <img> 与 <wbr> 不进入 open_tags 栈 → 不会为它们补闭合标签
        assert_eq!(
            truncate_html("<p>x<img>yz</p>".to_string(), 1),
            format!("<p>x<img></p>{}", SUFFIX)
        );
        // 标签原文保留，只是不为其补闭合标签
        assert_eq!(
            truncate_html("<div>a<wbr>b</div>".to_string(), 1),
            format!("<div>a<wbr></div>{}", SUFFIX)
        );
    }

    #[test]
    fn tag_name_accumulation_restarts_after_space_and_absorbs_attribute_words() {
        // 空格后 tag_started 复位，属性中的字母重新进入 current_tag → 标签名被属性污染
        assert_eq!(
            truncate_html("<div class=\"x\">ab</div>".to_string(), 1),
            format!("<div class=\"x\">a</divclassx>{}", SUFFIX)
        );
    }

    #[test]
    fn nested_open_tags_close_in_reverse_order() {
        // 超限的 'c' 不会进入结果；已闭合的 span 从栈中移除
        assert_eq!(
            truncate_html("<div><span>ab</span><b>c</b></div>".to_string(), 2),
            format!("<div><span>ab</span><b></b></div>{}", SUFFIX)
        );
    }

    #[test]
    fn tag_names_are_lowercased_when_closed() {
        assert_eq!(
            truncate_html("<DIV>ab</DIV>".to_string(), 1),
            format!("<DIV>a</div>{}", SUFFIX)
        );
    }

    #[test]
    fn hyphenated_tag_names_are_tracked() {
        assert_eq!(
            truncate_html("<my-tag>ab</my-tag>".to_string(), 1),
            format!("<my-tag>a</my-tag>{}", SUFFIX)
        );
    }

    #[test]
    fn open_tag_stack_closes_at_most_50_of_60_nested_tags() {
        let html = format!("{}abc", "<div>".repeat(60));
        let result = truncate_html(html, 1);
        assert_eq!(result.matches("<div>").count(), 60);
        assert_eq!(result.matches("</div>").count(), 50);
        assert!(result.ends_with(SUFFIX));
    }
}
