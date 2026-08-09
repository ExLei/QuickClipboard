use file_icon_provider::get_file_icon;
use image::{RgbaImage, ImageFormat};
use std::io::Cursor;
use sha2::{Sha256, Digest};

// 获取文件图标并转换为 Base64 Data URL
pub fn get_file_icon_base64(path: &str) -> Option<String> {
    match get_file_icon(path, 32) {
        Ok(icon) => {
            if let Ok(png_data) = icon_to_png(&icon) {
                use base64::{Engine as _, engine::general_purpose};
                let base64_str = general_purpose::STANDARD.encode(&png_data);
                return Some(format!("data:image/png;base64,{}", base64_str));
            }
            None
        }
        Err(_) => None,
    }
}

// 将 Icon 转换为 PNG 格式
pub fn icon_to_png(icon: &file_icon_provider::Icon) -> Result<Vec<u8>, String> {
    let img = RgbaImage::from_raw(icon.width, icon.height, icon.pixels.clone())
        .ok_or("创建图像失败")?;
    
    let mut png_data = Vec::new();
    let mut cursor = Cursor::new(&mut png_data);
    img.write_to(&mut cursor, ImageFormat::Png)
        .map_err(|e| format!("PNG编码失败: {}", e))?;
    
    Ok(png_data)
}

// 计算图标哈希
fn calculate_icon_hash(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    let hash = format!("{:x}", hasher.finalize());
    hash[..16].to_string()
}

// 保存应用图标到 app_icons 目录
pub fn save_app_icon(exe_path: &str) -> Option<String> {
    let icon = match get_file_icon(exe_path, 32) {
        Ok(icon) => icon,
        Err(_) => return None,
    };
    
    let png_data = match icon_to_png(&icon) {
        Ok(data) => data,
        Err(_) => return None,
    };

    let hash = calculate_icon_hash(&png_data);

    let data_dir = match crate::services::get_data_directory() {
        Ok(dir) => dir,
        Err(_) => return None,
    };

    let icons_dir = data_dir.join("app_icons");
    if !icons_dir.exists() {
        if std::fs::create_dir_all(&icons_dir).is_err() {
            return None;
        }
    }

    let icon_path = icons_dir.join(format!("{}.png", hash));
    if !icon_path.exists() {
        if std::fs::write(&icon_path, &png_data).is_err() {
            return None;
        }
    }
    
    Some(hash)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_icon() -> file_icon_provider::Icon {
        file_icon_provider::Icon {
            width: 2,
            height: 2,
            pixels: vec![
                255, 0, 0, 255, // red
                0, 255, 0, 255, // green
                0, 0, 255, 255, // blue
                255, 255, 255, 255, // white
            ],
        }
    }

    #[test]
    fn icon_to_png_encodes_rgba_pixels_as_png() {
        let png = icon_to_png(&test_icon()).expect("png encoding should succeed");
        // PNG 魔数
        assert!(png.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]));
        // IHDR: 8 字节签名 + 4 字节长度 + 4 字节 "IHDR" → 宽高在字节 16..24（大端）
        assert_eq!(&png[16..20], &[0, 0, 0, 2]);
        assert_eq!(&png[20..24], &[0, 0, 0, 2]);
    }

    #[test]
    fn icon_to_png_rejects_pixel_buffer_size_mismatch() {
        let bad = file_icon_provider::Icon {
            width: 2,
            height: 2,
            pixels: vec![0, 0, 0, 0],
        };
        assert_eq!(icon_to_png(&bad), Err("创建图像失败".to_string()));
    }

    #[test]
    fn icon_hash_is_first_16_hex_chars_of_sha256() {
        // 标准 SHA-256 测试向量：sha256("")、sha256("a")、sha256("data") 的前 16 个十六进制字符
        assert_eq!(calculate_icon_hash(b""), "e3b0c44298fc1c14");
        assert_eq!(calculate_icon_hash(b"a"), "ca978112ca1bbdca");
        assert_eq!(calculate_icon_hash(b"data"), "3a6eb0790f39ac87");
        // 区分不同输入
        assert_ne!(calculate_icon_hash(b"data"), calculate_icon_hash(b"datb"));
    }
}
