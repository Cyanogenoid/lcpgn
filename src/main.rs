mod policy_map;
#[allow(non_snake_case)]
#[path = "../target/flatbuffers/chunk_generated.rs"]
#[allow(clippy::all)]
mod chunk_generated;

use chunk_generated::flatlczero as flat;
use flatbuffers::{FlatBufferBuilder, WIPOffset};
use pgn_reader::{BufferedReader, RawHeader, SanPlus, Skip, Visitor};
use shakmaty::fen::Fen;
use shakmaty::{
    Chess, Color, Move, Outcome, Pieces, Position, Role, Setup, Square,
};
use std::cell::RefCell;
use std::convert::From;
use std::fs::File;
use std::path::PathBuf;
use std::io::Write;
use std::fs::create_dir_all;
use structopt::StructOpt;
use flate2::write::GzEncoder;
use flate2::Compression;

#[derive(StructOpt, Debug)]
#[structopt(name = "lcpgn")]
struct Opt {
    /// Files to process
    #[structopt(name = "FILE", parse(from_os_str))]
    files: Vec<std::path::PathBuf>,
}

struct Chunk<'fbb> {
    pos: Chess,
    builder: RefCell<FlatBufferBuilder<'fbb>>,
    states: Vec<WIPOffset<flat::State<'fbb>>>,
    path: PathBuf,
    move_table: Vec<u16>,
    game_id: i32,
    result: flat::Result,
}

impl<'fbb> Chunk<'fbb> {
    fn new(mut path: PathBuf) -> Chunk<'fbb> {
        let folder_name = path.file_stem().unwrap().to_os_string();
        // get rid of filename
        path.pop();
        // put in directory (without extension) instead
        path.push(&folder_name);
        create_dir_all(&path).expect("couldn't create directory");
        path.push("default");

        Chunk {
            pos: Chess::default(),
            builder: RefCell::new(FlatBufferBuilder::new()),
            states: Vec::new(),
            path,
            move_table: build_move_table(),
            game_id: -1,
            result: flat::Result::Draw,
        }
    }

    fn pieces_to_vec(pieces: Pieces, color: Color) -> (Vec<flat::PieceType>, Vec<u8>) {
        let role_to_type = |role| match role {
            Role::Pawn => flat::PieceType::Pawn,
            Role::Knight => flat::PieceType::Knight,
            Role::Bishop => flat::PieceType::Bishop,
            Role::Rook => flat::PieceType::Rook,
            Role::Queen => flat::PieceType::Queen,
            Role::King => flat::PieceType::King,
        };
        pieces
            .filter(|(_, piece)| piece.color == color)
            .map(|(square, piece)| (role_to_type(piece.role), u8::from(square)))
            .unzip()
    }

    fn pieces(&self, color: Color) -> flat::PiecesArgs<'fbb> {
        let (types, indices) = Chunk::pieces_to_vec(self.pos.board().pieces(), color);
        let mut builder = self.builder.borrow_mut();
        let types = builder.create_vector(&types);
        let indices = builder.create_vector(&indices);
        flat::PiecesArgs {
            types: Some(types),
            indices: Some(indices),
        }
    }

}

fn move_to_nn_index(m: &Move, table: &[u16]) -> u16 {
    let index = move_to_packed_int(&m);
    table[index as usize]
}

fn build_move_table() -> Vec<u16> {
    // map packed index to nn index
    let mut table: Vec<u16> = vec![0; 4 * 64 * 64];
    for (i, &m) in policy_map::POLICY_INDEX.iter().enumerate() {
        let b= m.as_bytes();
        let from = Square::from_ascii(&b[0..2]).expect("bad policy index");
        let to = Square::from_ascii(&b[2..4]).expect("bad policy_index");
        let promotion = b.get(4);
        let m = Move::Normal {
            from,
            to,
            promotion: match promotion {
                None => None,
                Some(b'q') => Some(Role::Queen),
                Some(b'r') => Some(Role::Rook),
                Some(b'b') => Some(Role::Bishop),
                // knight promotion is stored without promotion suffix, so covered by None branch
                Some(_) => panic!("unrecognised promotion in policy index"),
            },
            // these don't matter, just put in anything
            role: Role::Pawn,
            capture: None,
        };
        table[move_to_packed_int(&m) as usize] = i as u16;
    }
    table
}

fn move_to_packed_int(m: &Move) -> u16 {
    // same packed int format as
    // https://github.com/LeelaChessZero/lc0/blob/9d374646c527e5575179d131c992eb9b2ddc27dc/src/chess/bitboard.cc#L314-L321
    let from: u16 = m.from().unwrap().into();
    let to: u16 = m.to().into();
    // https://github.com/LeelaChessZero/lc0/blob/9d374646c527e5575179d131c992eb9b2ddc27dc/src/chess/bitboard.h#L238
    let promotion: u16 = match m.promotion() {
        None => 0,
        Some(Role::Knight) => 0,
        Some(Role::Queen) => 1,
        Some(Role::Rook) => 2,
        Some(Role::Bishop) => 3,
        Some(Role::Pawn) => panic!("Tried to promote to a pawn"),
        Some(Role::King) => panic!("Tried to promote to a king"),
    };
    // bits 0..6: to square
    // bits 6..12: from square
    // bits 12..14: promotion
    promotion * 64 * 64 & from * 64 & to
}


