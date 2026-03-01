use std::path::Path;
use std::io::Read;

use sha2::{Sha256, Digest};

use crate::error::{WrightError, Result};

pub fn sha256_file(path: &Path) -> Result<String> {
    let mut file = std::fs::File::open(path).map_err(|e| {
        WrightError::IoError(e)
    })?;

    let mut hasher = Sha256::new();
    let mut buffer = [0; 8192];
    loop {
        let count = file.read(&mut buffer).map_err(|e| {
            WrightError::IoError(e)
        })?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }

    Ok(format!("{:x}", hasher.finalize()))
}
