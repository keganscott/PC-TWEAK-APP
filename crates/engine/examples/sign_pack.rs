//! Sign a profile pack (`profile_pack.rs`, NOTES N138).
//!
//! ```text
//! cargo run -p peaktweaks-engine --example sign_pack -- <seed file> <key id> <pack.json> <out.ptpack>
//! cargo run -p peaktweaks-engine --example sign_pack -- --public <seed file>
//! ```
//!
//! The seed file is 32 random bytes kept off this repository (for example
//! made in PowerShell with
//! `[IO.File]::WriteAllBytes("pack-key.bin", [Security.Cryptography.RandomNumberGenerator]::GetBytes(32))`).
//! `--public` prints the public half for `profile_pack::PACK_KEYS`.

use peaktweaks_engine::{profile_pack, signed};

fn seed(path: &str) -> [u8; 32] {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("cannot read {path}: {e}"));
    bytes.try_into().expect("the seed file must be exactly 32 bytes")
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [flag, seed_file] if flag == "--public" => {
            let public = signed::public_key(&seed(seed_file));
            let list: Vec<String> = public.iter().map(|b| format!("0x{b:02x}")).collect();
            println!("[{}]", list.join(", "));
        }
        [seed_file, key_id, pack, out] => {
            let payload = std::fs::read(pack).unwrap_or_else(|e| panic!("cannot read {pack}: {e}"));
            let text = signed::sign(&seed(seed_file), key_id, profile_pack::PURPOSE, &payload);
            std::fs::write(out, text).unwrap_or_else(|e| panic!("cannot write {out}: {e}"));
            println!("signed {pack} as {out}");
        }
        _ => {
            eprintln!("usage: sign_pack <seed file> <key id> <pack.json> <out.ptpack> | --public <seed file>");
            std::process::exit(2);
        }
    }
}