impl<'fbb> Visitor for Chunk<'fbb> {
    type Result = ();

    fn begin_game(&mut self) {
        self.builder.borrow_mut().reset();
        self.states.clear();
        self.pos = Chess::default();
        self.game_id += 1;
    }

    fn header(&mut self, key: &[u8], value: RawHeader<'_>) {
        // Support games from a non-standard starting position.
        if key == b"FEN" {
            let pos = Fen::from_ascii(value.as_bytes())
                .ok()
                .and_then(|f| f.position().ok());

            if let Some(pos) = pos {
                self.pos = pos;
            }
        }
    }

    fn san(&mut self, san_plus: SanPlus) {
        // construct position
        // first, create the board position
        let white;
        let black;
        {
            let args = &Chunk::pieces(&self, Color::White);
            white = flat::Pieces::create(&mut self.builder.borrow_mut(), args);
        }
        {
            let args = &Chunk::pieces(&self, Color::Black);
            black = flat::Pieces::create(&mut self.builder.borrow_mut(), args);
        }
        let castling = self.pos.castling_rights();
        let position_args = flat::PositionArgs {
            white: Some(white),
            black: Some(black),
            repetitions: 0, // TODO
            us_ooo: castling.contains(Square::A1),
            us_oo: castling.contains(Square::H1),
            them_ooo: castling.contains(Square::A8),
            them_oo: castling.contains(Square::H8),
            side_to_move: match self.pos.turn() {
                Color::White => flat::Side::White,
                Color::Black => flat::Side::Black,
            },
            rule_50: 0,
        };
        let position = flat::Position::create(&mut self.builder.borrow_mut(), &position_args);

        // second, create policy
        let legal_moves = self.pos.legals();
        let legal_indices: Vec<u16> = legal_moves
            .iter()
            .map(|m| { move_to_nn_index(&m, &self.move_table) })
            .collect();
        let policy_indices = self.builder.borrow_mut().create_vector(&legal_indices);

        let played_move = san_plus.san.to_move(&self.pos).expect("couldn't parse move");
        self.pos.play_unchecked(&played_move);
        let probabilities: Vec<f32> = legal_moves
            .iter()
            .map(|legal|
                if move_to_packed_int(&legal) == move_to_packed_int(&played_move) {
                    1.0
                } else {
                    0.0
                })
            .collect();
        let policy_probabilities = self.builder.borrow_mut().create_vector(&probabilities);
        let policy = flat::Policy::create(&mut self.builder.borrow_mut(), &flat::PolicyArgs {
            index: Some(policy_indices),
            probability: Some(policy_probabilities),
        });

        let state = flat::State::create(&mut self.builder.borrow_mut(), &flat::StateArgs {
            position: Some(position),
            policy: Some(policy),
            evaluation: None,
        });
        self.states.push(state);
    }

    fn begin_variation(&mut self) -> Skip {
        Skip(true) // stay in the mainline
    }

    fn outcome(&mut self, outcome: Option<Outcome>) {
        self.result = match outcome {
            None => panic!("game ended, but no outcome"),
            Some(Outcome::Draw) => flat::Result::Draw,
            Some(Outcome::Decisive {
                winner: Color::White,
            }) => flat::Result::White,
            Some(Outcome::Decisive {
                winner: Color::Black,
            }) => flat::Result::Black,
        }
    }

    fn end_game(&mut self) -> Self::Result {
        let states = self.builder.borrow_mut().create_vector(&self.states);
        let mut builder = self.builder.borrow_mut();
        let game = flat::Game::create(&mut builder, &flat::GameArgs {
            states: Some(states),
            winner: self.result,
        });
        builder.finish_minimal(game);
        let data = builder.finished_data();
        // write out data
        self.path.set_file_name(format!("game_{}.gz", &self.game_id));
        let file = File::create(&self.path).unwrap();
        let mut encoder = GzEncoder::new(file, Compression::default());
        encoder.write_all(data).unwrap();
    }
}

fn main() -> std::io::Result<()> {
    let opt = Opt::from_args();

    for filename in opt.files {
        let file = File::open(&filename).expect("failed to open file");
        let mut reader = BufferedReader::new(file);

        let mut chunk_writer = Chunk::new(filename);
        reader
            .read_all(&mut chunk_writer)
            .expect("failed to read file");
    }
    Ok(())
}
