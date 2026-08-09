pub fn generate_cf_html(html: &str) -> String {
    let html_content = if !html.contains("<html") {
        format!(
            "<!DOCTYPE html>\n<html>\n<head>\n<meta charset=\"utf-8\">\n</head>\n<body>\n<!--StartFragment-->{}\n<!--EndFragment-->\n</body>\n</html>",
            html
        )
    } else if !html.contains("<!--StartFragment-->") {
        html.replace("<body>", "<body>\n<!--StartFragment-->")
            .replace("</body>", "<!--EndFragment-->\n</body>")
    } else {
        html.to_string()
    };

    let header = "Version:0.9\r\nStartHTML:0000000000\r\nEndHTML:0000000000\r\nStartFragment:0000000000\r\nEndFragment:0000000000\r\n";
    let start_html = header.len();
    let end_html = start_html + html_content.len();

    let start_fragment = start_html + html_content.find("<!--StartFragment-->").unwrap_or(0);
    let end_fragment = start_html + html_content.find("<!--EndFragment-->").unwrap_or(html_content.len());

    format!(
        "Version:0.9\r\nStartHTML:{:010}\r\nEndHTML:{:010}\r\nStartFragment:{:010}\r\nEndFragment:{:010}\r\n{}",
        start_html, end_html, start_fragment, end_fragment, html_content
    )
}

pub fn normalize_clipboard_html(input: &str) -> String {
    let s = input;

    if s.contains("StartFragment") || s.contains("StartHTML") {
        if let Some(fragment) = extract_cf_html_by_markers(s) {
            return fragment;
        }
        if let Some(fragment) = extract_cf_html_by_offsets(s) {
            return fragment;
        }
    }

    s.to_string()
}

fn extract_cf_html_by_markers(s: &str) -> Option<String> {
    let start_marker = "<!--StartFragment-->";
    let end_marker = "<!--EndFragment-->";

    let start = s.find(start_marker)? + start_marker.len();
    let end = s.find(end_marker)?;
    if end <= start {
        return None;
    }

    Some(s[start..end].to_string())
}

