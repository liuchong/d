use crate::error::{Error, Result};
use std::fs;
use std::fs::Metadata;

pub struct FileInfo(pub String, pub Metadata);

pub enum LsRes {
    Dir(Vec<FileInfo>),
    File(FileInfo),
}

pub fn ls(path: &str) -> Result<LsRes> {
    let metadata = fs::metadata(path)?;
    if metadata.is_dir() {
        let mut files: Vec<FileInfo> = Vec::new();
        for entry in fs::read_dir(path)? {
            let p = match entry?.path().to_str() {
                Some(str) => str.to_string(),
                None => {
                    return Err(Error::Io(
                        "Cannot convert path to str".to_string(),
                    ));
                }
            };

            files.push(FileInfo(p.clone(), fs::metadata(p)?));
        }
        return Ok(LsRes::Dir(files));
    }

    Ok(LsRes::File(FileInfo(path.to_string(), metadata)))
}
