use std::net::ToSocketAddrs;
use std::sync::{
    Arc,
    Mutex,
    Weak
};

use bytes::{Bytes, Buf};
use clap::{App, Arg, ArgMatches, SubCommand};
use futures::{
    Future,
    Stream,
    Sink,
    stream::empty
};
use futures3::{
    compat::{
        Compat,
        CompatSink,
        Compat01As03,
    },
    Never,
    prelude::*,
};
use hyper::{
    Body,
    Response,
    header::{
        CACHE_CONTROL,
        CONTENT_TYPE
    }
};
use warp::{
    self,
    Filter,
    path
};
use weak_table::{
    WeakValueHashMap
};
use webmetro::{
    channel::{
        Channel,
        Handle,
        Listener,
        Transmitter
    },
    chunk::WebmStream,
    error::WebmetroError,
    fixers::{
        ChunkStream,
        ChunkTimecodeFixer,
    },
    stream_parser::StreamEbml
};

const BUFFER_LIMIT: usize = 2 * 1024 * 1024;

fn get_stream(channel: Handle) -> impl Stream<Item = Bytes, Error = WebmetroError> {
    let mut timecode_fixer = ChunkTimecodeFixer::new();
    Compat::new(Listener::new(channel).map(|c| Ok(c))
    .map_ok(move |chunk| timecode_fixer.process(chunk))
    .find_starting_point()
    .map_ok(|webm_chunk| webm_chunk.into_bytes())
    .map_err(|err: Never| match err {}))
}

fn post_stream(channel: Handle, stream: impl Stream<Item = impl Buf, Error = warp::Error>) -> impl Stream<Item = Bytes, Error = WebmetroError> {
    let source = Compat01As03::new(stream
        .map_err(WebmetroError::from))
        .parse_ebml().with_soft_limit(BUFFER_LIMIT)
        .chunk_webm().with_soft_limit(BUFFER_LIMIT);
    let sink = CompatSink::new(Transmitter::new(channel));

    Compat::new(source).forward(sink.sink_map_err(|err| -> WebmetroError {match err {}}))
    .into_stream()
    .map(|_| empty())
    .map_err(|err| {
        warn!("{}", err);
        err
    })
    .flatten()
}

fn media_response(body: Body) -> Response<Body> {
    Response::builder()
        .header(CONTENT_TYPE, "video/webm")
        .header("X-Accel-Buffering", "no")
        .header(CACHE_CONTROL, "no-cache, no-store")
        .body(body)
        .unwrap()
}

pub fn options() -> App<'static, 'static> {
    SubCommand::with_name("relay")
        .about("Hosts an HTTP-based relay server")
        .arg(Arg::with_name("listen")
            .help("The address:port to listen to")
            .required(true))
}

pub fn run(args: &ArgMatches) -> Result<(), WebmetroError> {
    let channel_map = Arc::new(Mutex::new(WeakValueHashMap::<String, Weak<Mutex<Channel>>>::new()));
    let addr_str = args.value_of("listen").ok_or("Listen address wasn't provided")?;

    let addrs = addr_str.to_socket_addrs()?;
    info!("Binding to {:?}", addrs);
    if addrs.len() == 0 {
        return Err("Listen address didn't resolve".into());
    }

    let channel = path!("live" / String).map(move |name: String| {
        let channel = channel_map.lock().unwrap()
            .entry(name.clone())
            .or_insert_with(|| Channel::new(name.clone()));
        (channel, name)
    });

    let head = channel.clone().and(warp::head())
        .map(|(_, name)| {
            info!("HEAD Request For Channel {}", name);
            media_response(Body::empty())
        });

    let get = channel.clone().and(warp::get2())
        .map(|(channel, name)| {
            info!("Listener Connected On Channel {}", name);
            media_response(Body::wrap_stream(get_stream(channel)))
        });

    let post_put = channel.clone().and(warp::post2().or(warp::put2()).unify())
        .and(warp::body::stream()).map(|(channel, name), stream| {
            info!("Source Connected On Channel {}", name);
            Response::new(Body::wrap_stream(post_stream(channel, stream)))
        });

    let routes = head
        .or(get)
        .or(post_put);

    let mut rt = tokio::runtime::Runtime::new()?;

    for do_serve in addrs.map(|addr| warp::serve(routes.clone()).try_bind(addr)) {
        rt.spawn(do_serve);
    }

    rt.shutdown_on_idle().wait().map_err(|_| "Shutdown error.".into())
}
