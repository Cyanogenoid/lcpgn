use flatc_rust;
use std::path::Path;

fn main() {
    let chunk_path = "libs/lczero-common/flat/chunk.fbs";
    println!("cargo:rerun-if-changed={:}", &chunk_path);

    flatc_rust::run(flatc_rust::Args {
        inputs: &[Path::new(&chunk_path)],
        out_dir: Path::new("target/flatbuffers"),
        ..Default::default()
    })
    .expect("flatc");
}
