use std::{
    io,
    io::prelude::*
};

use clap::{App, Arg, ArgMatches, SubCommand};
use futures::prelude::*;
use futures3::compat::{
    Compat,
    Compat01As03
};
use tokio::runtime::Runtime;

use super::stdin_stream;
use webmetro::{
    chunk::{
        Chunk,
        WebmStream
    },
    error::WebmetroError,
    fixers::{
        ChunkStream,
        ChunkTimecodeFixer,
    },
    stream_parser::StreamEbml
};

pub fn options() -> App<'static, 'static> {
    SubCommand::with_name("filter")
        .about("Copies WebM from stdin to stdout, applying the same cleanup & stripping the relay server does.")
        .arg(Arg::with_name("throttle")
            .long("throttle")
            .help("Slow down output to \"real time\" speed as determined by the timestamps (useful for streaming static files)"))
}

pub fn run(args: &ArgMatches) -> Result<(), WebmetroError> {
    let mut timecode_fixer = ChunkTimecodeFixer::new();
    let mut chunk_stream: Box<dyn Stream<Item = Chunk, Error = WebmetroError> + Send> = Box::new(
        stdin_stream()
        .parse_ebml()
        .chunk_webm()
        .map(move |chunk| timecode_fixer.process(chunk))
    );

    if args.is_present("throttle") {
        chunk_stream = Box::new(Compat::new(Compat01As03::new(chunk_stream).throttle()));
    }

    Runtime::new().unwrap().block_on(chunk_stream.for_each(|chunk| {
        io::stdout().write_all(chunk.as_ref()).map_err(WebmetroError::from)
    }))
}
