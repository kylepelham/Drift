//! Git blob objects, hashed and stored in-process: launching git costs tens of milliseconds on Windows,
//! and every write and edit records its file before and after.

use std::io::{Read, Write};
use std::path::Path;

use sha1::{Digest, Sha1};

use super::Error;

/// `path`'s blob id, and with `objects` the blob stored there too.
pub(super) fn of_file(path: &Path, objects: Option<&Path>) -> Result<String, Error> {
    match objects {
        Some(objects) => store(objects, &std::fs::read(path).map_err(failed)?),
        None => id_of_file(path),
    }
}

/// Streamed, so a large file is never held whole.
fn id_of_file(path: &Path) -> Result<String, Error> {
    let mut file = std::fs::File::open(path).map_err(failed)?;
    let len = file.metadata().map_err(failed)?.len();
    let mut hasher = hasher(len);
    std::io::copy(&mut (&mut file).take(len), &mut hasher).map_err(failed)?;

    Ok(hex(hasher))
}

/// Stores `bytes` as a loose object under `objects`, as `git hash-object -w --no-filters` does.
pub(super) fn store(objects: &Path, bytes: &[u8]) -> Result<String, Error> {
    let mut hasher = hasher(bytes.len() as u64);
    hasher.update(bytes);
    let id = hex(hasher);
    let dir = objects.join(&id[..2]);
    let target = dir.join(&id[2..]);
    if target.exists() {
        return Ok(id);
    }

    std::fs::create_dir_all(&dir).map_err(failed)?;
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    encoder
        .write_all(format!("blob {}\0", bytes.len()).as_bytes())
        .and_then(|()| encoder.write_all(bytes))
        .map_err(failed)?;
    let compressed = encoder.finish().map_err(failed)?;
    // Written aside and renamed in, so a reader never sees half an object.
    let temp = dir.join(format!("tmp_obj_{}", crate::random_hex(8)));
    std::fs::write(&temp, compressed).map_err(failed)?;
    match std::fs::rename(&temp, &target) {
        Ok(()) => Ok(id),
        Err(error) => {
            let _ = std::fs::remove_file(&temp);
            // Another writer stored the same object first.
            if target.exists() { Ok(id) } else { Err(failed(error)) }
        }
    }
}

fn hasher(len: u64) -> Sha1 {
    let mut hasher = Sha1::new();
    hasher.update(format!("blob {len}\0").as_bytes());
    hasher
}

fn hex(hasher: Sha1) -> String {
    crate::hex_bytes(&hasher.finalize())
}

fn failed(error: std::io::Error) -> Error {
    Error::Failed(error.to_string())
}
