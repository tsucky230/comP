//! ファイルのハッシュは index.db に保存され、次回の起動時に照合される。表記（小文字16進64桁）が
//! 変わると、更新後に全ファイルを変更ありと判定して索引し直すことになるので、既知の値で固定する。
//! sha2 0.11 で `format!("{:x}", digest)` が使えなくなったときに足した。
//! Owned by the designer role; implementers must not edit.
use super::*;
use std::io::Write;

const SHA256_OF_ABC: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

#[test]
fn sha256_hex_matches_known_vector() {
    assert_eq!(sha256_hex(b"abc"), SHA256_OF_ABC);
}

#[test]
fn file_hash_is_lowercase_hex_of_sha256() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("abc.txt");
    std::fs::File::create(&path).unwrap().write_all(b"abc").unwrap();

    let walker = FileWalker::new(dir.path().to_str().unwrap(), WalkerConfig::default());
    assert_eq!(walker.calculate_file_hash(&path).unwrap(), SHA256_OF_ABC);
}
