// 判断是否是图片文件
pub fn is_image_file(path: &str) -> bool {
    let path_lower = path.to_lowercase();
    path_lower.ends_with(".jpg") || 
    path_lower.ends_with(".jpeg") || 
    path_lower.ends_with(".png") || 
    path_lower.ends_with(".gif") || 
    path_lower.ends_with(".bmp") || 
    path_lower.ends_with(".webp")
}

// 读取图片尺寸
pub fn get_image_dimensions(path: &str) -> Option<(u32, u32)> {
    use std::fs::File;
    use std::io::BufReader;
    use image::ImageReader;
    
    let file = File::open(path).ok()?;
    let reader = BufReader::new(file);
    let img_reader = ImageReader::new(reader).with_guessed_format().ok()?;
    img_reader.into_dimensions().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_extension_detection_is_case_insensitive() {
        for ext in ["jpg", "jpeg", "png", "gif", "bmp", "webp"] {
            assert!(is_image_file(&format!("photo.{}", ext)), "{}", ext);
            assert!(
                is_image_file(&format!("photo.{}", ext.to_uppercase())),
                "{}",
                ext
            );
        }
        assert!(is_image_file("dir/photo.PnG"));
        assert!(!is_image_file("doc.txt"));
        assert!(!is_image_file("archive.tar"));
        assert!(!is_image_file("photo"));
        assert!(!is_image_file("a.png.txt")); // 后缀必须是结尾
        assert!(!is_image_file(""));
    }

    #[test]
    fn image_dimensions_read_real_png_file() {
        let path = std::env::temp_dir().join(format!("qc_utils_dim_test_{}.png", std::process::id()));
        let img = image::RgbaImage::from_pixel(3, 2, image::Rgba([255u8, 0, 0, 255]));
        img.save(&path).expect("write test png");
        assert_eq!(get_image_dimensions(&path.to_string_lossy()), Some((3, 2)));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn image_dimensions_none_for_missing_or_invalid_files() {
        assert_eq!(get_image_dimensions("/nonexistent/qc_missing.png"), None);
        let path = std::env::temp_dir().join(format!("qc_utils_garbage_{}.png", std::process::id()));
        std::fs::write(&path, b"this is not an image").expect("write garbage file");
        assert_eq!(get_image_dimensions(&path.to_string_lossy()), None);
        let _ = std::fs::remove_file(&path);
    }
}
