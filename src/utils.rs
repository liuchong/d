use mime_guess::get_mime_type_str as guess_mime_type_str;
use regex::Regex;
use std::path::Path;

#[derive(Default)]
pub struct Range(pub String, pub u64, pub Option<u64>);

pub fn parse_range(range: &str) -> Option<Range> {
    // Range Syntax, 3, 4 not supported for now
    // 1. Range: <unit>=<range-start>-
    // 2. Range: <unit>=<range-start>-<range-end>
    // 3. Range: <unit>=<range-start>-<range-end>, <range-start>-<range-end>
    // 4. Range: <unit>=<range-start>-<range-end>, <range-start>-<range-end>, <range-start>-<range-end>

    let re = Regex::new(r"(.*)=(\d+)-(\d+)?").unwrap(); // will success
    match re.captures(range) {
        Some(cap) => {
            let unit = cap.get(1).unwrap().as_str().to_string();
            let start = cap.get(2).unwrap().as_str().parse::<u64>().unwrap();
            let end = match cap.get(3) {
                Some(m) => Some(m.as_str().parse::<u64>().unwrap()),
                _ => None,
            };
            Some(Range(unit, start, end))
        }
        _ => None,
    }
}

pub fn get_mime_type_str(filename: &str) -> &str {
    let ext = Path::new(&filename)
        .extension()
        .unwrap_or_default()
        .to_str()
        .unwrap_or_default();
    guess_mime_type_str(ext).unwrap_or("application/octet-stream")
}
