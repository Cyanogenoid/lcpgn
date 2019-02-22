use chess;
use pgn_reader;
use structopt::StructOpt;

#[allow(non_snake_case)]
#[path = "../target/flatbuffers/chunk_generated.rs"]
mod chunk_generated;

#[derive(StructOpt, Debug)]
#[structopt(name = "lcpgn")]
struct Opt {
    /// Files to process
    #[structopt(name = "FILE", parse(from_os_str))]
    files: Vec<std::path::PathBuf>,
}

fn main() {
    let opt = Opt::from_args();
    println!("{:?}", opt);
}
