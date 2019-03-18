use crate::error::Error;
use crate::list::FileInfo;
use crate::utils::{get_mime_type_str, Range};
use futures::future::{ok, Either};
use futures::Future;
use futures_fs::FsPool;
use hyper::{Body, Response, StatusCode};
use num_cpus::get as get_num_cpus;
use std::io::SeekFrom;
use tokio_fs::file::File;

static NOTFOUND: &[u8] = b"Not Found";

pub type ResponseFuture =
    Box<Future<Item = Response<Body>, Error = Error> + Send>;

fn not_found() -> Response<Body> {
    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .body(NOTFOUND.into())
        .unwrap() // will success
}

fn internal_server_error() -> Response<Body> {
    Response::builder()
        .status(StatusCode::INTERNAL_SERVER_ERROR)
        .body(Body::empty())
        .unwrap() // will success
}

pub struct Sender {
    pool: FsPool,
}

impl Sender {
    pub fn new() -> Self {
        Sender {
            pool: FsPool::new(get_num_cpus()),
        }
    }

    pub fn send_file(
        &self,
        f: &FileInfo,
        range: Option<Range>,
    ) -> ResponseFuture {
        let file_name = f.0.to_owned();
        let file_length = f.1.len();
        let fspool = self.pool.clone();
        let responder = move |file: File, unit: &str, start: u64| {
            let content_type =
                get_mime_type_str(&file_name).to_string() + ";charset=utf-8";
            let content_range = format!(
                "{} {}-{}/{}",
                unit,
                start,
                file_length - 1,
                file_length,
            );
            let content_length = file_length - start;
            let status = if start > 0 { 206 } else { 200 };

            Response::builder()
                .status(status)
                .header("Content-Type", content_type)
                .header("Content-Length", content_length)
                .header("Accept-Ranges", unit.to_string())
                .header("Content-Range", content_range)
                .body(Body::wrap_stream(
                    fspool.read_file(file.into_std(), Default::default()),
                ))
                .unwrap() // will success
        };

        Box::new(
            File::open(f.0.to_owned())
                .and_then(move |file| match range {
                    Some(Range(ref unit, start, _)) if unit == "bytes" => {
                        Either::A(
                            file.seek(SeekFrom::Start(start))
                                .and_then(move |(file, seek_start)| {
                                    Ok(responder(file, "bytes", seek_start))
                                })
                                .or_else(|_| Ok(internal_server_error())),
                        )
                    }
                    _ => Either::B(ok(responder(file, "bytes", 0))),
                })
                .or_else(|_| Ok(not_found())),
        )
    }

    pub fn send_string(&self, s: &str) -> ResponseFuture {
        Box::new(ok(Response::new(Body::from(s.to_owned()))))
    }

    pub fn send_404(&self) -> ResponseFuture {
        Box::new(ok(not_found()))
    }

    pub fn send_500(&self) -> ResponseFuture {
        Box::new(ok(internal_server_error()))
    }
}
