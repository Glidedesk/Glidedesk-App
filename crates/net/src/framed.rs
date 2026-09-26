//! Length-prefixed postcard frames over QUIC streams.

use nexpingdesk_proto::{check_frame_len, decode_body, encode_frame};
use quinn::{ReadExactError, RecvStream, SendStream};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::NetError;

#[derive(Debug)]
pub struct FrameWriter {
    stream: SendStream,
    buf: Vec<u8>,
    limit: usize,
}

impl FrameWriter {
    #[must_use]
    pub fn new(stream: SendStream, limit: usize) -> Self {
        Self { stream, buf: Vec::with_capacity(256), limit }
    }

    pub async fn send<T: Serialize>(&mut self, msg: &T) -> Result<(), NetError> {
        self.buf.clear();
        encode_frame(msg, self.limit, &mut self.buf)?;
        self.stream.write_all(&self.buf).await.map_err(|e| NetError::Stream(e.to_string()))
    }

    /// Queues several messages and writes them with one call (coalescing).
    pub async fn send_batch<'a, T: Serialize + 'a>(
        &mut self,
        msgs: impl IntoIterator<Item = &'a T>,
    ) -> Result<(), NetError> {
        self.buf.clear();
        for m in msgs {
            encode_frame(m, self.limit, &mut self.buf)?;
        }
        if self.buf.is_empty() {
            return Ok(());
        }
        self.stream.write_all(&self.buf).await.map_err(|e| NetError::Stream(e.to_string()))
    }

    /// Gracefully finishes the stream.
    pub fn finish(&mut self) {
        let _ = self.stream.finish();
    }
}

#[derive(Debug)]
pub struct FrameReader {
    stream: RecvStream,
    buf: Vec<u8>,
    limit: usize,
}

impl FrameReader {
    #[must_use]
    pub fn new(stream: RecvStream, limit: usize) -> Self {
        Self { stream, buf: Vec::new(), limit }
    }

    /// Next message, `Ok(None)` on a clean end of stream.
    pub async fn recv<T: DeserializeOwned>(&mut self) -> Result<Option<T>, NetError> {
        let mut header = [0u8; 4];
        match self.stream.read_exact(&mut header).await {
            Ok(()) => {}
            Err(ReadExactError::FinishedEarly(0)) => return Ok(None),
            Err(e) => return Err(NetError::Stream(e.to_string())),
        }
        let len = check_frame_len(header, self.limit)?;
        self.buf.resize(len, 0);
        self.stream.read_exact(&mut self.buf).await.map_err(|e| NetError::Stream(e.to_string()))?;
        Ok(Some(decode_body(&self.buf)?))
    }
}
