use std::{env, fs, path::Path};

const BASE64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn main() {
    embed_png("favicon-192x192.png", "favicon.b64");
    embed_png("logo-512x512.png", "brand-logo.b64");
}

fn embed_png(source: &str, output: &str) {
    println!("cargo:rerun-if-changed={source}");

    let manifest_dir = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is set by Cargo");
    let out_dir = env::var("OUT_DIR").expect("OUT_DIR is set by Cargo");
    let bytes = fs::read(Path::new(&manifest_dir).join(source))
        .unwrap_or_else(|error| panic!("failed to read {source}: {error}"));

    fs::write(Path::new(&out_dir).join(output), encode_base64(&bytes))
        .unwrap_or_else(|error| panic!("failed to write {output}: {error}"));
}

fn encode_base64(bytes: &[u8]) -> String {
    let mut encoded = String::with_capacity(bytes.len().div_ceil(3) * 4);

    for chunk in bytes.chunks(3) {
        let first = chunk[0];
        let second = chunk.get(1).copied().unwrap_or(0);
        let third = chunk.get(2).copied().unwrap_or(0);

        encoded.push(BASE64[(first >> 2) as usize] as char);
        encoded.push(BASE64[(((first & 0b0000_0011) << 4) | (second >> 4)) as usize] as char);

        if chunk.len() > 1 {
            encoded.push(BASE64[(((second & 0b0000_1111) << 2) | (third >> 6)) as usize] as char);
        } else {
            encoded.push('=');
        }

        if chunk.len() > 2 {
            encoded.push(BASE64[(third & 0b0011_1111) as usize] as char);
        } else {
            encoded.push('=');
        }
    }

    encoded
}
