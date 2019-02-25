use crate::error::Error;
use crate::utils::get_mime_type_str;
use futures::future::ok;
use futures::Future;
use futures_fs::FsPool;
use hyper::{Body, Response, StatusCode};

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

pub fn send_file(f: &str) -> ResponseFuture {
    let filename = f.to_owned();

    let mime = get_mime_type_str(&filename);

    let fspool = FsPool::default();
    let maybe_resp = Response::builder()
        .status(200)
        .header("Content-Type", mime.to_string() + ";charset=utf-8")
        .body(Body::wrap_stream(fspool.read(filename, Default::default())));

    match maybe_resp {
        Ok(resp) => Box::new(ok(resp)),
        _ => send_500(),
    }
}

pub fn send_string(s: &str) -> ResponseFuture {
    Box::new(ok(Response::new(Body::from(s.to_owned()))))
}

pub fn send_404() -> ResponseFuture {
    Box::new(ok(not_found()))
}

pub fn send_500() -> ResponseFuture {
    Box::new(ok(internal_server_error()))
}