fn extract_cf_html_by_offsets(s: &str) -> Option<String> {
    fn parse_offset(s: &str, key: &str) -> Option<usize> {
        let idx = s.find(key)?;
        let rest = &s[idx + key.len()..];
        let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        if digits.is_empty() {
            return None;
        }
        digits.parse::<usize>().ok()
    }

    let bytes = s.as_bytes();
    let len = bytes.len();

    let start_fragment = parse_offset(s, "StartFragment:").or_else(|| parse_offset(s, "StartHTML:"));
    let end_fragment = parse_offset(s, "EndFragment:").or_else(|| parse_offset(s, "EndHTML:"));

    let (start, end) = match (start_fragment, end_fragment) {
        (Some(a), Some(b)) if a < b && b <= len => (a, b),
        _ => return None,
    };

    std::str::from_utf8(&bytes[start..end]).ok().map(|t| t.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    // header 固定长度："Version:0.9\r\n"(13) + "StartHTML:...\r\n"(22) +
    // "EndHTML:...\r\n"(20) + "StartFragment:...\r\n"(26) + "EndFragment:...\r\n"(24) = 105
    const START_HTML: usize = 105;

    #[test]
    fn generate_cf_html_wraps_plain_fragment_in_full_document() {
        let result = generate_cf_html("x");
        // 前缀 68 字节；StartFragment=105+68=173；fragment 到 105+90=195；content=124 → EndHTML=229
        let expected = concat!(
            "Version:0.9\r\n",
            "StartHTML:0000000105\r\n",
            "EndHTML:0000000229\r\n",
            "StartFragment:0000000173\r\n",
            "EndFragment:0000000195\r\n",
            "<!DOCTYPE html>\n<html>\n<head>\n<meta charset=\"utf-8\">\n</head>\n<body>\n",
            "<!--StartFragment-->x\n<!--EndFragment-->\n</body>\n</html>",
        );
        assert_eq!(result, expected);
    }

    #[test]
    fn generate_cf_html_wraps_existing_html_body_in_fragment_markers() {
        let result = generate_cf_html("<html><body><p>hi</p></body></html>");
        let expected = concat!(
            "Version:0.9\r\n",
            "StartHTML:0000000105\r\n",
            "EndHTML:0000000180\r\n",
            "StartFragment:0000000118\r\n",
            "EndFragment:0000000147\r\n",
            "<html><body>\n<!--StartFragment--><p>hi</p><!--EndFragment-->\n</body></html>",
        );
        assert_eq!(result, expected);
    }

    #[test]
    fn generate_cf_html_preserves_existing_markers() {
        // 已含标记的输入原样保留；偏移：input 72 字节 → StartFragment=105+12=117，
        // EndFragment=105+40=145，EndHTML=105+72=177
        let input = "<html><body><!--StartFragment--><p>x</p><!--EndFragment--></body></html>";
        let result = generate_cf_html(input);
        let expected = concat!(
            "Version:0.9\r\n",
            "StartHTML:0000000105\r\n",
            "EndHTML:0000000177\r\n",
            "StartFragment:0000000117\r\n",
            "EndFragment:0000000145\r\n",
            "<html><body><!--StartFragment--><p>x</p><!--EndFragment--></body></html>",
        );
        assert_eq!(result, expected);
    }

    #[test]
    fn normalize_extracts_fragment_by_markers() {
        let input = concat!(
            "Version:0.9\r\nStartHTML:0000000105\r\nEndHTML:0000000233\r\n",
            "StartFragment:0000000176\r\nEndFragment:0000000199\r\n",
            "<html><body><!--StartFragment-->fragment<!--EndFragment--></body></html>",
        );
        assert_eq!(normalize_clipboard_html(input), "fragment");
    }

    #[test]
    fn normalize_extracts_fragment_by_offsets_when_markers_absent() {
        // body 从 105 开始；<html><body> = 12 字节 → "<p>" 在 117；fragment = 117..129 → "<p>hello</p>"
        let input = concat!(
            "Version:0.9\r\nStartHTML:0000000105\r\nEndHTML:0000000143\r\n",
            "StartFragment:0000000117\r\nEndFragment:0000000129\r\n",
            "<html><body><p>hello</p></body></html>",
        );
        assert_eq!(normalize_clipboard_html(input), "<p>hello</p>");
    }

    #[test]
    fn normalize_falls_back_to_starthtml_endhtml_offsets() {
        // 该输入只有 StartHTML/EndHTML 两行 → header = 55 字节；body = 55..93
        let input = concat!(
            "Version:0.9\r\nStartHTML:0000000055\r\nEndHTML:0000000093\r\n",
            "<html><body><p>hello</p></body></html>",
        );
        assert_eq!(
            normalize_clipboard_html(input),
            "<html><body><p>hello</p></body></html>"
        );
    }

    #[test]
    fn normalize_extracts_utf8_fragment_by_offsets() {
        // "<p>中文内容</p>" = 3 + 12 + 4 = 19 字节；"<p>" 在 117；body = 12+19+14 = 45 → EndHTML = 150
        let input = concat!(
            "Version:0.9\r\nStartHTML:0000000105\r\nEndHTML:0000000150\r\n",
            "StartFragment:0000000117\r\nEndFragment:0000000136\r\n",
            "<html><body><p>中文内容</p></body></html>",
        );
        assert_eq!(normalize_clipboard_html(input), "<p>中文内容</p>");
    }

    #[test]
    fn normalize_returns_input_unchanged_without_cf_markers() {
        let plain = "<html><body>hello</body></html>";
        assert_eq!(normalize_clipboard_html(plain), plain);
        // 含 "StartFragment" 字样但无任何可解析数据 → 原样返回
        let junk = "no StartFragment: here, StartHTML is a lie";
        assert_eq!(normalize_clipboard_html(junk), junk);
    }

    #[test]
    fn normalize_rejects_corrupted_or_non_utf8_offsets() {
        // EndFragment < StartFragment → 两种提取都失败 → 原样返回
        let bad_order = concat!(
            "StartHTML:0000000105\r\nEndHTML:0000000144\r\n",
            "StartFragment:0000000140\r\nEndFragment:0000000120\r\n",
            "<html><body>corrupted</body></html>",
        );
        assert_eq!(normalize_clipboard_html(bad_order), bad_order);
        // 偏移超出字符串长度 → 提取失败 → 原样返回
        let out_of_range = concat!(
            "StartHTML:0000000105\r\nEndHTML:0000000144\r\n",
            "StartFragment:0000000105\r\nEndFragment:0000000999\r\n",
            "<html><body>corrupted</body></html>",
        );
        assert_eq!(normalize_clipboard_html(out_of_range), out_of_range);
        // 只有 StartFragment 没有 EndFragment 对应项 → 原样返回
        let missing_end = concat!(
            "StartHTML:0000000105\r\n",
            "StartFragment:0000000118\r\n",
            "<html><body>corrupted</body></html>",
        );
        assert_eq!(normalize_clipboard_html(missing_end), missing_end);
    }
}
