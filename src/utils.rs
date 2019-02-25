use mime_guess::get_mime_type_str as guess_mime_type_str;
use std::path::Path;

pub fn get_mime_type_str(filename: &str) -> &str {
    let ext = Path::new(&filename)
        .extension()
        .unwrap_or_default()
        .to_str()
        .unwrap_or_default();

    guess_mime_type_str(ext).unwrap_or("application/octet-stream")
}
