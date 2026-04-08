use std::io::Read;
use std::path::Path;

use sha2::{Digest, Sha256};

use crate::error::{Result, WrightError};

pub fn sha256_file(path: &Path) -> Result<String> {
    let mut file = std::fs::File::open(path).map_err(WrightError::IoError)?;

    let mut hasher = Sha256::new();
    let mut buffer = [0; 8192];
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(WrightError::IoError)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }

    Ok(format!("{:x}", hasher.finalize()))
}
