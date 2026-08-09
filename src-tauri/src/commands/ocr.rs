// OCR 识别命令

// OCR识别结果结构
#[derive(Debug, serde::Serialize)]
pub struct OcrWord {
    pub text: String,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

#[derive(Debug, serde::Serialize)]
pub struct OcrLine {
    pub text: String,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub words: Vec<OcrWord>,
    pub word_gaps: Vec<f32>,
}

#[derive(Debug, serde::Serialize)]
pub struct OcrResult {
    pub text: String,
    pub lines: Vec<OcrLine>,
}

// OCR识别图片字节数组
#[tauri::command]
pub async fn recognize_image_ocr(image_data: Vec<u8>) -> Result<OcrResult, String> {
    tokio::task::spawn_blocking(move || {
        use qcocr::recognize_from_bytes;
        
        let result = recognize_from_bytes(&image_data, None)
            .map_err(|e| format!("OCR识别失败: {}", e))?;
        
        convert_ocr_result(result)
    })
    .await
    .map_err(|e| format!("任务执行失败: {}", e))?
}

// OCR识别图片文件
#[tauri::command]
pub async fn recognize_file_ocr(file_path: String, language: Option<String>) -> Result<OcrResult, String> {
    tokio::task::spawn_blocking(move || {
        use qcocr::recognize_from_file;
        
        let lang = language.as_deref();
        let result = recognize_from_file(&file_path, lang)
            .map_err(|e| format!("OCR识别失败: {}", e))?;
        
        convert_ocr_result(result)
    })
    .await
    .map_err(|e| format!("任务执行失败: {}", e))?
}

// 转换OCR结果为返回格式
fn convert_ocr_result(result: qcocr::OcrRecognitionResult) -> Result<OcrResult, String> {
    let lines = result.lines.iter().map(|line| {
        let words = line.words.iter().map(|word| OcrWord {
            text: word.text.clone(),
            x: word.bounds.x,
            y: word.bounds.y,
            width: word.bounds.width,
            height: word.bounds.height,
        }).collect();
        
        let word_gaps = line.compute_word_gaps();
        
        OcrLine {
            text: line.text.clone(),
            x: line.bounds.x,
            y: line.bounds.y,
            width: line.bounds.width,
            height: line.bounds.height,
            words,
            word_gaps,
        }
    }).collect();
    
    Ok(OcrResult {
        text: result.text,
        lines,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(text: &str, x: f32, width: f32) -> qcocr::OcrWord {
        qcocr::OcrWord {
            text: text.to_string(),
            bounds: qcocr::BoundingBox {
                x,
                y: 0.0,
                width,
                height: 10.0,
            },
        }
    }

    fn line(text: &str, words: Vec<qcocr::OcrWord>) -> qcocr::OcrLine {
        qcocr::OcrLine {
            text: text.to_string(),
            bounds: qcocr::BoundingBox {
                x: 0.0,
                y: 0.0,
                width: 100.0,
                height: 10.0,
            },
            words,
        }
    }

    #[test]
    fn convert_ocr_result_maps_lines_words_and_text() {
        let result = qcocr::OcrRecognitionResult {
            lines: vec![line(
                "hello world",
                vec![word("hello", 0.0, 30.0), word("world", 50.0, 40.0)],
            )],
            text: "hello world".to_string(),
            text_angle: None,
        };
        let out = convert_ocr_result(result).unwrap();
        assert_eq!(out.text, "hello world");
        assert_eq!(out.lines.len(), 1);
        let l = &out.lines[0];
        assert_eq!(l.text, "hello world");
        assert_eq!(l.x, 0.0);
        assert_eq!(l.words.len(), 2);
        assert_eq!(l.words[0].text, "hello");
        assert_eq!(l.words[0].x, 0.0);
        assert_eq!(l.words[1].width, 40.0);
        // word_gaps: w2.x - (w1.x + w1.width) = 50 - 30 = 20
        assert_eq!(l.word_gaps, vec![20.0]);
    }

    #[test]
    fn convert_ocr_result_clamps_negative_word_gaps_to_zero() {
        // 重叠单词 → 负间距钳制为 0
        let result = qcocr::OcrRecognitionResult {
            lines: vec![line(
                "overlap",
                vec![word("a", 10.0, 20.0), word("b", 25.0, 10.0), word("c", 40.0, 5.0)],
            )],
            text: "overlap".to_string(),
            text_angle: None,
        };
        let out = convert_ocr_result(result).unwrap();
        // gap1 = 25 - (10+20) = -5 → 0；gap2 = 40 - (25+10) = 5
        assert_eq!(out.lines[0].word_gaps, vec![0.0, 5.0]);
    }

    #[test]
    fn convert_ocr_result_handles_empty_result() {
        let result = qcocr::OcrRecognitionResult {
            lines: vec![],
            text: String::new(),
            text_angle: None,
        };
        let out = convert_ocr_result(result).unwrap();
        assert_eq!(out.text, "");
        assert!(out.lines.is_empty());
    }
}
