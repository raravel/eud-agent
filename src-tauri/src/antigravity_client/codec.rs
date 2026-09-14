mod metadata;
mod request;
mod stream;

pub(in crate::antigravity_client) use request::request_body;
pub(in crate::antigravity_client) use stream::decode_stream;
